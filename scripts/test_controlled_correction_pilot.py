"""Fault-focused checks: removing claim limits/durability/approval must fail these."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import sys
SPEC=importlib.util.spec_from_file_location('pilot_runner',Path(__file__).with_name('run_controlled_correction_pilot.py'))
pilot=importlib.util.module_from_spec(SPEC);sys.modules[SPEC.name]=pilot;SPEC.loader.exec_module(pilot)

class PilotAccountingTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.out=Path(self.tmp.name)
    def tearDown(self):self.tmp.cleanup()
    def response(self,**changes):
        value={'model':'ornith-1.5:9b','done':True,'done_reason':'stop','message':{'role':'assistant','content':'{}'},'prompt_eval_count':181,'eval_count':30,'total_duration':11,'load_duration':0,'prompt_eval_duration':5,'eval_duration':6}
        value.update(changes);return json.dumps(value).encode()
    def test_unapproved_invocation_denied_before_artifacts_or_runtime(self):
        with self.assertRaises(PermissionError):pilot.run_live(self.out/'absent.json',self.out/'run')
        self.assertFalse((self.out/'run').exists())
    def test_fourteenth_claim_cannot_be_sent(self):
        ledger=pilot.Ledger(self.out,deadline=1000,clock=lambda:0)
        for _ in range(13):
            n=ledger.claim('{}');ledger.receive(n,self.response())
        with self.assertRaises(ValueError):ledger.claim('{}')
        audit=pilot.audit(self.out)
        self.assertEqual(audit['claims'],13);self.assertEqual(audit['known_tokens'],2743)
    def test_uncertain_reply_retained_and_blocks_further_claims(self):
        ledger=pilot.Ledger(self.out,deadline=1000,clock=lambda:0)
        n=ledger.claim('{}')
        with self.assertRaises(ValueError):ledger.receive(n,b'{bad')
        self.assertEqual((self.out/'raw-01.bin').read_bytes(),b'{bad')
        with self.assertRaises(ValueError):ledger.claim('{}')
        result=pilot.audit(self.out)
        self.assertEqual(result['unresolved_claims'],1);self.assertIsNone(result['hard_token_liability_bound'])
        self.assertEqual(result['unresolved_quote_units'],2144)
    def test_observed_excess_usage_is_not_clamped_or_dropped(self):
        ledger=pilot.Ledger(self.out,deadline=1000,clock=lambda:0)
        n=ledger.claim('{}')
        with self.assertRaises(ValueError):ledger.receive(n,self.response(prompt_eval_count=3000))
        with self.assertRaises(ValueError):ledger.claim('{}')
        self.assertEqual(pilot.audit(self.out)['known_tokens'],3030)
    def test_deadline_preserves_original_cutoff_and_sends_nothing(self):
        ledger=pilot.Ledger(self.out,deadline=100,clock=lambda:40)
        with self.assertRaises(TimeoutError):ledger.claim('{}')
        self.assertEqual(pilot.audit(self.out)['claims'],0)
    def test_duplicate_json_keys_and_boolean_usage_remain_unknown(self):
        for name,body in [('duplicate',b'{"done":true,"done":false}'),('boolean',self.response(eval_count=True))]:
            target=self.out/name;target.mkdir();ledger=pilot.Ledger(target,deadline=1000,clock=lambda:0)
            n=ledger.claim('{}')
            with self.assertRaises(ValueError):ledger.receive(n,body)
            self.assertEqual(pilot.audit(target)['unresolved_claims'],1)
    def test_failed_claim_persistence_cannot_authorize_send(self):
        (self.out/'claim-01.json').mkdir()
        ledger=pilot.Ledger(self.out,deadline=1000,clock=lambda:0)
        with self.assertRaises(OSError):ledger.claim('{}')
        self.assertEqual(ledger.claims,0)


class AdditionalFailureTests(unittest.TestCase):
    def test_nonfinal_usage_cannot_release_unknown_liability(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);ledger=pilot.Ledger(out,deadline=1000,clock=lambda:0)
            ordinal=ledger.claim('{}')
            raw=json.dumps({'model':'ornith-1.5:9b','done':False,'prompt_eval_count':181,'eval_count':30}).encode()
            with self.assertRaises(ValueError):ledger.receive(ordinal,raw)
            result=pilot.audit(out)
            self.assertEqual(result['known_tokens'],211)
            self.assertEqual(result['unresolved_claims'],1,'partial measured usage cannot reconcile final liability')

class WatchdogTests(unittest.TestCase):
    def check_owned_child_terminated(self, close_writer):
        import os,signal,subprocess
        child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
        r,w=os.pipe()
        try:
            if close_writer:os.close(w);w=None
            result=pilot.watch_pipe(r,pilot.clock()+0.03,lambda:os.killpg(child.pid,signal.SIGKILL))
            self.assertEqual(result,'parent_lost' if close_writer else 'deadline')
            self.assertIsNotNone(child.poll() if child.poll() is not None else child.wait(timeout=1))
        finally:
            if child.poll() is None:os.killpg(child.pid,signal.SIGKILL)
            child.wait();os.close(r)
            if w is not None:os.close(w)
    def test_eof_kills_only_owned_worker(self):self.check_owned_child_terminated(True)
    def test_absolute_deadline_kills_owned_worker(self):self.check_owned_child_terminated(False)

class RequestContractTests(unittest.TestCase):
    def request(self):
        import re
        source=(pilot.ROOT/'examples/controlled_correction_pilot.rs').read_text()
        prompt=re.search(r'const REPORT_PROMPT:\s*&str\s*=\s*r#"(.*?)"#;',source,re.S).group(1)
        request={**pilot.ENVELOPE,'messages':[{'role':'system','content':prompt},{'role':'user','content':'{"ci_report":{"failing_cases":0,"release":"R42","test_summary":"12/12","version":"run/17"}}'}]}
        return {'op':'generate','ordinal':1,'model_digest':pilot.MODEL_DIGEST,'request_utf8':json.dumps(request)}
    def test_original_prompt_parity_and_changed_caps_are_refused(self):
        import re
        original=(pilot.ROOT/'examples/scoped_model_reuse.rs').read_text()
        request=self.request();value=json.loads(request['request_utf8'])
        current=(pilot.ROOT/'examples/controlled_correction_pilot.rs').read_text()
        for name in ['REPORT_PROMPT','PLAN_PROMPT']:
            pattern=r'const '+name+r':\s*&str\s*=\s*r#"(.*?)"#;'
            self.assertEqual(re.search(pattern,current,re.S).group(1),re.search(pattern,original,re.S).group(1))
        expected=re.search(r'const REPORT_PROMPT:\s*&str\s*=\s*r#"(.*?)"#;',original,re.S).group(1)
        self.assertEqual(value['messages'][0]['content'],expected)
        pilot.validate_request(request,1)
        value['options']={**pilot.OPTIONS,'num_predict':97};request['request_utf8']=json.dumps(value)
        with self.assertRaises(ValueError):pilot.validate_request(request,1)
    def test_cpu_observation_rejects_gpu_or_wrong_runtime_context(self):
        for model in [dict(name=pilot.MODEL,digest=pilot.MODEL_DIGEST,size_vram=1,context_length=2048),dict(name=pilot.MODEL,digest=pilot.MODEL_DIGEST,size_vram=0,context_length=4096)]:
            with self.assertRaises(ValueError):pilot.validate_cpu_observation({'models':[model]})
        pilot.validate_cpu_observation({'models':[dict(name=pilot.MODEL,digest=pilot.MODEL_DIGEST,size_vram=0,context_length=2048)]})

class CompiledBridgeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import os
        cls.binary=os.environ.get('KAIROTENSOR_PILOT_TEST_BINARY')
        if not cls.binary:raise unittest.SkipTest('compiled bridge checked by explicit offline qualification command')
    def response(self,raw,timeout):
        request=json.loads(raw);data=json.loads(request['messages'][1]['content'])
        if 'ci_report' in data:
            r=data['ci_report'];answer=dict(release=r['release'],tests=r['test_summary'],status='pass' if r['failing_cases']==0 else 'fail')
        else:
            r=data['requirement'];v=data['test_certificate'];answer=dict(release=r['release'],target=r['target'],approval=r['approval'],tests=v['tests'],action='deploy' if v['status']=='pass' else 'hold')
        return json.dumps({'model':pilot.MODEL,'done':True,'done_reason':'stop','message':{'role':'assistant','content':json.dumps(answer)},'prompt_eval_count':181,'eval_count':30}).encode()
    def test_real_binary_ipc_runs_thirteen_synthetic_deliveries(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);deadline=pilot.clock()+120;ledger=pilot.Ledger(out,deadline=deadline)
            result=pilot.drive(self.binary,out,ledger,deadline,self.response)
            self.assertEqual(result['dispatches'],13);self.assertEqual(result['reuse_hits'],2)
            self.assertEqual(pilot.audit(out)['known_tokens'],2743)
            self.assertEqual(pilot.schedule_outcomes(out,13)['unexecuted_slots'],[])
    def test_transport_eof_keeps_one_claim_and_twelve_unexecuted_slots(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);deadline=pilot.clock()+120;ledger=pilot.Ledger(out,deadline=deadline)
            def broken(*_):raise ConnectionError('synthetic transport EOF')
            with self.assertRaises(ConnectionError):pilot.drive(self.binary,out,ledger,deadline,broken)
            self.assertEqual(pilot.audit(out)['unresolved_claims'],1)
            self.assertEqual(len(pilot.schedule_outcomes(out,1)['unexecuted_slots']),12)
            self.assertEqual(set(pilot.schedule_outcomes(out,1)['scenario_outcomes'].values()),{'unexecuted'})
    def test_invalid_model_verdict_charged_without_further_calls(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);deadline=pilot.clock()+120;ledger=pilot.Ledger(out,deadline=deadline)
            def wrong(raw,timeout):
                response=json.loads(self.response(raw,timeout));response['message']['content']='{"release":"R42","tests":"12/12","status":"fail"}';return json.dumps(response).encode()
            with self.assertRaises(ValueError):pilot.drive(self.binary,out,ledger,deadline,wrong)
            audit=pilot.audit(out);self.assertEqual(audit['claims'],1);self.assertEqual(audit['known_tokens'],211)
            result=json.loads((out/'store/result.json').read_text());self.assertFalse(result['complete']);self.assertEqual(len(result['unexecuted_slots']),12)


class HttpDeadlineTests(unittest.TestCase):
    def check_transport(self, drip):
        import http.server,threading,time
        from unittest.mock import patch
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get('Content-Length','0')))
                self.send_response(200);self.send_header('Content-Length',str(len(b'partial-prefix')+(30 if drip else 86)));self.end_headers()
                try:
                    self.wfile.write(b'partial-prefix');self.wfile.flush()
                    if drip:
                        for _ in range(30):self.wfile.write(b'x');self.wfile.flush();time.sleep(0.04)
                    else:time.sleep(1)
                except (BrokenPipeError,ConnectionResetError):pass
            def log_message(self,*args):pass
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        server.daemon_threads=True
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as d,patch.object(pilot,'SERVER',f'http://127.0.0.1:{server.server_port}'):
                raw=Path(d)/'raw-01.bin';started=time.monotonic()
                with self.assertRaises((TimeoutError,pilot.HttpFailure)):
                    pilot.http('/api/chat',b'{}',timeout=0.25,raw_path=raw)
                self.assertLess(time.monotonic()-started,0.8,'deadline must bound total response time, not each socket read')
                self.assertTrue(raw.exists(),'received prefix must survive timeout')
                self.assertTrue(raw.read_bytes().startswith(b'partial-prefix'))
        finally:server.shutdown();server.server_close();thread.join()
    def test_slow_drip_has_absolute_deadline(self):self.check_transport(True)
    def test_partial_stall_bytes_survive_timeout(self):self.check_transport(False)


class LateContainerTests(unittest.TestCase):
    def test_parent_loss_reconciles_late_creation_or_reports_unresolved(self):
        import os,time,types
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);fake=out/'fake-podman';name='kt-correction-'+'a'*32
            born=time.monotonic()
            fake.write_text('#!'+sys.executable+'\n'+f'''import sys,time,json,pathlib
root=pathlib.Path({d!r})
if sys.argv[1]=='inspect':
 if time.monotonic() < {born+0.75!r}:sys.exit(1)
 print(json.dumps([{{'Name':{name!r},'Image':{pilot.IMAGE_ID!r},'Id':'b'*64,'State':{{'Running':not (root/'killed').exists()}}}}]))
elif sys.argv[1]=='kill':(root/'killed').write_text('yes')
''');fake.chmod(0o700)
            r,w=os.pipe();os.close(w)
            try:
                with os.fdopen(r,'rb') as reader,patch.object(pilot.sys,'stdin',reader):
                    code=pilot.watchdog_main(types.SimpleNamespace(name=name,deadline=pilot.clock()+1.5,out=str(out),podman=str(fake)))
                self.assertTrue((out/'killed').exists(),'watcher must not report success before delayed creation is reconciled')
                self.assertEqual(code,0)
                self.assertTrue(json.loads((out/'watchdog.json').read_text())['cleanup_verified'])
            finally:pass
    def test_absence_without_a_sealed_startup_is_not_cleanup_success(self):
        import os,types
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);fake=out/'fake-podman';fake.write_text('#!'+sys.executable+'\nimport sys;sys.exit(1)\n');fake.chmod(0o700)
            r,w=os.pipe();os.close(w)
            with os.fdopen(r,'rb') as reader,patch.object(pilot.sys,'stdin',reader):
                code=pilot.watchdog_main(types.SimpleNamespace(name='kt-correction-'+'a'*32,deadline=pilot.clock()+0.2,out=str(out),podman=str(fake)))
            self.assertNotEqual(code,0,'unobserved cleanup must not be reported successful')
            self.assertFalse(json.loads((out/'watchdog.json').read_text())['cleanup_verified'])


class ParentBindingTests(unittest.TestCase):
    def test_wrong_expected_parent_cannot_execute_child(self):
        import functools,os,subprocess
        process=subprocess.Popen([sys.executable,'-c','raise SystemExit(0)'],
            preexec_fn=functools.partial(pilot.child_setup,os.getpid()+1000000))
        self.assertEqual(process.wait(timeout=2),127)
    def test_uncertain_complete_json_keeps_observed_usage_and_hold(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);ledger=pilot.Ledger(out,deadline=1000,clock=lambda:0)
            ordinal=ledger.claim('{}')
            raw=json.dumps({'model':pilot.MODEL,'done':True,'done_reason':'stop',
                'message':{'role':'assistant','content':'{}'},'prompt_eval_count':181,'eval_count':30}).encode()
            (out/'raw-01.bin').write_bytes(raw)
            ledger.observe_uncertain(ordinal)
            result=pilot.audit(out)
            self.assertEqual(result['known_tokens'],211);self.assertEqual(result['unresolved_claims'],1)
            with self.assertRaises(ValueError):ledger.claim('{}')

if __name__=='__main__':unittest.main()
