import copy
import json
import sqlite3
import tempfile
import time
import unittest
from pathlib import Path
from incident_preflight import calibration_plan
from incident_calibration_controller import ExecutionApproval, run_fixture, audit_run

class ControllerTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.out=Path(self.tmp.name)/'run'
    def run(self, result=None):
        return super().run(result)
    def execute(self,fault='none',budget=30000):
        return run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=budget,audit_reserve_ms=300,fault=fault)
    def diagnostics(self,report):
        details={'report':report}
        for name in ('supervisor.json','controller-stderr.log'):
            path=self.out/name
            details[name]=path.read_text() if path.exists() else '<not created>'
        return details
    def test_default_and_live_denied_before_files(self):
        for approval in [None,ExecutionApproval(),ExecutionApproval('live')]:
            with self.subTest(approval=approval):
                with self.assertRaises(PermissionError):run_fixture(self.out,approval=approval)
                self.assertFalse(self.out.exists())
    def test_complete_eight_probe_ledger_and_no_replay(self):
        report=self.execute();self.assertTrue(report['complete']);self.assertEqual(report['claimed_slots'],8)
        self.assertEqual(report['settled_slots'],8);self.assertEqual(report['ledger_charged_units'],1592)
        self.assertEqual(report['unresolved_hold_units'],0);self.assertEqual(report['new_model_calls'],0)
        with self.assertRaises(FileExistsError):self.execute()
        self.assertEqual(audit_run(self.out)['ledger_charged_units'],1592)
        db=sqlite3.connect(self.out/'ledger.sqlite')
        rows=db.execute('select request_sha256, input_units, output_cap, state from slots order by slot').fetchall()
        self.assertEqual([r[1] for r in rows],[173]*4+[211]*4)
        self.assertTrue(all(r[2:]==(96,'settled') for r in rows));db.close()
    def test_startup_deadline_expiry_does_not_invent_zero_accounting(self):
        from unittest.mock import patch
        import incident_calibration_controller as controller
        original_launch=controller.subprocess.Popen
        original_clock=controller.now_ns
        # Keep parent setup on a deterministic injected pre-expiry tick. At
        # launch, advance to the real clock; the fixed child also observes the
        # already-expired original window and cannot initialize/claim work.
        setup_tick=original_clock(controller._START_MODE)-1_000_000_000
        launched=False
        def launch_after_expiry(*args,**kwargs):
            nonlocal launched
            launched=True
            return original_launch(*args,**kwargs)
        def clock(mode):
            return original_clock(mode) if launched else setup_tick
        with patch('incident_calibration_controller.subprocess.Popen',launch_after_expiry), patch('incident_calibration_controller.now_ns',clock):
            report=run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=300,audit_reserve_ms=100,fault='wrong_quote')
        self.assertFalse(report['complete'],report);self.assertFalse(report['audit_complete'],report)
        self.assertIsNone(report['ledger_charged_units'],report)
        self.assertIsNone(report['unresolved_hold_units'],report)
        seal=json.loads((self.out/'supervisor.json').read_text())
        self.assertTrue(seal['deadline_overrun']);self.assertTrue(seal['child_reaped'])
        self.assertFalse(any(self.out.glob('receipt-*.json')))

    def test_plan_changes_refused_before_execution(self):
        for change in [lambda p:p.update(max_generation_calls=9),lambda p:p['schedule'][0].update(request_utf8='{}'),lambda p:p.update(max_total_quoted_tokens=99999)]:
            with self.subTest(change=change):
                p=calibration_plan();change(p)
                with self.assertRaises(ValueError):run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),plan=p)
                self.assertFalse(self.out.exists())
    def test_crashes_before_and_after_claim_keep_conservative_holds(self):
        for fault,state,claimed in [('after_reserve','reserved',0),('after_claim','claimed',1)]:
            with self.subTest(fault=fault):
                self.out=Path(self.tmp.name)/fault;report=self.execute(fault)
                with self.subTest(**self.diagnostics(report)):
                    self.assertFalse(report['complete']);self.assertEqual(report['claimed_slots'],claimed)
                    self.assertEqual(report['unresolved_hold_units'],269);self.assertEqual(report['ledger_charged_units'],0)
                    self.assertEqual(report['slots'][0]['state'],state)
                    with self.assertRaises(FileExistsError):self.execute()
    def test_external_watchdog_bounds_child_and_generation_hangs(self):
        for fault in ['hang_before_worker','hang_generation','partial_line']:
            with self.subTest(fault=fault):
                self.out=Path(self.tmp.name)/fault;start=time.monotonic();report=self.execute(fault,1500)
                with self.subTest(**self.diagnostics(report)):
                    self.assertLess(time.monotonic()-start,5);self.assertFalse(report['complete'])
                    if fault=='hang_generation':self.assertEqual(report['unresolved_hold_units'],269)
                    self.assertTrue(report['supervisor']['child_reaped'])
    def test_receipt_survives_crash_and_sqlite_failures_without_replay(self):
        for fault in ['after_receipt','hang_after_receipt','sqlite_receipt_failure','sqlite_settle_failure']:
            with self.subTest(fault=fault):
                self.out=Path(self.tmp.name)/fault
                report=self.execute(fault,1500 if fault=='hang_after_receipt' else 30000)
                with self.subTest(**self.diagnostics(report)):
                    self.assertFalse(report['complete']);self.assertEqual(report['unresolved_hold_units'],269)
                    self.assertEqual(report['known_receipt_units'],180)
                    self.assertEqual(report['reconciliation'][0]['action'],'review_known_receipt_no_resend')
    def test_wrong_quote_runtime_and_malformed_usage_fail_closed(self):
        for fault in ['wrong_quote','runtime_mismatch','oversized_reply']:
            with self.subTest(fault=fault):
                self.out=Path(self.tmp.name)/fault;report=self.execute(fault)
                diagnostic=json.dumps(self.diagnostics(report),sort_keys=True)
                self.assertFalse(report['complete'],diagnostic)
                self.assertEqual(report['claimed_slots'],0,diagnostic)
                self.assertEqual(report['ledger_charged_units'],0,diagnostic)
        self.out=Path(self.tmp.name)/'bad-usage';report=self.execute('wrong_usage')
        self.assertEqual(report['claimed_slots'],1);self.assertEqual(report['unresolved_hold_units'],269)
        self.assertEqual(report['known_receipt_units'],0);self.assertFalse(report['complete'])
    def test_late_valid_reply_charged_and_audit_fsync_failure_not_zeroed(self):
        report=self.execute('late_reply',1800)
        self.assertFalse(report['complete']);self.assertEqual(report['ledger_charged_units'],180)
        self.assertEqual(report['late_receipts'],1);self.assertEqual(report['unresolved_hold_units'],0)
        self.out=Path(self.tmp.name)/'fsync';report=self.execute('receipt_fsync_failure')
        self.assertFalse(report['audit_complete']);self.assertEqual(report['ledger_charged_units'],180)
        self.assertEqual(report['unresolved_hold_units'],0)


    def test_failed_claim_never_sends_and_eof_retains_hold(self):
        report=self.execute('sqlite_claim_failure')
        self.assertEqual(report['claimed_slots'],0);self.assertEqual(report['unresolved_hold_units'],269)
        self.assertFalse((self.out/'receipt-0.json').exists())
        self.out=Path(self.tmp.name)/'eof';report=self.execute('worker_eof')
        self.assertEqual(report['claimed_slots'],1);self.assertEqual(report['unresolved_hold_units'],269)
        self.assertEqual(report['known_receipt_units'],0)
    def test_late_fallback_receipt_retains_usage_and_timing(self):
        report=self.execute('late_receipt_failure',1800)
        self.assertFalse(report['complete']);self.assertEqual(report['late_receipts'],1)
        self.assertEqual(report['known_receipt_units'],180);self.assertEqual(report['unresolved_hold_units'],269)

class AuditAndLifetimeTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.out=Path(self.tmp.name)/'run'
    def test_audit_needs_ended_supervisor_and_checks_manifest_binding(self):
        run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=3000,audit_reserve_ms=300)
        seal=self.out/'supervisor.json';original=seal.read_bytes();seal.unlink()
        self.assertFalse(audit_run(self.out)['complete'])
        seal.write_bytes(original)
        manifest=self.out/'manifest.json';value=json.loads(manifest.read_text());value['runtime']['revision']='tampered'
        manifest.write_text(json.dumps(value))
        report=audit_run(self.out);self.assertFalse(report['complete']);self.assertFalse(report['audit_complete'])
    def test_audit_is_read_only_and_corrupt_accounting_unscored(self):
        run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=3000,audit_reserve_ms=300)
        before={p.name:p.read_bytes() for p in self.out.iterdir() if p.is_file()}
        audit_run(self.out)
        self.assertEqual(before,{p.name:p.read_bytes() for p in self.out.iterdir() if p.is_file()})
        db=sqlite3.connect(self.out/'ledger.sqlite');db.execute("UPDATE slots SET state='reserved',charged_units=-1 WHERE slot=0");db.commit();db.close()
        report=audit_run(self.out);self.assertFalse(report['audit_complete']);self.assertIsNone(report['ledger_charged_units'])
    def test_conflicting_receipt_and_event_tampering_refused(self):
        run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=3000,audit_reserve_ms=300)
        receipt=self.out/'receipt-0.json';original=receipt.read_bytes();record=json.loads(original)
        record['observed_ns']+=1;receipt.write_text(json.dumps(record))
        self.assertFalse(audit_run(self.out)['audit_complete']);receipt.write_bytes(original)
        db=sqlite3.connect(self.out/'ledger.sqlite')
        db.execute("INSERT INTO events(event,slot,payload,observed_ns) SELECT event,slot,payload,observed_ns FROM events WHERE event='claimed' LIMIT 1")
        db.commit();db.close()
        report=audit_run(self.out);self.assertFalse(report['audit_complete']);self.assertIsNone(report['ledger_charged_units'])

    def test_terminal_payload_and_future_event_fail_closed(self):
        for scenario in ('payload','future'):
            self.out=Path(self.tmp.name)/scenario
            run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=3000,audit_reserve_ms=300)
            seal=json.loads((self.out/'supervisor.json').read_text())
            db=sqlite3.connect(self.out/'ledger.sqlite')
            db.execute('DROP TRIGGER immutable_events_update')
            if scenario=='payload':
                db.execute("UPDATE events SET payload='[]' WHERE event='controller_finished'")
            else:
                db.execute("UPDATE events SET observed_ns=? WHERE event='controller_finished'",(seal['finished_ns']+1,))
            db.commit();db.close()
            report=audit_run(self.out)
            self.assertFalse(report['complete']);self.assertFalse(report['audit_complete'])
            self.assertIsNone(report['ledger_charged_units']);self.assertIn('audit_error',report)
    def test_observed_finish_overrun_is_not_complete_but_preserves_usage(self):
        run_fixture(self.out,approval=ExecutionApproval('offline_fixture'),budget_ms=3000,audit_reserve_ms=300)
        path=self.out/'supervisor.json';seal=json.loads(path.read_text())
        seal['finished_ns']=seal['deadline_ns']+1
        seal['deadline_overrun']=True
        path.write_text(json.dumps(seal))
        report=audit_run(self.out)
        self.assertFalse(report['complete']);self.assertTrue(report['audit_complete'])
        self.assertTrue(report['supervisor']['deadline_overrun'])
        self.assertEqual(report['ledger_charged_units'],1592)
        seal['deadline_overrun']=False;path.write_text(json.dumps(seal))
        self.assertFalse(audit_run(self.out)['audit_complete'])

    def test_supervisor_death_terminates_owned_descendants(self):
        import os,signal,subprocess,sys
        script=Path(__file__).with_name('incident_calibration_controller.py')
        parent=subprocess.Popen([sys.executable,str(script),'fixture','--out',str(self.out),'--fault','hang_generation','--budget-ms','3000'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        child=None;worker=None
        def proc_for(pid):
            for path in Path('/proc').glob('[0-9]*'):
                try:
                    lines=path.joinpath('status').read_text().splitlines()
                    ids=next(line.split()[1:] for line in lines if line.startswith('NSpid:'))
                    if int(ids[-1]) == pid: return path
                except (OSError,StopIteration,ValueError): pass
            return None
        def running(pid):
            path=proc_for(pid)
            if path is None:return False
            try:return path.joinpath('stat').read_text().split()[2] not in ('Z','X')
            except FileNotFoundError:return False
        def children_of(pid):
            result=[]
            for path in Path('/proc').glob('[0-9]*'):
                try:
                    lines=path.joinpath('status').read_text().splitlines()
                    parent_id=int(next(line.split()[-1] for line in lines if line.startswith('PPid:')))
                    if parent_id==pid:
                        result.append(int(next(line.split()[-1] for line in lines if line.startswith('NSpid:'))))
                except (OSError,StopIteration,ValueError):pass
            return result
        try:
            end=time.monotonic()+2
            while time.monotonic()<end:
                children=children_of(parent.pid)
                if children:
                    child=children[0];grandchildren=children_of(child)
                    if grandchildren and (self.out/'ledger.sqlite').exists():
                        worker=int(grandchildren[0]);db=sqlite3.connect(self.out/'ledger.sqlite')
                        try:claimed=db.execute("SELECT count(*) FROM events WHERE event='claimed'").fetchone()[0]
                        except sqlite3.Error:claimed=0
                        db.close()
                        if claimed:break
                time.sleep(.01)
            self.assertIsNotNone(worker)
            parent.kill();parent.wait(timeout=1)
            end=time.monotonic()+.5
            while time.monotonic()<end and (running(child) or running(worker)):time.sleep(.01)
            self.assertFalse(running(child),'child survived supervisor death')
            self.assertFalse(running(worker),'worker survived controller death')
            report=audit_run(self.out);self.assertFalse(report['complete']);self.assertEqual(report['unresolved_hold_units'],269)
        finally:
            if parent.poll() is None:parent.kill();parent.wait()
            if child and running(child):
                try:os.killpg(child,signal.SIGKILL)
                except ProcessLookupError:pass

if __name__=='__main__':unittest.main()
