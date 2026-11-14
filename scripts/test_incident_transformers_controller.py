"""No installed Transformers/Torch module is used by these process tests."""
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from incident_transformers_controller import run_prepared, audit_prepared
from incident_transformers_backend import InvocationApproval


class PreparedControllerTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.out=Path(self.temp.name)/'run'
    def execute(self,fault='none'):
        return run_prepared(self.out,approval=InvocationApproval('offline_fake_dependencies'),fault=fault,budget_ms=30000,audit_reserve_ms=1000)
    def test_denied_before_files_or_dependency_loading(self):
        with self.assertRaises(PermissionError):run_prepared(self.out)
        self.assertFalse(self.out.exists())
    def test_eight_slots_use_backend_observations_and_same_accounting(self):
        report=self.execute()
        self.assertTrue(report['complete'],report)
        self.assertEqual(report['ledger_charged_units'],1592,report)
        self.assertEqual(report['unresolved_hold_units'],0)
        self.assertEqual(report['new_model_calls'],0)
        self.assertEqual(report['backend'],'offline_fake_dependencies')
        self.assertTrue((self.out/'runtime-observed.json').exists())
        for i in range(8):
            p=json.loads((self.out/f'prepared-{i}.json').read_text())
            self.assertEqual(len(p['observation']['input_ids'][0]),173 if i<4 else 211)
            self.assertEqual(p['observation']['attention_mask'],[[1]*len(p['observation']['input_ids'][0])])
        self.assertEqual(audit_prepared(self.out),report)
        with self.assertRaises(FileExistsError):self.execute()
    def test_decode_failure_preserves_measured_ids_and_hold(self):
        report=self.execute('decode_failure')
        self.assertFalse(report['complete'],report)
        self.assertEqual(report['claimed_slots'],1,report)
        self.assertEqual(report['unresolved_hold_units'],269,report)
        self.assertEqual(report['known_observation_units'],180,report)
        self.assertEqual(report['ledger_charged_units'],0,report)
        self.assertTrue((self.out/'backend-receipt-0.json').exists())
    def test_valid_reply_without_continuation_observation_never_settles(self):
        report=self.execute('missing_observation')
        self.assertFalse(report['complete'],report)
        # A malformed success envelope is not independently verified accounting.
        self.assertFalse(report['audit_complete'],report)
        db=sqlite3.connect(self.out/'ledger.sqlite')
        row=db.execute('SELECT state,charged_units,input_units+output_cap FROM slots WHERE slot=0').fetchone();db.close()
        self.assertEqual(row,('uncertain',None,269))
        self.assertTrue((self.out/'backend-receipt-0.json').exists())
    def test_changed_observation_is_unscored(self):
        self.execute()
        p=self.out/'prepared-0.json';r=json.loads(p.read_text())
        r['observation']['input_ids'][0][0]+=1;p.write_text(json.dumps(r))
        report=audit_prepared(self.out)
        self.assertFalse(report['audit_complete'],report)
        self.assertIsNone(report['ledger_charged_units'])
    def test_bad_runtime_never_claims(self):
        report=self.execute('runtime_mismatch')
        self.assertFalse(report['complete'],report)
        self.assertEqual(report['claimed_slots'],0,report)
        self.assertEqual(report['ledger_charged_units'],0,report)

    def test_crash_settlement_and_invalid_tensor_paths_keep_holds(self):
        for fault,known in [('after_claim',0),('after_receipt',180),('sqlite_settle_failure',180),('wrong_prefix',0),('overcap',0)]:
            with self.subTest(fault=fault):
                self.out=Path(self.temp.name)/fault
                report=self.execute(fault)
                self.assertFalse(report['complete'],report)
                self.assertEqual(report['claimed_slots'],1,report)
                self.assertEqual(report['unresolved_hold_units'],269,report)
                self.assertEqual(report['known_observation_units'],known,report)
                self.assertEqual(report['ledger_charged_units'],0,report)
    def test_raw_artifact_fsync_failure_preserves_usage_in_ledger(self):
        report=self.execute('raw_receipt_failure')
        self.assertFalse(report['complete'],report);self.assertFalse(report['audit_complete'],report)
        self.assertEqual(report['ledger_charged_units'],180,report)
        self.assertEqual(report['known_observation_units'],180,report)
        self.assertEqual(report['unresolved_hold_units'],0,report)
        self.assertFalse((self.out/'backend-receipt-0.json').exists())
        db=sqlite3.connect(self.out/'ledger.sqlite')
        self.assertEqual(db.execute("SELECT count(*) FROM events WHERE event='backend_received'").fetchone()[0],1);db.close()
    def test_delayed_persistence_does_not_relabel_receipt_late(self):
        # First obtain a real new-backend fake-dependency envelope. Then exercise
        # the same transport/SQLite receipt path with a synthetic clock, not a
        # wall-clock sleep that races the independent process watchdog.
        import base64
        from unittest.mock import patch
        import incident_transformers_controller as controller
        import incident_calibration_controller as ledger
        from incident_preflight import Ticket
        from incident_routing import digest
        seed=self.execute();self.assertTrue(seed['complete'],seed)
        manifest=json.loads((self.out/'manifest.json').read_text())
        ready=json.loads((self.out/'runtime-observed.json').read_text())
        prepared=json.loads((self.out/'prepared-0.json').read_text())
        raw_record=json.loads((self.out/'backend-receipt-0.json').read_text())
        raw=base64.b64decode(raw_record['raw_base64'])
        self.out=Path(self.temp.name)/'injected-clock';self.out.mkdir()
        manifest.update(start_ns=0,work_deadline_ns=100,deadline_ns=1000,fault='none')
        ledger._durable_new(self.out/'manifest.json',manifest)
        tick=[10]
        with patch.object(controller,'now_ns',lambda mode:tick[0]),patch.object(ledger,'now_ns',lambda mode:tick[0]):
            db=ledger._initialize(self.out,manifest['plan'],manifest['clock_mode'])
            try:
                ledger._durable_new(self.out/'runtime-observed.json',ready)
                ledger._durable_new(self.out/'prepared-0.json',prepared)
                with db:
                    ledger._event(db,'runtime_observed',None,{'artifact_sha256':digest(ready),'runtime_sha256':ready['runtime_sha256']},manifest['clock_mode'])
                    ledger._event(db,'preparation_observed',0,{'artifact_sha256':digest(prepared),'prepared_sha256':prepared['observation']['prepared_sha256']},manifest['clock_mode'])
                slot=manifest['plan']['schedule'][0];quote=prepared['reply']
                ticket=Ticket(quote['handle'],slot['request_utf8'],slot['request_sha256'],quote['input_tokens'],quote['max_new_tokens'])
                ledger._reserve(db,0,ticket,manifest);tick[0]=20
                ledger._claim(db,0,ticket,manifest,manifest['runtime'])
                transport=controller.Transport.__new__(controller.Transport)
                transport.db=db;transport.out=self.out;transport.slot=0;transport.manifest=manifest
                transport.fault='none';transport.errors=[];transport.runtime=ready['runtime_sha256']
                transport.prepared={0:prepared['observation']};transport._write=lambda command:None
                def read_reply(deadline):
                    tick[0]=50
                    return raw
                transport._line=read_reply
                durable=ledger._durable_new
                def slow_persistence(path,payload,**kwargs):
                    durable(path,payload,**kwargs)
                    if path.name=='backend-receipt-0.json':tick[0]=150
                with patch.object(ledger,'_durable_new',slow_persistence):
                    reply=transport({'op':'generate','handle':ticket.handle,'input_tokens':ticket.input_tokens,'max_new_tokens':ticket.output_cap})
                with db:
                    units=reply['input_tokens']+reply['output_tokens']
                    db.execute("UPDATE slots SET state='settled',charged_units=? WHERE slot=0",(units,))
                    ledger._event(db,'settled',0,{'charged_units':units},manifest['clock_mode'])
                self.assertEqual(db.execute('SELECT receipt_ns,late,charged_units FROM slots WHERE slot=0').fetchone(),(50,0,180))
                self.assertEqual(db.execute("SELECT observed_ns FROM events WHERE event='backend_received'").fetchone()[0],150)
            finally:db.close()
        report=audit_prepared(self.out)
        self.assertTrue(report['audit_complete'],report);self.assertFalse(report['complete'],report)
        self.assertEqual(report['ledger_charged_units'],180,report);self.assertEqual(report['late_receipts'],0,report)
        self.assertEqual(json.loads((self.out/'receipt-0.json').read_text())['observed_ns'],50)
    def test_worker_load_failure_and_hang(self):
        report=self.execute('load_failure')
        self.assertEqual(report['claimed_slots'],0,report);self.assertEqual(report['ledger_charged_units'],0,report)
        self.out=Path(self.temp.name)/'hang'
        report=run_prepared(self.out,approval=InvocationApproval('offline_fake_dependencies'),fault='hang_generation',budget_ms=2000,audit_reserve_ms=500)
        self.assertFalse(report['complete'],report);self.assertEqual(report['unresolved_hold_units'],269,report)
        self.assertTrue(report['supervisor']['child_reaped'],report)
    def test_no_live_cli_or_artifact_grant(self):
        import subprocess,sys
        script=Path(__file__).with_name('incident_transformers_controller.py')
        result=subprocess.run([sys.executable,str(script),'fixture','--out',str(self.out),'--live'],capture_output=True)
        self.assertNotEqual(result.returncode,0);self.assertFalse(self.out.exists())
        with self.assertRaises(PermissionError):run_prepared(self.out,approval={'kind':'local_cached_model'})
        self.assertFalse(self.out.exists())
    def test_legacy_v1_audit_compatibility_with_pinned_sources(self):
        import incident_calibration_controller as old
        from incident_routing import canonical,digest
        report=old.run_fixture(self.out,approval=old.ExecutionApproval('offline_fixture'),budget_ms=30000,audit_reserve_ms=1000)
        self.assertTrue(report['complete'],report)
        # Rebind an otherwise identical v1 bundle to the explicitly pinned PR16
        # source tuple; this is a format compatibility test, not authentication.
        path=self.out/'manifest.json';m=json.loads(path.read_text());m['sources']=old._LEGACY_V1_SOURCES
        path.write_text(canonical(m))
        db=sqlite3.connect(self.out/'ledger.sqlite');db.execute('DROP TRIGGER immutable_events_update')
        db.execute("UPDATE events SET payload=? WHERE event='planned'",(canonical({'plan_sha256':m['plan']['plan_sha256'],'manifest_sha256':digest(m)}),));db.commit();db.close()
        path=self.out/'supervisor.json';seal=json.loads(path.read_text());seal['manifest_sha256']=digest(m);path.write_text(canonical(seal))
        report=old.audit_run(self.out);self.assertTrue(report['complete'],report)
    def test_manifest_and_raw_observation_tampering_refused(self):
        import base64
        from incident_routing import canonical
        self.execute()
        path=self.out/'backend-receipt-0.json';record=json.loads(path.read_text());envelope=json.loads(base64.b64decode(record['raw_base64']))
        envelope['observation']['output_tokens']=6;record['raw_base64']=base64.b64encode(canonical(envelope).encode()).decode();path.write_text(canonical(record))
        report=audit_prepared(self.out);self.assertFalse(report['audit_complete'],report);self.assertIsNone(report['ledger_charged_units'])

if __name__=='__main__':unittest.main()
