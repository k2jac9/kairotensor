"""Versioned local-cache backend calibration; CLI permits injected fakes only."""
import time
_START_MODE='linux_boottime' if hasattr(time,'CLOCK_BOOTTIME') else 'fixture_monotonic'
_START_NS=time.clock_gettime_ns(time.CLOCK_BOOTTIME) if _START_MODE=='linux_boottime' else time.monotonic_ns()
import argparse
import base64
import copy
import hashlib
import os
from pathlib import Path
import selectors
import sqlite3
import subprocess
import sys

from incident_routing import canonical,digest,require,_loads
from incident_preflight import calibration_plan,PreparedBridge,MODEL,REVISION
from incident_calibration_process import arm_parent_death
import incident_calibration_controller as ledger
from incident_calibration_fixture import now_ns
from incident_transformers_backend import InvocationApproval, REQUIRED_VERSIONS

VERSION='incident-transformers-controller/1'
WIRE='incident-transformers/1'
FAULTS={'none','load_failure','runtime_mismatch','wrong_prefix','overcap','decode_failure','hang_generation',
        'after_claim','after_receipt','sqlite_settle_failure','raw_receipt_failure','late_persistence','missing_observation'}


def _sources():
    names=('incident_transformers_controller.py','incident_transformers_worker.py','incident_transformers_backend.py',
           'incident_transformers_fake.py','incident_calibration_controller.py','incident_calibration_process.py','incident_preflight.py','handoff_model.py')
    return {n:hashlib.sha256(Path(__file__).with_name(n).read_bytes()).hexdigest() for n in names}


def _validate_manifest(m):
    require(type(m) is dict and m['version']==VERSION,'version mismatch')
    require(m['backend'] in ('offline_fake_dependencies','local_cached_model'),'backend mismatch')
    require(m['sources']==_sources(),'source mismatch')
    require(canonical(m['plan'])==canonical(calibration_plan()),'plan mismatch')
    require(m['runtime']=={'model':MODEL,'revision':REVISION,'dtype':'bfloat16','device':'cpu','backend':m['backend']},'requested runtime mismatch')
    require(m['clock_mode'] in ('linux_boottime','fixture_monotonic'),'clock mode')
    require(all(type(m[k]) is int for k in ('start_ns','work_deadline_ns','deadline_ns')),'clock types')
    require(0<=m['start_ns']<m['work_deadline_ns']<m['deadline_ns'] and m['deadline_ns']-m['start_ns']<=1200000*1000000,'clock window')
    require(m['fault'] in FAULTS and (m['backend']=='offline_fake_dependencies' or m['fault']=='none'),'fault mode')
    require(m['no_resume'] is True,'restart denied')


def _envelope(value,op,runtime=None):
    require(type(value) is dict and set(value)=={'wire_version','op','runtime_sha256','reply','observation','error'},'wire fields')
    require(value['wire_version']==WIRE and value['op']==op,'wire version/op mismatch')
    require(value['error'] is None or (type(value['error']) is str and bool(value['error'])),'wire error shape')
    if runtime is not None:require(value['runtime_sha256']==runtime,'runtime binding changed')
    return value


