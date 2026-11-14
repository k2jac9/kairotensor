import json
import tempfile
import unittest
from pathlib import Path
from incident_hosted_controller import HostedLedger,OfflineApproval,OfflinePriceBound,FixtureTransport,HttpReceipt

MODEL='fixture-hosted-model/1'
def reply(usage=None,status='completed',text='{}'):
    return HttpReceipt(200,(('content-type','application/json'),),json.dumps({'id':'resp-fixture','object':'response','model':MODEL,'status':status,'output':[{'type':'message','role':'assistant','content':[{'type':'output_text','text':text}]}],'output_text':text,'usage':usage if usage is not None else {'input_tokens':10,'output_tokens':5,'total_tokens':15}}).encode())

class HostedTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)/'run'
        self.ledger=HostedLedger.create(self.root,budget=800,clock_domain='fixture-clock',start=0,deadline=1000)
        self.quote=OfflinePriceBound(400,2,3,10,20,96)
    def op(self,key='a'):
        return self.ledger.create_operation(key,'cal-01',MODEL,self.quote)
    def dispatch(self,op,transport,now=20):
        return self.ledger.dispatch(op,transport,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:now)
    def test_defaults_deny_and_exact_key_binding(self):
        op=self.op();self.assertEqual(op,self.op())
        with self.assertRaises(ValueError):self.ledger.create_operation('a','cal-02',MODEL,self.quote)
        with self.assertRaises(PermissionError):self.ledger.dispatch(op,FixtureTransport(reply()))
        self.assertEqual(self.ledger.audit()['claimed_count'],0)
    def test_charge_is_synthetic_estimate_and_duplicate_never_resends(self):
        op=self.op();t=FixtureTransport(reply());self.dispatch(op,t)
        a=self.ledger.audit();self.assertEqual(a['estimated_spent'],45);self.assertEqual(a['held_bound'],0)
        self.assertIsNone(a['observed_provider_cost']);self.assertFalse(a['live_admissible'])
        self.dispatch(op,t);self.assertEqual(t.calls,1);self.assertEqual(self.ledger.audit()['estimated_spent'],45)
    def test_ambiguous_send_survives_restart_blocks_continuation(self):
        op=self.op();self.dispatch(op,FixtureTransport(TimeoutError('unknown')))
        reopened=HostedLedger(self.root)
        second=reopened.create_operation('b','cal-02',MODEL,self.quote)
        t=FixtureTransport(reply());reopened.dispatch(second,t,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:30)
        self.assertEqual(t.calls,0);self.assertEqual(reopened.audit()['held_bound'],800)
        self.assertFalse(reopened.cancel(op))
    def test_partial_usage_preserved_without_invented_charge(self):
        op=self.op();self.dispatch(op,FixtureTransport(reply({'input_tokens':10})))
        a=self.ledger.audit();self.assertEqual(a['held_bound'],400);self.assertEqual(a['estimated_spent'],0)
        self.assertEqual(a['operations'][0]['parsed']['usage']['input_tokens'],10)
    def test_late_receipt_still_reconciles_once(self):
        ticks=iter((20,20,20,1200));op=self.op()
        self.ledger.dispatch(op,FixtureTransport(reply()),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:next(ticks))
        a=self.ledger.audit();self.assertEqual(a['estimated_spent'],45);self.assertTrue(a['operations'][0]['late'])
        self.ledger.reconcile(op);self.assertEqual(self.ledger.audit()['estimated_spent'],45)
    def test_budget_denial_and_cancel_only_known_unsent(self):
        self.ledger=HostedLedger.create(Path(self.temp.name)/'small',budget=399,clock_domain='fixture-clock',start=0,deadline=1000)
        op=self.op();t=FixtureTransport(reply());self.dispatch(op,t)
        self.assertEqual(t.calls,0);self.assertEqual(self.ledger.audit()['held_bound'],0)
        self.assertEqual(self.ledger.audit()['operations'][0]['state'],'denied')
    def test_concurrent_duplicate_claim_single_invocation(self):
        from concurrent.futures import ThreadPoolExecutor
        op=self.op();self.ledger.reserve(op)
        def claim(_):return HostedLedger(self.root).claim(op,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        with ThreadPoolExecutor(max_workers=4) as pool:claims=list(pool.map(claim,range(4)))
        self.assertEqual(sum(x is not None for x in claims),1)
        self.assertEqual(self.ledger.audit()['held_bound'],400)

    def test_inconsistent_bound_rejected_and_max_claims_persisted(self):
        with self.assertRaises(ValueError):self.ledger.create_operation('bad','cal-01',MODEL,OfflinePriceBound(100,2,3,10,20))
        limited=HostedLedger.create(Path(self.temp.name)/'limited',budget=800,clock_domain='fixture-clock',start=0,deadline=1000,max_claims=1)
        first=limited.create_operation('a','cal-01',MODEL,self.quote)
        limited.dispatch(first,FixtureTransport(reply()),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        second=limited.create_operation('b','cal-02',MODEL,self.quote);t=FixtureTransport(reply())
        limited.dispatch(second,t,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:30)
        self.assertEqual(t.calls,0);self.assertEqual(limited.audit()['claimed_count'],1)
    def test_final_clock_expiry_skips_transport_preserves_hold(self):
        op=self.op();t=FixtureTransport(reply());ticks=iter((20,1000))
        self.ledger.dispatch(op,t,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:next(ticks))
        self.assertEqual(t.calls,0);self.assertEqual(self.ledger.audit()['held_bound'],400)
    def test_postcommit_expiry_never_invokes_transport(self):
        op=self.op();t=FixtureTransport(reply());ticks=iter((20,20,1000))
        self.ledger.dispatch(op,t,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:next(ticks))
        self.assertEqual(t.calls,0);self.assertEqual(self.ledger.audit()['held_bound'],400)
    def test_receipt_clock_failure_preserves_body_and_usage(self):
        op=self.op();ticks=iter((20,20,20))
        report=self.ledger.dispatch(op,FixtureTransport(reply()),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:next(ticks))
        self.assertEqual(report['estimated_spent'],45,report);self.assertIsNone(report['operations'][0]['late'])
        self.assertTrue((self.root/f'receipt-{op}.json').exists())
    def test_clock_floor_survives_late_receipt_and_restart(self):
        op=self.op();ticks=iter((20,20,20,1200))
        self.ledger.dispatch(op,FixtureTransport(reply()),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:next(ticks))
        reopened=HostedLedger(self.root);second=reopened.create_operation('b','cal-02',MODEL,self.quote);reopened.reserve(second)
        self.assertIsNone(reopened.claim(second,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:60))
    def test_over_bound_and_bad_usage_retains_raw_counts(self):
        for i,usage in enumerate(({'input_tokens':21,'output_tokens':5,'total_tokens':26},{'input_tokens':True,'output_tokens':5,'total_tokens':6},{'input_tokens':10,'output_tokens':5,'total_tokens':99})):
            with self.subTest(usage=usage):
                ledger=HostedLedger.create(Path(self.temp.name)/f'bad-{i}',budget=400,clock_domain='fixture-clock',start=0,deadline=1000)
                op=ledger.create_operation('a','cal-01',MODEL,self.quote)
                a=ledger.dispatch(op,FixtureTransport(reply(usage)),approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
                self.assertEqual(a['held_bound'],400,a);self.assertEqual(a['estimated_spent'],0,a)
                self.assertEqual(a['operations'][0]['parsed']['usage']['raw'],usage)
    def test_terminal_invalid_output_still_has_estimated_cost(self):
        op=self.op();a=self.dispatch(op,FixtureTransport(reply(status='incomplete')))
        self.assertEqual(a['estimated_spent'],45,a);self.assertFalse(a['operations'][0]['parsed']['protocol_valid'])
    def test_duplicate_concurrent_receipts_do_not_double_estimate(self):
        from concurrent.futures import ThreadPoolExecutor
        op=self.op();self.ledger.reserve(op);self.ledger.claim(op,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        def record(i):HostedLedger(self.root).record_receipt(op,reply(),observed=30+i)
        with ThreadPoolExecutor(max_workers=4) as pool:list(pool.map(record,range(4)))
        self.ledger.reconcile(op);self.ledger.reconcile(op)
        self.assertEqual(self.ledger.audit()['estimated_spent'],45)
        with self.assertRaises(ValueError):self.ledger.record_receipt(op,reply(text='different'),observed=99)
    def test_independent_receipt_persistence_and_header_allowlist(self):
        from unittest.mock import patch
        import incident_hosted_controller as controller
        op=self.op();r=reply();r=HttpReceipt(r.status,(('authorization','secret'),('x-request-id','safe')),r.body)
        original=controller._write
        def fail_receipt(path,value):
            if path.name.startswith('receipt-'):raise OSError('injected')
            return original(path,value)
        with patch.object(controller,'_write',fail_receipt):a=self.dispatch(op,FixtureTransport(r))
        self.assertEqual(a['estimated_spent'],45,a)
        import sqlite3
        db=sqlite3.connect(self.root/'ledger.sqlite');raw=db.execute('SELECT payload FROM receipts').fetchone()[0];db.close()
        self.assertNotIn('secret',raw);self.assertIn('safe',raw)
    def test_audit_recomputes_raw_and_rejects_tampering(self):
        import sqlite3
        op=self.op();self.dispatch(op,FixtureTransport(reply()))
        before={p.name:p.read_bytes() for p in self.root.iterdir() if p.is_file()}
        self.ledger.audit();self.assertEqual(before,{p.name:p.read_bytes() for p in self.root.iterdir() if p.is_file()})
        db=sqlite3.connect(self.root/'ledger.sqlite');db.execute('UPDATE operations SET estimated=1 WHERE id=?',(op,));db.commit();db.close()
        a=self.ledger.audit();self.assertFalse(a['audit_complete'],a);self.assertIsNone(a['estimated_spent'])
    def test_fallback_only_receipt_preserves_late_and_reconciles(self):
        import sqlite3
        op=self.op();self.ledger.reserve(op);self.ledger.claim(op,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        db=sqlite3.connect(self.root/'ledger.sqlite');db.execute("CREATE TRIGGER block_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(ABORT,'injected'); END");db.commit();db.close()
        self.ledger.record_receipt(op,reply(),observed=1200)
        a=self.ledger.audit();self.assertEqual(a['held_bound'],400,a);self.assertTrue(a['operations'][0]['late']);self.assertEqual(a['operations'][0]['recomputed_estimate'],45)
        self.ledger.reconcile(op);a=self.ledger.audit();self.assertEqual(a['estimated_spent'],45,a)

    def test_conflicting_receipt_cannot_release_hold_or_enable_continuation(self):
        import sqlite3
        op=self.op();self.ledger.reserve(op);self.ledger.claim(op,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        self.ledger.record_receipt(op,reply(),observed=30)
        path=self.root/f'receipt-{op}.json';value=json.loads(path.read_text());value['status']=201;path.write_text(json.dumps(value))
        with self.assertRaises(ValueError):self.ledger.reconcile(op)
        db=sqlite3.connect(self.root/'ledger.sqlite')
        self.assertEqual(db.execute('SELECT state,estimated FROM operations WHERE id=?',(op,)).fetchone(),('received',None));db.close()
        second=self.ledger.create_operation('b','cal-02',MODEL,self.quote);t=FixtureTransport(reply())
        self.dispatch(second,t);self.assertEqual(t.calls,0)
    def test_fallback_send_clock_checked_before_reconciliation(self):
        import sqlite3
        op=self.op();self.ledger.reserve(op);self.ledger.claim(op,approval=OfflineApproval(),clock_domain='fixture-clock',clock=lambda:20)
        db=sqlite3.connect(self.root/'ledger.sqlite');db.execute("CREATE TRIGGER block_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(ABORT,'injected'); END");db.commit();db.close()
        self.ledger.record_receipt(op,reply(),observed=30)
        path=self.root/f'receipt-{op}.json';value=json.loads(path.read_text());value['send_tick']=1000;value['observed']=1001;path.write_text(json.dumps(value))
        with self.assertRaises(ValueError):self.ledger.reconcile(op)
        db=sqlite3.connect(self.root/'ledger.sqlite')
        self.assertEqual(db.execute('SELECT state,estimated FROM operations WHERE id=?',(op,)).fetchone(),('claimed',None));db.close()

if __name__=='__main__':unittest.main()
