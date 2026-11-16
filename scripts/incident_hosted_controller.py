"""Offline hosted accounting experiment. No HTTP client, credentials or live CLI."""
import argparse
import base64
from contextlib import closing
from dataclasses import dataclass,asdict
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import threading
from incident_routing import canonical,_loads,require,FREEZE_SHA256
from incident_adaption_backend import render_case,parse_response

LIMIT=2**63-1
HOLD_STATES=('reserved','claimed','received','uncertain')
SAFE_HEADERS={'content-type','request-id','x-request-id','retry-after'}


def integer(value,name,minimum=0):
    require(type(value) is int and minimum<=value<=LIMIT,f'{name}: integer bound')
    return value

@dataclass(frozen=True)
class OfflineApproval:
    """Explicit fake-only caller marker, not user permission or a capability."""
    kind:str='offline_fixture'

@dataclass(frozen=True)
class OfflinePriceBound:
    maximum:int
    input_rate:int
    output_rate:int
    fixed_fee:int
    maximum_input:int
    maximum_output:int=96
    def validate(self):
        for key,value in asdict(self).items():integer(value,key)
        require(self.maximum>0 and self.maximum_input>0 and self.maximum_output==96,'fixture quote bounds')
        worst=integer(self.maximum_input*self.input_rate+self.maximum_output*self.output_rate+self.fixed_fee,'worst-case synthetic cost')
        require(self.maximum>=worst,'hold does not cover worst-case synthetic quote')
    def estimate(self,usage):
        result=usage['input_tokens']*self.input_rate+usage['output_tokens']*self.output_rate+self.fixed_fee
        return integer(result,'estimated synthetic cost')

@dataclass(frozen=True)
class HttpReceipt:
    status:int
    headers:tuple
    body:bytes

class FixtureTransport:
    """Fixed canned outcome, no redirects/retries/network implementation."""
    def __init__(self,outcome):
        require(type(outcome) is HttpReceipt or isinstance(outcome,Exception),'fixture outcome required')
        self.outcome=outcome;self.calls=0;self.requests=[];self.lock=threading.Lock()
    def send(self,request):
        with self.lock:self.calls+=1;self.requests.append(request)
        if isinstance(self.outcome,Exception):raise self.outcome
        return self.outcome


def _approval(value):
    if type(value) is not OfflineApproval or value.kind!='offline_fixture':
        raise PermissionError('only explicit offline fixture invocation is implemented; live inadmissible')


def _write(path,value):
    data=canonical(value).encode()+b'\n';temp=path.with_name(path.name+'.tmp')
    with temp.open('xb') as f:f.write(data);f.flush();os.fsync(f.fileno())
    if path.exists():raise FileExistsError(path)
    os.rename(temp,path)
    fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
    try:os.fsync(fd)
    finally:os.close(fd)


def _connect(path,readonly=False):
    if readonly:
        header=path.read_bytes()[:20]
        require(header[18:20]==b'\x01\x01' and not any(Path(str(path)+s).exists() for s in ('-wal','-shm')),'only rollback-journal snapshots supported')
        db=sqlite3.connect(path.as_uri()+'?mode=ro',uri=True,timeout=5)
        db.execute('PRAGMA query_only=ON')
    else:
        db=sqlite3.connect(path,timeout=10)
        db.execute('PRAGMA journal_mode=DELETE');db.execute('PRAGMA synchronous=FULL')
    db.row_factory=sqlite3.Row
    return db


def _assess(parsed,quote):
    quote.validate();usage=parsed['usage'];estimate=None;reason=None
    terminal=parsed['response_status'] in ('completed','failed','cancelled','incomplete')
    if parsed['response_bound'] and terminal and usage['status']=='complete':
        if usage['input_tokens']>quote.maximum_input or usage['output_tokens']>quote.maximum_output:reason='usage_exceeds_synthetic_bound'
        else:
            try:estimate=quote.estimate(usage)
            except ValueError:reason='estimated_cost_overflow'
            if estimate is not None and estimate>quote.maximum:estimate=None;reason='estimated_cost_exceeds_hold'
    else:reason='usage_or_attribution_unknown'
    return estimate,reason


