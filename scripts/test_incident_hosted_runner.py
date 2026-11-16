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
        result=run_fixture(self.root)
        self.assertTrue(result['audit_complete']);self.assertTrue(result['receipt_complete'])
        self.assertEqual(result['claimed_count'],1);self.assertEqual(result['new_model_calls'],0)
        self.assertEqual(result['held_micro_usd'],400);self.assertIsNone(result['verified_provider_cost'])
        self.assertEqual(result['provider_reported_usage']['total_tokens'],15)
        self.assertEqual(result['monetary_basis'],'synthetic_fixture_bound')
    def test_rerun_cannot_resend_and_audit_does_not_mutate(self):
        run_fixture(self.root)
        before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in self.root.iterdir()}
        with self.assertRaises(FileExistsError):run_fixture(self.root)
        self.assertTrue(audit_run(self.root)['audit_complete'])
        after={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in self.root.iterdir()}
        self.assertEqual(before,after)
    def test_crash_hang_partial_keep_uncertain_hold(self):
        for fault in ('crash','hang','partial'):
            path=self.root.with_name(fault)
            result=run_fixture(path,duration_ms=200,fault=fault)
            with self.subTest(fault=fault):
                self.assertTrue(result['audit_complete']);self.assertEqual(result['held_micro_usd'],400)
                self.assertFalse(result['receipt_complete']);self.assertEqual(result['state'],'delivery_uncertain')
                if fault=='partial':self.assertGreater(result['raw_body_bytes'],0)
    def test_raw_request_and_manifest_tampering_refused(self):
        run_fixture(self.root)
        request=self.root/'request.json';request.write_text('{"body_base64":"e30="}')
        self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_incomplete_temporary_record_does_not_destroy_committed_prefix(self):
        run_fixture(self.root)
        (self.root/'transport-00003.json.tmp').write_bytes(b'{partial')
        result=audit_run(self.root)
        self.assertTrue(result['audit_complete']);self.assertEqual(result['provider_reported_usage']['total_tokens'],15)
    def test_transport_sequence_gap_refused(self):
        run_fixture(self.root)
        (self.root/'transport-00001.json').unlink()
        self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_audit_strict_integer_budget(self):
        run_fixture(self.root)
        path=self.root/'manifest.json';original=json.loads(path.read_text())
        for field in ('maximum_calls','budget_micro_usd','reserved_micro_usd'):
            value=dict(original);value[field]=True;path.write_text(json.dumps(value))
            with self.subTest(field=field):self.assertFalse(audit_run(self.root)['audit_complete'])
    def test_rejected_wal_and_journal_never_mutate_source(self):
        run_fixture(self.root)
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