def _runtime(value,manifest):
    _envelope(value,'ready')
    require(value['error'] is None and value['reply'] is None and type(value['observation']) is dict,'runtime unavailable')
    observation=value['observation']
    require(value['runtime_sha256']==digest(observation),'runtime digest mismatch')
    expected={'backend','mode','fixture_identity','model','revision','dtype','device','versions','asset_manifest',
              'threads','affinity','machine_architecture','python_version','chat_template_sha256','model_config',
              'generation_config','load_us','source_sha256','tokenization_route','cpu_identity','tokenizer_config'}
    require(set(observation)==expected,'runtime schema')
    require(observation['backend']==WIRE and observation['mode']==manifest['backend'],'runtime mode mismatch')
    require((type(observation['fixture_identity']) is str and bool(observation['fixture_identity'])) if manifest['backend']=='offline_fake_dependencies' else observation['fixture_identity'] is None,'fake/real identity mismatch')
    require(observation['model']==MODEL and observation['revision']==REVISION,'observed model mismatch')
    require(observation['dtype']=='bfloat16' and observation['device']=='cpu','observed placement mismatch')
    require(observation['versions']==REQUIRED_VERSIONS,'observed dependency mismatch')
    require(type(observation['threads']) is int and 1<=observation['threads']<=4,'observed threads')
    affinity=observation['affinity']
    require(affinity is None or (type(affinity) is list and bool(affinity) and all(type(v) is int and v>=0 for v in affinity) and affinity==sorted(set(affinity))),'observed affinity')
    for key in ('machine_architecture','python_version'):
        require(type(observation[key]) is str and bool(observation[key]),'missing runtime identity')
    require(type(observation['load_us']) is int and observation['load_us']>=0,'load measurement')
    require(type(observation['model_config']) is dict and type(observation['generation_config']) is dict,'runtime config')
    require(observation['tokenization_route']=='render_once_then_tokenize_no_added_special_tokens/1','tokenization route')
    hashes=observation['asset_manifest']
    require(type(hashes) is dict and bool(hashes),'asset hashes required')
    for name,sha in hashes.items():
        require(type(name) is str and name and not Path(name).is_absolute() and '..' not in Path(name).parts,'relative asset path required')
        require(type(sha) is str and len(sha)==64 and all(c in '0123456789abcdef' for c in sha),'asset SHA256')
    sha=observation['chat_template_sha256']
    require(type(sha) is str and len(sha)==64 and all(c in '0123456789abcdef' for c in sha),'template SHA256')
    cpu=observation['cpu_identity']
    require(type(cpu) is dict and set(cpu)=={'source','model_names','flags','logical_processor_count'},'CPU identity schema')
    require(cpu['source']==('synthetic' if manifest['backend']=='offline_fake_dependencies' else 'linux_proc_cpuinfo'),'CPU provenance')
    require(type(cpu['logical_processor_count']) is int and cpu['logical_processor_count']>0,'CPU count')
    for key in ('model_names','flags'):
        require(type(cpu[key]) is list and bool(cpu[key]) and all(type(v) is str and v for v in cpu[key]) and cpu[key]==sorted(set(cpu[key])),'CPU identity missing')
    tokenizer=observation['tokenizer_config']
    require(type(tokenizer) is dict and set(tokenizer)=={'class_name','module','is_fast','model_max_length','padding_side','truncation_side','special_tokens_map','init_kwargs','omitted_init_keys'},'tokenizer config schema')
    require(type(tokenizer['is_fast']) is bool and type(tokenizer['model_max_length']) is int and tokenizer['model_max_length']>0,'tokenizer config types')
    require(all(type(tokenizer[k]) is str and tokenizer[k] for k in ('class_name','module')) and tokenizer['padding_side'] in ('left','right') and tokenizer['truncation_side'] in ('left','right'),'tokenizer identity')
    require(type(tokenizer['special_tokens_map']) is dict and type(tokenizer['init_kwargs']) is dict and type(tokenizer['omitted_init_keys']) is list and all(type(v) is str for v in tokenizer['omitted_init_keys']),'tokenizer config shape')
    require(observation['source_sha256']=={name:manifest['sources'][name] for name in ('incident_transformers_backend.py','handoff_model.py','incident_preflight.py')},'backend source provenance')
    return observation


