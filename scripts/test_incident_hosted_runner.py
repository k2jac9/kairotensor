import hashlib
import multiprocessing
import os
import signal
import subprocess
import sys
import time
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from incident_hosted_runner import run_fixture, run_live, audit_run, LiveApproval
from incident_adaption_backend import render_case

class RunnerTests(unittest.TestCase):
    def setUp(self):self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name)/'run'
    def execute(self,root=None,*,fault='none'):
        # Protocol/audit assertions must reach their intended path despite slow
        # durable setup. Explicit clock-boundary tests below cover expiry.
        return run_fixture(self.root if root is None else root,duration_ms=30000,fault=fault)
    def test_live_default_deny_before_credentials_or_network(self):
        with patch('incident_hosted_runner.os.environ',{}),patch('incident_hosted_runner.send_https',side_effect=AssertionError('network')):
            with self.assertRaises(PermissionError):run_live(self.root,'cal-01','some-model',budget_micro_usd=1000)
        self.assertFalse(self.root.exists())
    def test_wrong_request_and_missing_billing_attestation_denied(self):
        rendered=render_case('cal-01','some-model')
        for sha,bounded in [('wrong',True),(rendered['request_sha256'],False)]:
            approval=LiveApproval(sha,'some-model',100,'approval','price',True,True,bounded)
            with self.subTest(sha=sha,bounded=bounded),self.assertRaises(ValueError):
                run_live(self.root,'cal-01','some-model',approval=approval,budget_micro_usd=100)
        self.assertFalse(self.root.exists())
    def test_fixture_complete_usage_but_full_hold_not_invoice(self):
        result=self.execute()
        self.assertTrue(result['audit_complete']);self.assertTrue(result['receipt_complete'])
        self.assertEqual(result['claimed_count'],1);self.assertEqual(result['new_model_calls'],0)
        self.assertEqual(result['held_micro_usd'],400);self.assertIsNone(result['verified_provider_cost'])
        self.assertEqual(result['provider_reported_usage']['total_tokens'],15)
        self.assertEqual(result['monetary_basis'],'synthetic_fixture_bound')
    def test_rerun_cannot_resend_and_audit_does_not_mutate(self):
        self.execute()
        before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in self.root.iterdir()}
        with self.assertRaises(FileExistsError):self.execute()
        self.assertTrue(audit_run(self.root)['audit_complete'])
        after={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in self.root.iterdir()}
        self.assertEqual(before,after)
    def test_crash_and_partial_keep_uncertain_hold(self):
        for fault,exitcode,raw_bytes in (('crash',7,0),('partial',8,50)):
            with self.subTest(fault=fault):
                result=self.execute(self.root.with_name(fault),fault=fault)
                self.assertTrue(result['audit_complete'],result);self.assertEqual(result['claimed_count'],1,result)
                self.assertEqual(result['held_micro_usd'],400,result);self.assertEqual(result['new_model_calls'],0,result)
                self.assertFalse(result['receipt_complete'],result);self.assertEqual(result['state'],'delivery_uncertain',result)
                self.assertEqual(result['raw_body_bytes'],raw_bytes,result)
                self.assertEqual(result['parent']['child_exitcode'],exitcode,result)
                self.assertEqual(result['parent']['reason'],'child_exit_without_complete_receipt',result)
    def test_hanging_child_is_reaped_after_watchdog_expiry_with_hold(self):
        import incident_hosted_runner as runner
        hanging=multiprocessing.get_context('fork').Event()
        parent_pid=os.getpid();original_sleep=time.sleep;original_clock=runner._now
        expired_at=None;observed_at=None
        def observe_hang(seconds):
            # This forked fixture's only sleep is its real hanging send path.
            if os.getpid()!=parent_pid:hanging.set()
            return original_sleep(seconds)
        def expire_when_hanging():
            nonlocal expired_at,observed_at
            if os.getpid()==parent_pid and hanging.is_set():
                if expired_at is None:
                    observed_at=time.monotonic()
                    expired_at=json.loads((self.root/'manifest.json').read_text())['deadline_ns']
                return expired_at
            return original_clock()
        with patch.object(runner.time,'sleep',observe_hang),patch.object(runner,'_now',expire_when_hanging):
            result=self.execute(fault='hang')
        self.assertTrue(hanging.is_set(),result);self.assertIsNotNone(observed_at,result)
        self.assertLess(time.monotonic()-observed_at,5,result)
        self.assertTrue(result['audit_complete'],result);self.assertEqual(result['claimed_count'],1,result)
        self.assertEqual(result['held_micro_usd'],400,result);self.assertEqual(result['new_model_calls'],0,result)
        self.assertFalse(result['receipt_complete'],result);self.assertEqual(result['raw_body_bytes'],0,result)
        self.assertEqual(result['state'],'delivery_uncertain',result)
        self.assertEqual(result['parent']['reason'],'local_watchdog_expired_remote_outcome_unknown',result)
        self.assertEqual(result['parent']['child_exitcode'],-signal.SIGKILL,result)
    def test_deadline_before_and_after_claim_never_forks(self):
        import sqlite3
        from contextlib import closing
        import incident_hosted_runner as runner
        for boundary,claims,hold,state in (('before',0,0,'not_sent'),('after',1,400,'delivery_uncertain')):
            root=self.root.with_name(boundary)
            def clock():
                expire=(root/'request.json').exists() if boundary=='before' else False
                if boundary=='after' and (root/'ledger.sqlite').exists():
                    with closing(sqlite3.connect((root/'ledger.sqlite').as_uri()+'?mode=ro',uri=True)) as db:
                        expire=db.execute('SELECT count(*) FROM claim').fetchone()[0]==1
                return json.loads((root/'manifest.json').read_text())['deadline_ns'] if expire else 10_000_000
            with self.subTest(boundary=boundary),patch.object(runner,'_now',clock), \
                 patch.object(multiprocessing.process.BaseProcess,'start',side_effect=AssertionError('expired request forked')):
                result=run_fixture(root,duration_ms=200)
                self.assertTrue(result['audit_complete'],result);self.assertEqual(result['claimed_count'],claims,result)
                self.assertEqual(result['held_micro_usd'],hold,result);self.assertEqual(result['state'],state,result)
                self.assertFalse(result['receipt_complete'],result);self.assertEqual(result['raw_body_bytes'],0,result)
                self.assertEqual(result['new_model_calls'],0,result)
                self.assertEqual(result['parent']['reason'],'ValueError',result)
                self.assertIsNone(result['parent']['child_exitcode'],result)
                self.assertFalse(any(root.glob('transport-*.json')))
    def test_raw_request_and_manifest_tampering_refused(self):
        self.execute()
        request=self.root/'request.json';request.write_text('{"body_base64":"e30="}')
        self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_incomplete_temporary_record_does_not_destroy_committed_prefix(self):
        self.execute()
        (self.root/'transport-00003.json.tmp').write_bytes(b'{partial')
        result=audit_run(self.root)
        self.assertTrue(result['audit_complete']);self.assertEqual(result['provider_reported_usage']['total_tokens'],15)
    def test_transport_sequence_gap_refused(self):
        self.execute()
        (self.root/'transport-00001.json').unlink()
        self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_audit_strict_integer_budget(self):
        self.execute()
        path=self.root/'manifest.json';original=json.loads(path.read_text())
        for field in ('maximum_calls','budget_micro_usd','reserved_micro_usd'):
            value=dict(original);value[field]=True;path.write_text(json.dumps(value))
            with self.subTest(field=field):self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_rejected_wal_and_journal_never_mutate_source(self):
        self.execute()
        ledger=self.root/'ledger.sqlite'
        for suffix in ('-wal','-shm','-journal'):
            sidecar=Path(str(ledger)+suffix);sidecar.write_bytes(b'untrusted sidecar')
            before={p.name:p.read_bytes() for p in self.root.iterdir()}
            self.assertFalse(audit_run(self.root)['audit_complete'])
            self.assertEqual(before,{p.name:p.read_bytes() for p in self.root.iterdir()})
            sidecar.unlink()
        raw=bytearray(ledger.read_bytes());raw[18:20]=b'\x02\x02';ledger.write_bytes(raw)
        before={p.name:p.read_bytes() for p in self.root.iterdir()}
        self.assertFalse(audit_run(self.root)['audit_complete'])
        self.assertEqual(before,{p.name:p.read_bytes() for p in self.root.iterdir()})
    def test_supervisor_death_kills_fork_child(self):
        script=Path(__file__).resolve().parent
        code="import sys;sys.path.insert(0,sys.argv[1]);from incident_hosted_runner import run_fixture;run_fixture(sys.argv[2],duration_ms=10000,fault='hang')"
        process=subprocess.Popen([sys.executable,'-c',code,str(script),str(self.root)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        child=None
        def proc_for(pid):
            for path in Path('/proc').glob('[0-9]*'):
                try:
                    lines=path.joinpath('status').read_text().splitlines()
                    found=int(next(v.split()[-1] for v in lines if v.startswith('NSpid:')))
                    if found==pid:return path
                except (OSError,StopIteration,ValueError):pass
            return None
        def running(pid):
            path=proc_for(pid)
            if path is None:return False
            try:return path.joinpath('stat').read_text().split()[2] not in ('Z','X')
            except (FileNotFoundError,ProcessLookupError):return False
        try:
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                for path in Path('/proc').glob('[0-9]*'):
                    try:
                        lines=path.joinpath('status').read_text().splitlines()
                        parent=int(next(v.split()[-1] for v in lines if v.startswith('PPid:')))
                        if parent==process.pid:
                            child=int(next(v.split()[-1] for v in lines if v.startswith('NSpid:')));break
                    except (OSError,StopIteration,ValueError):pass
                if child is not None:break
                time.sleep(.01)
            self.assertIsNotNone(child)
            process.kill();process.wait(timeout=2)
            deadline=time.monotonic()+3
            while running(child) and time.monotonic()<deadline:time.sleep(.01)
            self.assertFalse(running(child))
            result=audit_run(self.root)
            self.assertTrue(result['audit_complete']);self.assertEqual(result['held_micro_usd'],400)
        finally:
            if process.poll() is None:process.kill();process.wait(timeout=2)
            if child is not None and running(child):
                try:os.kill(child,9)
                except ProcessLookupError:pass
    def test_clock_and_budget_limits(self):
        for duration in (True,0,99,1200001):
            with self.subTest(duration=duration),self.assertRaises(ValueError):run_fixture(self.root,duration_ms=duration)

    def test_parent_interrupt_reaps_child_before_propagating_and_retains_hold(self):
        # Removing cleanup on BaseException would leave a real hanging child
        # running after a caller catches the parent's interrupt.
        original_join = multiprocessing.process.BaseProcess.join
        for exception in (KeyboardInterrupt, SystemExit):
            path = self.root.with_name(exception.__name__)
            children = []
            interrupted = False

            def interrupt_first_join(process, timeout=None):
                nonlocal interrupted
                if not interrupted:
                    interrupted = True
                    children.append(process)
                    if exception is KeyboardInterrupt:
                        os.kill(os.getpid(), signal.SIGINT)
                    else:
                        raise SystemExit(9)
                return original_join(process, timeout)

            try:
                with self.subTest(exception=exception.__name__):
                    with patch.object(multiprocessing.process.BaseProcess, 'join', interrupt_first_join):
                        with self.assertRaises(exception):
                            run_fixture(path, duration_ms=10000, fault='hang')
                    self.assertEqual(len(children), 1)
                    self.assertFalse(children[0].is_alive())
                    self.assertIsNotNone(children[0].exitcode)
                    result = audit_run(path)
                    self.assertTrue(result['audit_complete'])
                    self.assertEqual(result['held_micro_usd'], 400)
                    self.assertEqual(result['claimed_count'], 1)
                    self.assertEqual(result['state'], 'delivery_uncertain')
                    with self.assertRaises(FileExistsError):
                        run_fixture(path)
            finally:
                for child in children:
                    if child.is_alive():
                        child.kill()
                    original_join(child, 2)

if __name__=='__main__':unittest.main()