class HostedLedger:
    def __init__(self,root):
        self.root=Path(root).resolve();self.path=self.root/'ledger.sqlite'
        require(self.path.is_file(),'existing ledger required')
    @classmethod
    def create(cls,root,*,budget,clock_domain,start,deadline,max_claims=4):
        integer(budget,'budget',1);integer(start,'start');integer(deadline,'deadline');integer(max_claims,'max_claims',1)
        require(max_claims<=4,'at most four initial calibration calls')
        require(deadline>start and type(clock_domain) is str and clock_domain.strip() and clock_domain not in ('unknown','none'),'fixture clock window')
        root=Path(root).resolve();root.mkdir(mode=0o700)
        db=_connect(root/'ledger.sqlite')
        db.executescript('''
        CREATE TABLE meta(id INTEGER PRIMARY KEY CHECK(id=1),budget INTEGER NOT NULL,domain TEXT NOT NULL,start INTEGER NOT NULL,deadline INTEGER NOT NULL,floor INTEGER NOT NULL,max_claims INTEGER NOT NULL);
        CREATE TABLE operations(id INTEGER PRIMARY KEY,key TEXT UNIQUE NOT NULL,case_id TEXT NOT NULL,model TEXT NOT NULL,request BLOB NOT NULL,sha TEXT NOT NULL,provenance TEXT NOT NULL,quote TEXT NOT NULL,state TEXT NOT NULL,estimated INTEGER,parsed TEXT,issue TEXT);
        CREATE TABLE claims(operation INTEGER PRIMARY KEY,tick INTEGER NOT NULL);
        CREATE TABLE receipts(operation INTEGER PRIMARY KEY,payload TEXT NOT NULL);
        CREATE TABLE events(id INTEGER PRIMARY KEY,operation INTEGER,event TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE TRIGGER immutable_meta BEFORE UPDATE OF budget,domain,start,deadline,max_claims ON meta BEGIN SELECT RAISE(ABORT,'immutable window');END;
        CREATE TRIGGER immutable_events_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT,'immutable event');END;
        CREATE TRIGGER immutable_events_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT,'immutable event');END;
        CREATE TRIGGER immutable_binding BEFORE UPDATE OF key,case_id,model,request,sha,provenance,quote ON operations BEGIN SELECT RAISE(ABORT,'immutable request binding');END;
        CREATE TRIGGER immutable_claim_update BEFORE UPDATE ON claims BEGIN SELECT RAISE(ABORT,'immutable claim');END;
        CREATE TRIGGER immutable_claim_delete BEFORE DELETE ON claims BEGIN SELECT RAISE(ABORT,'immutable claim');END;
        CREATE TRIGGER immutable_receipt BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT,'immutable receipt');END;
        ''')
        with db:db.execute('INSERT INTO meta VALUES(1,?,?,?,?,?,?)',(budget,clock_domain,start,deadline,start,max_claims))
        db.close()
        _write(root/'manifest.json',{'version':'incident-hosted-fixture/1','mode':'offline_fixture','live_admissible':False,'new_model_calls':0,'monetary_unit':'synthetic_micro_usd','budget':budget,'max_claims':max_claims,'freeze_sha256':FREEZE_SHA256,'clock_domain':clock_domain,'start':start,'deadline':deadline,'source_sha256':{name:hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in ('incident_hosted_controller.py','incident_adaption_backend.py','incident_routing.py')}})
        return cls(root)
    def _event(self,db,op,name,payload):db.execute('INSERT INTO events(operation,event,payload) VALUES(?,?,?)',(op,name,canonical(payload)))
    def create_operation(self,key,case_id,model,quote,states=None):
        require(type(key) is str and key.strip(),'operation key');require(type(quote) is OfflinePriceBound,'synthetic quote required');quote.validate()
        rendered=render_case(case_id,model,states=states)
        request=rendered['request_bytes'];provenance={k:v for k,v in rendered.items() if k!='request_bytes'}
        binding=(case_id,model,request,rendered['request_sha256'],canonical(provenance),canonical(asdict(quote)))
        with closing(_connect(self.path)) as db, db:
            db.execute('BEGIN IMMEDIATE')
            prior=db.execute('SELECT * FROM operations WHERE key=?',(key,)).fetchone()
            if prior:
                require(tuple(prior[k] for k in ('case_id','model','request','sha','provenance','quote'))==binding,'key bindings differ')
                return prior['id']
            op=db.execute("INSERT INTO operations(key,case_id,model,request,sha,provenance,quote,state) VALUES(?,?,?,?,?,?,?,'planned')",(key,)+binding).lastrowid
            self._event(db,op,'planned',{'request_sha256':binding[3]});return op
    @staticmethod
    def _totals(db):
        spent=held=0
        for row in db.execute('SELECT state,estimated,quote FROM operations'):
            if row['estimated'] is not None:spent+=integer(row['estimated'],'estimate')
            if row['state'] in HOLD_STATES:held+=_loads(row['quote'])['maximum']
        return integer(spent,'aggregate estimate'),integer(held,'aggregate hold')
    def reserve(self,op):
        with closing(_connect(self.path)) as db, db:
            db.execute('BEGIN IMMEDIATE');r=db.execute('SELECT * FROM operations WHERE id=?',(op,)).fetchone();require(r is not None,'operation missing')
            if r['state']!='planned':return r['state']=='reserved'
            spent,held=self._totals(db);maximum=_loads(r['quote'])['maximum'];budget=db.execute('SELECT budget FROM meta').fetchone()[0]
            accepted=spent+held+maximum<=budget;state='reserved' if accepted else 'denied'
            db.execute('UPDATE operations SET state=? WHERE id=?',(state,op));self._event(db,op,state,{'bound':maximum});return accepted
    def claim(self,op,*,approval=None,clock_domain=None,clock=None):
        _approval(approval);require(callable(clock),'trusted fixture clock required')
        with closing(_connect(self.path)) as db, db:
            db.execute('BEGIN IMMEDIATE');r=db.execute('SELECT * FROM operations WHERE id=?',(op,)).fetchone();require(r is not None,'operation missing')
            if r['state']!='reserved':return None
            m=db.execute('SELECT * FROM meta').fetchone()
            if clock_domain!=m['domain']:self._event(db,op,'refused',{'reason':'clock_domain'});return None
            if db.execute("SELECT 1 FROM operations WHERE id<>? AND state IN ('claimed','received','uncertain')",(op,)).fetchone():
                self._event(db,op,'refused',{'reason':'prior_unresolved'});return None
            if db.execute('SELECT count(*) FROM claims').fetchone()[0]>=m['max_claims']:
                self._event(db,op,'refused',{'reason':'call_limit'});return None
            spent,held=self._totals(db);require(spent+held<=m['budget'],'budget invariant')
            require(hashlib.sha256(r['request']).hexdigest()==r['sha'],'request binding invalid')
            now=integer(clock(),'clock')
            if now<m['floor'] or now<m['start'] or now>=m['deadline']:
                self._event(db,op,'refused',{'reason':'clock_window','tick':now});return None
            db.execute('UPDATE meta SET floor=?',(now,));db.execute('INSERT INTO claims VALUES(?,?)',(op,now))
            db.execute("UPDATE operations SET state='claimed' WHERE id=?",(op,));self._event(db,op,'claimed',{'tick':now})
            return bytes(r['request'])
    def cancel(self,op):
        with closing(_connect(self.path)) as db, db:
            db.execute('BEGIN IMMEDIATE');r=db.execute('SELECT state FROM operations WHERE id=?',(op,)).fetchone();require(r is not None,'operation missing')
            if r[0]=='cancelled':return True
            if r[0] not in ('planned','reserved'):return False
            db.execute("UPDATE operations SET state='cancelled' WHERE id=?",(op,));self._event(db,op,'cancelled',{});return True
    def record_receipt(self,op,receipt,*,observed,minimum_tick=None):
        require(type(receipt) is HttpReceipt and type(receipt.body) is bytes,'HTTP fixture receipt required')
        if observed is not None:integer(observed,'receipt clock')
        require(len(receipt.body)<=1024*1024,'receipt exceeds storage bound')
        headers=[]
        for key,value in receipt.headers:
            require(type(key) is str and type(value) is str,'header strings required')
            if key.lower() in SAFE_HEADERS:headers.append([key.lower(),value[:4096]])
        db=_connect(self.path);durable=False;issues=[]
        try:
            db.execute('BEGIN IMMEDIATE')
            row=db.execute('SELECT c.tick,o.sha,m.floor,m.deadline FROM claims c JOIN operations o ON o.id=c.operation CROSS JOIN meta m WHERE operation=?',(op,)).fetchone()
            require(row is not None,'receipt without prior claim')
            send_tick=row['tick'] if minimum_tick is None else integer(minimum_tick,'send clock')
            require(row['tick']<=send_tick<row['deadline'],'send observation window')
            if observed is None:issues.append('receipt_clock_unknown')
            elif observed<max(row['tick'],row['floor'],send_tick):issues.append('receipt_clock_regression');observed=None
            payload={'operation':op,'request_sha256':row['sha'],'status':receipt.status,'headers':headers,'body_base64':base64.b64encode(receipt.body).decode(),'observed':observed,'send_tick':send_tick}
            prior=db.execute('SELECT payload FROM receipts WHERE operation=?',(op,)).fetchone()
            path=self.root/f'receipt-{op}.json'
            existing=_loads(prior[0]) if prior else _loads(path.read_bytes()) if path.exists() else None
            if existing:
                require({k:v for k,v in existing.items() if k not in ('observed','send_tick')}=={k:v for k,v in payload.items() if k not in ('observed','send_tick')},'conflicting receipt');payload=existing
                if path.exists():require(_loads(path.read_bytes())==payload,'conflicting durable receipts')
                durable=True
            if not path.exists():
                try:_write(path,payload);durable=True
                except OSError:issues.append('artifact_persistence_failed')
            try:
                if not prior:
                    db.execute('INSERT INTO receipts VALUES(?,?)',(op,canonical(payload)))
                    db.execute("UPDATE operations SET state='received' WHERE id=? AND state IN ('claimed','uncertain')",(op,));self._event(db,op,'received',{'observed':payload['observed']})
                db.execute('UPDATE meta SET floor=max(floor,?)',(payload['send_tick'],))
                if payload['observed'] is not None:db.execute('UPDATE meta SET floor=max(floor,?)',(payload['observed'],))
                if issues:db.execute('UPDATE operations SET issue=? WHERE id=?',(canonical(issues),op))
                db.commit();durable=True
            except sqlite3.Error:
                db.rollback();issues.append('ledger_persistence_failed')
            require(durable,'both receipt persistence routes failed')
            return issues
        finally:db.close()
    def reconcile(self,op):
        with closing(_connect(self.path)) as db, db:
            db.execute('BEGIN IMMEDIATE');row=db.execute('SELECT * FROM operations WHERE id=?',(op,)).fetchone();require(row is not None,'operation missing')
            claim=db.execute('SELECT tick FROM claims WHERE operation=?',(op,)).fetchone();require(claim is not None,'reconciliation requires claim')
            record=db.execute('SELECT payload FROM receipts WHERE operation=?',(op,)).fetchone()
            if record:
                payload=_loads(record[0])
                independent=self.root/f'receipt-{op}.json'
                if independent.exists():require(_loads(independent.read_bytes())==payload,'conflicting durable receipts')
            else:
                path=self.root/f'receipt-{op}.json'
                if not path.exists():return
                payload=_loads(path.read_bytes())
            deadline=db.execute('SELECT deadline FROM meta').fetchone()[0]
            require(type(payload) is dict and set(payload)=={'operation','request_sha256','status','headers','body_base64','observed','send_tick'},'receipt fields')
            require(type(payload['operation']) is int and payload['operation']==op and payload['request_sha256']==row['sha'],'receipt binding')
            require(type(payload['send_tick']) is int and claim[0]<=payload['send_tick']<deadline,'receipt send clock window')
            require(payload['observed'] is None or (type(payload['observed']) is int and payload['observed']>=payload['send_tick']),'receipt observation window')
            if row['state']=='settled':return
            db.execute('UPDATE meta SET floor=max(floor,?)',(payload['send_tick'],))
            if payload['observed'] is not None:db.execute('UPDATE meta SET floor=max(floor,?)',(payload['observed'],))
            parsed=parse_response(payload['status'],base64.b64decode(payload['body_base64'],validate=True),expected_model=row['model'],case_id=row['case_id'])
            estimate,reason=_assess(parsed,OfflinePriceBound(**_loads(row['quote'])))
            state='settled' if estimate is not None else 'uncertain'
            db.execute('UPDATE operations SET state=?,estimated=?,parsed=? WHERE id=?',(state,estimate,canonical(parsed),op))
            if reason:db.execute('UPDATE operations SET issue=? WHERE id=?',(canonical([reason]),op))
            self._event(db,op,'reconciled',{'estimated':estimate,'reason':reason})
    def dispatch(self,op,transport,*,approval=None,clock_domain=None,clock=None):
        _approval(approval)
        if type(transport) is not FixtureTransport:raise PermissionError('only the fixed fake transport is implemented')
        self.reserve(op);request=self.claim(op,approval=approval,clock_domain=clock_domain,clock=clock)
        if request is None:return self.audit()
        try:
            with closing(_connect(self.path)) as db, db:
                m=db.execute('SELECT * FROM meta').fetchone()
                final=integer(clock(),'final clock')
                require(clock_domain==m['domain'] and m['floor']<=final<m['deadline'],'final clock window')
                db.execute('UPDATE meta SET floor=max(floor,?)',(final,));self._event(db,op,'send_boundary',{'tick':final})
            last=integer(clock(),'postcommit send clock')
            require(final<=last<m['deadline'],'postcommit clock window')
            receipt=transport.send(request)
            try:observed=integer(clock(),'receipt clock')
            except Exception:observed=None
            self.record_receipt(op,receipt,observed=observed,minimum_tick=last)
            self.reconcile(op)
        except Exception as error:
            with closing(_connect(self.path)) as db, db:
                db.execute("UPDATE operations SET state='uncertain',issue=? WHERE id=? AND state IN ('claimed','received')",(canonical([type(error).__name__]),op))
                self._event(db,op,'failed',{'exception_type':type(error).__name__})
        return self.audit()
    def audit(self):
        result={'version':'incident-hosted-fixture/1','mode':'offline_fixture','live_admissible':False,'live_blockers':['effective_account_pricing_unverified','exact_input_bound_unverified','remote_cancel_guarantee_unverified'],'new_model_calls':0,'monetary_unit':'synthetic_micro_usd','observed_provider_cost':None,'audit_complete':False,'estimated_spent':None,'held_bound':None,'claimed_count':0,'operations':[]}
        try:
            manifest=_loads((self.root/'manifest.json').read_bytes())
            require(manifest['version']==result['version'] and manifest['mode']=='offline_fixture' and manifest['live_admissible'] is False and type(manifest['new_model_calls']) is int and manifest['new_model_calls']==0 and manifest['monetary_unit']==result['monetary_unit'],'manifest identity')
            require(manifest['freeze_sha256']==FREEZE_SHA256,'frozen workload mismatch')
            require(manifest['source_sha256']=={name:hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in ('incident_hosted_controller.py','incident_adaption_backend.py','incident_routing.py')},'source binding mismatch')
            with closing(_connect(self.path,True)) as db, db:
                m=db.execute('SELECT * FROM meta').fetchone()
                require(tuple(m[k] for k in ('budget','domain','start','deadline','max_claims'))==tuple(manifest[k] for k in ('budget','clock_domain','start','deadline','max_claims')),'manifest/window mismatch')
                for key in ('budget','start','deadline','floor','max_claims'):integer(m[key],key)
                require(m['start']<m['deadline'] and m['floor']>=m['start'] and 1<=m['max_claims']<=4,'window invalid')
                spent=held=0;claims=0
                allowed={'planned','reserved','claimed','received','settled','uncertain','denied','cancelled'}
                for r in db.execute('SELECT * FROM operations ORDER BY id'):
                    require(r['state'] in allowed,'unknown operation state')
                    quote=OfflinePriceBound(**_loads(r['quote']));quote.validate()
                    rendered=render_case(r['case_id'],r['model'])
                    require(r['request']==rendered['request_bytes'] and r['sha']==rendered['request_sha256'] and _loads(r['provenance'])=={k:v for k,v in rendered.items() if k!='request_bytes'},'frozen request/provenance mismatch')
                    ev=[(e['event'],_loads(e['payload'])) for e in db.execute('SELECT event,payload FROM events WHERE operation=? ORDER BY id',(r['id'],))]
                    require(ev and ev[0]==('planned',{'request_sha256':r['sha']}),'planned event missing')
                    require(all(name in {'planned','reserved','denied','claimed','received','reconciled','cancelled','refused','failed','send_boundary'} and type(value) is dict for name,value in ev),'event shape')
                    names=[name for name,_ in ev]
                    require(names.count('planned')==1 and names.count('reserved')<=1 and names.count('denied')<=1 and not ('reserved' in names and 'denied' in names),'allocation event chain')
                    if r['state']=='planned':require(len(ev)==1,'planned state after allocation')
                    if 'claimed' in names:require(names.index('reserved')<names.index('claimed'),'claim precedes reserve')
                    if 'received' in names:require('claimed' in names and names.count('received')==1 and names.index('claimed')<names.index('received'),'receipt event chain')
                    require(names.count('send_boundary')<=1,'duplicate send boundary')
                    claim=db.execute('SELECT tick FROM claims WHERE operation=?',(r['id'],)).fetchone()
                    claim_events=[v for n,v in ev if n=='claimed']
                    require(len(claim_events)==(1 if claim else 0),'claim event mismatch')
                    if claim:
                        claims+=1;integer(claim[0],'claim clock')
                        require(m['start']<=claim[0]<m['deadline'] and claim[0]<=m['floor'] and claim_events==[{'tick':claim[0]}],'claim/window mismatch')
                        require(sum(n=='reserved' for n,_ in ev)==1 and r['state'] in {'claimed','received','settled','uncertain'},'claim/state mismatch')
                    else:require(r['state'] in {'planned','reserved','denied','cancelled'},'unclaimed execution state')
                    for name,value in ev:
                        if name=='send_boundary':require(claim is not None and set(value)=={'tick'} and type(value['tick']) is int and claim[0]<=value['tick']<=m['floor'] and value['tick']<m['deadline'],'send boundary clock')
                    reserve=[v for n,v in ev if n=='reserved'];require(len(reserve)<=1 and all(v=={'bound':quote.maximum} for v in reserve),'reservation event')
                    if r['state']=='reserved':require(len(reserve)==1,'reservation absent')
                    if r['state']=='denied':require([v for n,v in ev if n=='denied']==[{'bound':quote.maximum}],'denial absent')
                    if r['state']=='cancelled':require(sum(n=='cancelled' for n,_ in ev)==1,'cancellation absent')
                    record=db.execute('SELECT payload FROM receipts WHERE operation=?',(r['id'],)).fetchone()
                    require(sum(n=='received' for n,_ in ev)==(1 if record else 0),'receipt event missing')
                    payload=_loads(record[0]) if record else None
                    path=self.root/f"receipt-{r['id']}.json"
                    if path.exists():
                        independent=_loads(path.read_bytes());require(payload is None or payload==independent,'conflicting durable receipts');payload=independent
                    parsed=None;recomputed=None
                    if payload is not None:
                        require(type(payload) is dict and set(payload)=={'operation','request_sha256','status','headers','body_base64','observed','send_tick'} and type(payload['operation']) is int and payload['operation']==r['id'] and payload['request_sha256']==r['sha'] and claim is not None,'raw receipt binding')
                        if record:require([v for n,v in ev if n=='received']==[{'observed':payload['observed']}],'receipt event clock mismatch')
                        require(type(payload['send_tick']) is int and claim[0]<=payload['send_tick']<m['deadline'],'receipt send clock binding')
                        if record:require(payload['send_tick']<=m['floor'],'send floor regression')
                        observed=payload['observed']
                        require(observed is None or (type(observed) is int and observed>=payload['send_tick']),'receipt clock')
                        if record and observed is not None:require(observed<=m['floor'],'receipt floor regression')
                        require(type(payload['headers']) is list and all(type(pair) is list and len(pair)==2 and pair[0] in SAFE_HEADERS and type(pair[1]) is str for pair in payload['headers']),'safe receipt headers')
                        body=base64.b64decode(payload['body_base64'],validate=True)
                        parsed=parse_response(payload['status'],body,expected_model=r['model'],case_id=r['case_id'])
                        recomputed,_=_assess(parsed,quote)
                    if r['parsed'] is not None:require(parsed is not None and _loads(r['parsed'])==parsed,'stored parse differs from raw')
                    if r['state']=='settled':
                        require(r['parsed'] is not None and recomputed is not None and r['estimated']==recomputed,'settled estimate lacks raw evidence')
                        integer(r['estimated'],'estimate');spent+=r['estimated']
                    else:require(r['estimated'] is None,'unsettled estimated spend')
                    if r['state'] in HOLD_STATES:held+=quote.maximum
                    if r['state']=='received':require(record is not None,'received state missing receipt')
                    late=payload['observed']>=m['deadline'] if payload and payload['observed'] is not None else None
                    result['operations'].append({'id':r['id'],'state':r['state'],'request_sha256':r['sha'],'estimated':r['estimated'],'parsed':parsed,'recomputed_estimate':recomputed,'issue':_loads(r['issue']) if r['issue'] else None,'late':late,'raw_receipt_available':payload is not None})
                require(claims==db.execute('SELECT count(*) FROM claims').fetchone()[0]<=m['max_claims'],'call-count bound')
                require(spent+held<=m['budget'],'accounting invariant')
                result.update(estimated_spent=spent,held_bound=held,claimed_count=claims,audit_complete=True,max_claims=m['max_claims'])
        except (ValueError,TypeError,KeyError,OSError,sqlite3.Error,IndexError,OverflowError,RecursionError) as error:
            result.update(audit_complete=False,estimated_spent=None,held_bound=None,audit_error=str(error),operations=[])
        return result


def main(argv=None):
    p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='command',required=True)
    fixture=sub.add_parser('fixture');fixture.add_argument('--out',type=Path,required=True)
    audit=sub.add_parser('audit');audit.add_argument('out',type=Path)
    a=p.parse_args(argv)
    if a.command=='audit':report=HostedLedger(a.out).audit()
    else:
        ledger=HostedLedger.create(a.out,budget=400,clock_domain='fixture-clock',start=0,deadline=1000)
        op=ledger.create_operation('fixture-case','cal-01','fixture-hosted-model/1',OfflinePriceBound(400,2,3,10,20))
        body=canonical({'id':'fixture-response','object':'response','model':'fixture-hosted-model/1','status':'completed','output':[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'{}'}]}],'output_text':'{}','usage':{'input_tokens':10,'output_tokens':5,'total_tokens':15}}).encode()
        report=ledger.dispatch(op,FixtureTransport(HttpReceipt(200,(),body)),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
    print(canonical(report));return 0 if report['audit_complete'] else 1

if __name__=='__main__':
    try:raise SystemExit(main())
    except Exception as error:raise SystemExit(f'Offline hosted controller refused: {type(error).__name__}: {error}')