def _prepared(value,manifest,slot,runtime):
    _envelope(value,'prepare',runtime)
    require(value['error'] is None and type(value['observation']) is dict and type(value['reply']) is dict,'preparation failed')
    p=value['observation'];reply=value['reply']
    require(set(p)=={'request_sha256','handle','rendered_chat','input_ids','attention_mask','effective_generation_config','runtime_sha256','prepared_sha256'} and p['runtime_sha256']==runtime,'prepared schema/runtime')
    require(p['request_sha256']==manifest['plan']['schedule'][slot]['request_sha256'] and p['handle']==reply['handle'],'preparation binding')
    ids=p['input_ids'];mask=p['attention_mask']
    require(type(ids) is list and len(ids)==1 and type(ids[0]) is list and 1<=len(ids[0])<=2048,'input batch/coverage')
    require(all(type(v) is int and 0<=v<2**63 for v in ids[0]),'input IDs')
    require(mask==[[1]*len(ids[0])] and type(mask) is list and all(type(v) is int for v in mask[0]),'attention mask')
    require(type(reply['input_tokens']) is int and reply['input_tokens']==len(ids[0]) and type(reply['max_new_tokens']) is int and reply['max_new_tokens']==96,'prepared quote')
    require(type(p['rendered_chat']) is str and bool(p['rendered_chat']),'rendered chat missing');p['rendered_chat'].encode('utf-8')
    require(p['prepared_sha256']==digest({k:v for k,v in p.items() if k!='prepared_sha256'}),'prepared digest')
    config=p['effective_generation_config']
    require(type(config) is dict and config.get('do_sample') is False and type(config.get('num_beams')) is int and config['num_beams']==1 and type(config.get('max_new_tokens')) is int and config['max_new_tokens']==96,'effective generation bounds')
    return p


def _generated(value,prepared,runtime):
    _envelope(value,'generate',runtime)
    p=value['observation']
    if p is None:
        require(value['reply'] is None and type(value['error']) is str and bool(value['error']),'missing continuation observation for reply')
        return None
    require(type(p) is dict and p['request_sha256']==prepared['request_sha256'] and p['handle']==prepared['handle'] and p['prepared_sha256']==prepared['prepared_sha256'],'generation binding')
    # Only validated returned continuation observations can establish usage.
    if p.get('continuation_ids') is None:
        require(value['reply'] is None and p.get('usage_known') is False and type(value['error']) is str,'unknown observation cannot carry reply')
        return None
    require(set(p)=={'request_sha256','handle','prepared_sha256','continuation_ids','input_tokens','output_tokens','generation_us','usage_known','hit_cap','ended_eos'} and p['usage_known'] is True,'known observation schema')
    ids=p['continuation_ids']
    require(type(ids) is list and len(ids)<=96 and all(type(v) is int and 0<=v<2**63 for v in ids),'continuation IDs')
    require(type(p['input_tokens']) is int and p['input_tokens']==len(prepared['input_ids'][0]) and type(p['output_tokens']) is int and p['output_tokens']==len(ids),'observed usage')
    require(type(p['generation_us']) is int and 0<=p['generation_us']<2**63,'observed duration')
    eos=prepared['effective_generation_config'].get('eos_token_id')
    eos=[eos] if type(eos) is int else eos or []
    require(type(eos) is list and all(type(v) is int and v>=0 for v in eos),'EOS identity')
    require(type(p['hit_cap']) is bool and p['hit_cap']==(len(ids)==96) and type(p['ended_eos']) is bool and p['ended_eos']==(bool(ids) and ids[-1] in eos),'continuation flags')
    reply=value['reply']
    if reply is not None:
        require(value['error'] is None,'successful reply carries error')
        require(ledger._valid_receipt(canonical(reply).encode(),p['input_tokens'],96)==p['input_tokens']+len(ids),'reply usage mismatch')
        require(reply['generation_us']==p['generation_us'] and reply['hit_cap']==p['hit_cap'] and reply['ended_eos']==p['ended_eos'],'reply observation mismatch')
    else:require(type(value['error']) is str and bool(value['error']),'missing reply without failure')
    return p['input_tokens']+len(ids)


class Transport(ledger._Transport):
    def __init__(self,*args):
        super().__init__(*args);self.runtime=None;self.prepared={}
    def _write(self,command):
        payload=canonical(command).encode()+b'\n'
        require(len(payload)<=65536,'command exceeds wire bound')
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdin,selectors.EVENT_WRITE)
            while payload:
                remaining=(self.manifest['work_deadline_ns']-now_ns(self.manifest['clock_mode']))/1e9
                if remaining<=0:raise TimeoutError('command deadline reached')
                if not selector.select(min(.05,remaining)):continue
                require(now_ns(self.manifest['clock_mode'])<self.manifest['work_deadline_ns'],'command deadline reached')
                payload=payload[os.write(self.process.stdin.fileno(),payload):]
    def __call__(self,command):
        self._write(command)
        deadline=(self.manifest['deadline_ns']+self.manifest['work_deadline_ns'])//2 if command['op']=='generate' else self.manifest['work_deadline_ns']
        raw=self._line(deadline)
        if command['op']=='generate':
            # Save the full raw envelope before parsing or trusting any returned
            # metadata, including decode failures with valid continuation IDs.
            record={'slot':self.slot,'request_sha256':self.manifest['plan']['schedule'][self.slot]['request_sha256'],'observed_ns':now_ns(self.manifest['clock_mode']),'raw_base64':base64.b64encode(raw).decode()}
            durable=False
            try:
                ledger._durable_new(self.out/f'backend-receipt-{self.slot}.json',record,fail_fsync=self.fault=='raw_receipt_failure')
                durable=True
            except OSError as error:self.errors.append(f'backend receipt artifact: {error}')
            if self.fault=='late_persistence':
                remaining=(self.manifest['work_deadline_ns']+20000000-now_ns(self.manifest['clock_mode']))/1e9
                if remaining>0:time.sleep(remaining)
            try:
                with self.db:ledger._event(self.db,'backend_received',self.slot,record,self.manifest['clock_mode'])
                durable=True
            except Exception as error:self.errors.append(f'backend receipt ledger: {error}')
            require(durable,'both raw backend receipt persistence routes failed')
        value=_loads(raw)
        if command['op']=='prepare':
            p=_prepared(value,self.manifest,self.slot,self.runtime)
            ledger._durable_new(self.out/f'prepared-{self.slot}.json',value)
            with self.db:ledger._event(self.db,'preparation_observed',self.slot,{'artifact_sha256':digest(value),'prepared_sha256':p['prepared_sha256']},self.manifest['clock_mode'])
            self.prepared[self.slot]=p
        elif command['op']=='generate':
            _generated(value,self.prepared[self.slot],self.runtime)
            if value['reply'] is not None:
                ledger._receipt(self.db,self.out,self.slot,canonical(value['reply']).encode(),self.manifest,self.fault,self.errors,observed_ns=record['observed_ns'])
        else:_envelope(value,command['op'],self.runtime)
        require(value['error'] is None and value['reply'] is not None,'worker returned failure; retained observation requires review')
        return value['reply']


def _child(out,parent_pid,invocation):
    arm_parent_death(parent_pid)
    require(invocation in ('offline_fake_dependencies','local_cached_model'),'invocation denied')
    manifest=_loads((out/'manifest.json').read_bytes());_validate_manifest(manifest)
    require(manifest['backend']==invocation,'invocation/manifest mismatch')
    require(manifest['start_ns']<=now_ns(manifest['clock_mode'])<manifest['work_deadline_ns'],'child outside original window')
    db=ledger._initialize(out,manifest['plan'],manifest['clock_mode']);worker=None;errors=[]
    try:
        if manifest['fault']=='sqlite_settle_failure':db.execute("CREATE TRIGGER inject_settle BEFORE UPDATE OF charged_units ON slots BEGIN SELECT RAISE(ABORT,'injected settlement failure'); END")
        worker=subprocess.Popen([sys.executable,str(Path(__file__).with_name('incident_transformers_worker.py')),'--invocation',invocation,'--parent-pid',str(os.getpid()),'--fault',manifest['fault'] if manifest['fault'] in {'load_failure','runtime_mismatch','wrong_prefix','overcap','decode_failure','hang_generation','missing_observation'} else 'none'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,bufsize=0)
        transport=Transport(worker,manifest,db,out,manifest['fault'],errors)
        ready=transport.ready();_runtime(ready,manifest)
        ledger._durable_new(out/'runtime-observed.json',ready)
        with db:ledger._event(db,'runtime_observed',None,{'artifact_sha256':digest(ready),'runtime_sha256':ready['runtime_sha256']},manifest['clock_mode'])
        transport.runtime=ready['runtime_sha256'];client=PreparedBridge(transport)
        for slot in manifest['plan']['schedule']:
            i=slot['index'];transport.slot=i
            ticket=client.prepare(slot['request_utf8'])
            ledger._reserve(db,i,ticket,manifest)
            ledger._claim(db,i,ticket,manifest,manifest['runtime'])
            if manifest['fault']=='after_claim':os._exit(28)
            reply=client.generate(ticket)
            with db:
                require(db.execute('SELECT state FROM slots WHERE slot=?',(i,)).fetchone()[0]=='received','missing raw receipt')
                units=reply['input_tokens']+reply['output_tokens']
                db.execute("UPDATE slots SET state='settled',charged_units=? WHERE slot=?",(units,i))
                ledger._event(db,'settled',i,{'charged_units':units},manifest['clock_mode'])
            if errors:break
        with db:ledger._event(db,'controller_finished',None,{'errors':errors},manifest['clock_mode'])
    except BaseException as error:
        errors.append(f'{type(error).__name__}: {error}')
        try:
            with db:
                db.execute("UPDATE slots SET state='uncertain' WHERE state='claimed'")
                ledger._event(db,'controller_failed',None,{'errors':errors},manifest['clock_mode'])
        except Exception:pass
    finally:
        if worker is not None:
            worker.kill()
            try:worker.wait(timeout=.2)
            except subprocess.TimeoutExpired:pass
        db.close()
    ledger._durable_new(out/'child-audit.json',{'report':audit_prepared(out),'errors':errors})
    return 0 if not errors else 1


def _observations(out,m,rows,events,seal,observations=None):
    runtime=None;prepared={};received={};filtered=[];previous=m['start_ns'];seen_claim={}
    for name,slot,payload,tick in events:
        require(type(tick) is int and tick>=previous,'observation clock regression');previous=tick
        if seal:require(tick<=seal['finished_ns'],'observation after supervisor finish')
        value=_loads(payload)
        if name=='runtime_observed':
            require(runtime is None and slot is None and len(filtered)==1,'runtime event order')
            ready=_loads((out/'runtime-observed.json').read_bytes());_runtime(ready,m)
            require(value=={'artifact_sha256':digest(ready),'runtime_sha256':ready['runtime_sha256']},'runtime artifact binding')
            runtime=ready['runtime_sha256']
        elif name=='preparation_observed':
            require(runtime is not None and type(slot) is int and slot in range(8) and slot not in prepared,'preparation order')
            artifact=_loads((out/f'prepared-{slot}.json').read_bytes());p=_prepared(artifact,m,slot,runtime)
            require(value=={'artifact_sha256':digest(artifact),'prepared_sha256':p['prepared_sha256']},'prepared artifact binding')
            prepared[slot]=p
        elif name=='backend_received':
            require(type(slot) is int and slot in seen_claim and slot not in received,'backend receipt event order')
            require(type(value) is dict and type(value.get('observed_ns')) is int and value['observed_ns']<=tick,'backend receipt event clock')
            received[slot]=value
        else:
            if name in ('reserved','claimed'):
                require(slot in prepared,'quote before preparation observation')
                require(rows[slot][4:6]==(len(prepared[slot]['input_ids'][0]),96),'ledger/prepared quote mismatch')
                if name=='claimed':seen_claim[slot]=tick
            filtered.append((name,slot,payload,tick))
    known=0
    for slot in range(8):
        path=out/f'backend-receipt-{slot}.json'
        record=received.get(slot)
        if path.exists():
            independent=_loads(path.read_bytes())
            require(record is None or record==independent,'conflicting backend receipts')
            record=independent
        if record is None:
            require(rows[slot][7] is None,'ledger reply without backend receipt')
            continue
        require(type(record) is dict and set(record)=={'slot','request_sha256','observed_ns','raw_base64'} and type(record['slot']) is int and record['slot']==slot and slot in seen_claim,'backend receipt binding')
        require(record['request_sha256']==prepared[slot]['request_sha256'] and type(record['observed_ns']) is int and record['observed_ns']>=seen_claim[slot],'backend receipt identity')
        if seal:require(record['observed_ns']<=seal['finished_ns'],'backend receipt after finish')
        value=_loads(base64.b64decode(record['raw_base64'],validate=True));usage=_generated(value,prepared[slot],runtime)
        if usage is not None:known+=usage
        if rows[slot][7] is not None:require(value['reply'] is not None and canonical(value['reply']).encode()==rows[slot][7] and rows[slot][8]==record['observed_ns'],'backend/ledger reply or timing mismatch')
    if observations is not None:observations['known_observation_units']=known
    return filtered


def audit_prepared(out):
    out=Path(out)
    observations={}
    report=ledger.audit_run(out,_manifest_validator=_validate_manifest,_event_validator=lambda *args:_observations(*args,observations=observations))
    report.update(mode='versioned_backend_calibration',backend='unknown',new_model_calls=None,known_observation_units=None)
    try:
        m=_loads((out/'manifest.json').read_bytes());_validate_manifest(m)
        report['backend']=m['backend']
        if m['backend']=='offline_fake_dependencies':report['new_model_calls']=0
        if report.get('audit_error') is None:report['known_observation_units']=observations.get('known_observation_units')
    except (ValueError,KeyError,TypeError,OSError) as error:
        report.update(complete=False,audit_complete=False,audit_error=str(error),ledger_charged_units=None,unresolved_hold_units=None,known_observation_units=None)
    return report


def run_prepared(out,*,approval=None,fault='none',budget_ms=1200000,audit_reserve_ms=5000,_origin=None):
    mode=_START_MODE;start=now_ns(mode) if _origin is None else _origin
    if type(approval) is not InvocationApproval or approval.kind not in ('offline_fake_dependencies','local_cached_model'):
        raise PermissionError('explicit trusted backend invocation required; artifacts cannot grant execution')
    require(sys.platform.startswith('linux') and hasattr(os,'waitid'),'Linux watchdog required')
    require(type(budget_ms) is int and 0<budget_ms<=1200000 and type(audit_reserve_ms) is int and 0<audit_reserve_ms<budget_ms,'bounded window required')
    require(fault in FAULTS and (approval.kind=='offline_fake_dependencies' or fault=='none'),'fixture fault forbidden')
    plan=calibration_plan();deadline=start+budget_ms*1000000
    m={'version':VERSION,'backend':approval.kind,'sources':_sources(),'plan':plan,'runtime':{'model':MODEL,'revision':REVISION,'dtype':'bfloat16','device':'cpu','backend':approval.kind},'clock_mode':mode,'start_ns':start,'work_deadline_ns':deadline-audit_reserve_ms*1000000,'deadline_ns':deadline,'fault':fault,'no_resume':True}
    out=Path(out);out.mkdir(mode=0o700);ledger._durable_new(out/'manifest.json',m)
    require(now_ns(mode)<m['work_deadline_ns'],'setup consumed execution window')
    command=[sys.executable,str(Path(__file__).resolve()),'_child',str(out),str(os.getpid()),approval.kind]
    ledger._supervise(out,command,deadline,mode,m)
    return audit_prepared(out)


def main(args=None):
    args=sys.argv[1:] if args is None else args
    if len(args)==4 and args[0]=='_child':return _child(Path(args[1]),int(args[2]),args[3])
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='command',required=True)
    fake=sub.add_parser('fixture');fake.add_argument('--out',type=Path,required=True);fake.add_argument('--fault',choices=sorted(FAULTS),default='none')
    audit=sub.add_parser('audit');audit.add_argument('out',type=Path)
    options=parser.parse_args(args)
    report=run_prepared(options.out,approval=InvocationApproval('offline_fake_dependencies'),fault=options.fault,_origin=_START_NS) if options.command=='fixture' else audit_prepared(options.out)
    print(canonical(report));return 0 if report['complete'] else 1

if __name__=='__main__':
    try:raise SystemExit(main())
    except Exception as error:raise SystemExit(f'Calibration refused: {error}')
