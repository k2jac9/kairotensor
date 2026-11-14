import copy
import dataclasses
import json
import threading
import unittest
from types import SimpleNamespace
from handoff_model import RequestHandler
from incident_preflight import (PreparedBridge, BridgeError, build_request, validate_envelope,
                                calibration_plan, MODEL, REVISION, CAP, CALIBRATION_INVALID_OUTPUT)

class FakeWorker:
    """Deliberately fake tensor counts; never Qwen tokenizer measurements."""
    def __init__(self):
        self.tokenized=[]; self.generated=[]; self.commands=[]
        self.handler=RequestHandler(self.tokenize,self.generate)
    def tokenize(self, request):
        # Strict full-envelope checker, not RequestHandler's permissive injection.
        assert set(request)=={'adapter','model','revision','dtype','add_generation_prompt','generation','messages'}
        assert request['adapter']=='transformers-cpu/v1'
        assert request['model']==MODEL and request['revision']==REVISION
        assert request['dtype']=='bfloat16' and request['add_generation_prompt'] is True
        assert request['generation']=={'do_sample':False,'num_beams':1,'max_new_tokens':96}
        assert type(request['generation']['num_beams']) is int
        assert type(request['generation']['max_new_tokens']) is int
        assert all(set(m)=={'role','content'} and isinstance(m['content'],str) for m in request['messages'])
        inputs={'input_ids':SimpleNamespace(shape=(1,173)), 'attention_mask':object()}
        self.tokenized.append((inputs,request))
        return inputs,request['generation']
    def generate(self, inputs, generation):
        self.generated.append((inputs,generation))
        return {'text':'fake completion','input_tokens':173,'output_tokens':7,'generation_us':12,
                'hit_cap':False,'ended_eos':True}
    def exchange(self, command):
        self.commands.append(copy.deepcopy(command))
        return self.handler.handle(command)

class IncidentPreflightTests(unittest.TestCase):
    def setUp(self):
        self.request=build_request('cal-01')
        self.worker=FakeWorker()
        self.client=PreparedBridge(self.worker.exchange)

    def test_exact_full_request_tensors_quote_and_single_consumption(self):
        quote=self.client.prepare(self.request)
        self.assertEqual(quote.input_tokens,173);self.assertEqual(quote.output_cap,96)
        self.assertEqual(quote.request_utf8,self.request)
        self.assertEqual(self.worker.commands,[{'op':'prepare','request_utf8':self.request}])
        self.assertEqual(len(self.worker.tokenized),1);self.assertEqual(self.worker.generated,[])
        reply=self.client.generate(quote)
        self.assertEqual(reply['input_tokens'],173);self.assertEqual(reply['output_tokens'],7)
        self.assertIs(self.worker.generated[0][0],self.worker.tokenized[0][0])
        self.assertIs(self.worker.generated[0][1],self.worker.tokenized[0][1]['generation'])
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertEqual(len(self.worker.generated),1)
        self.assertEqual([c['op'] for c in self.worker.commands],['prepare','generate'])

    def test_discard_and_foreign_or_mutated_tickets_never_generate(self):
        quote=self.client.prepare(self.request)
        foreign=PreparedBridge(FakeWorker().exchange)
        with self.assertRaises(BridgeError): foreign.generate(quote)
        altered=dataclasses.replace(quote,input_tokens=174)
        with self.assertRaises(BridgeError): self.client.generate(altered)
        self.assertEqual(len(self.worker.commands),1)
        self.client.discard(quote)
        self.assertIsNone(self.worker.handler.pending)
        with self.assertRaises(BridgeError): self.client.generate(quote)
        next_quote=self.client.prepare(self.request)
        self.assertNotEqual(next_quote.handle,quote.handle)
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertEqual(self.worker.generated,[])

    def test_bypassing_dataclass_freeze_is_detected_against_snapshot(self):
        quote=self.client.prepare(self.request)
        object.__setattr__(quote,'input_tokens',174)
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertEqual(len(self.worker.commands),1)

    def test_real_handler_slot_and_quote_mismatch_contract(self):
        quote=self.client.prepare(self.request)
        with self.assertRaises(ValueError): self.worker.handler.handle({'op':'prepare','request_utf8':self.request})
        with self.assertRaises(ValueError): self.worker.handler.handle({'op':'generate','handle':quote.handle,'input_tokens':174,'max_new_tokens':96})
        self.assertEqual(self.worker.generated,[])
        self.assertEqual(self.client.generate(quote)['output_tokens'],7)

    def test_lost_handle_after_worker_restart_fails_closed(self):
        quote=self.client.prepare(self.request)
        self.worker.handler=RequestHandler(self.worker.tokenize,self.worker.generate)
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertEqual(self.client.state,'uncertain')
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertEqual(self.worker.generated,[])

    def test_ambiguous_prepare_generate_discard_poison_client(self):
        for operation in ('prepare','generate','discard'):
            worker=FakeWorker()
            def transport(command):
                result=worker.exchange(command)
                if command['op']==operation: raise OSError('lost response')
                return result
            client=PreparedBridge(transport)
            if operation=='prepare':
                with self.assertRaises(BridgeError): client.prepare(self.request)
            else:
                quote=client.prepare(self.request)
                with self.assertRaises(BridgeError): getattr(client,operation)(quote)
            calls=len(worker.commands)
            self.assertEqual(client.state,'uncertain')
            with self.assertRaises(BridgeError): client.prepare(self.request)
            self.assertEqual(len(worker.commands),calls)

    def test_generation_exception_consumes_worker_handle(self):
        quote=self.client.prepare(self.request)
        def fail(*args): raise RuntimeError('fake generation failure')
        self.worker.handler.generate=fail
        with self.assertRaises(BridgeError): self.client.generate(quote)
        self.assertIsNone(self.worker.handler.pending)
        with self.assertRaises(BridgeError): self.client.generate(quote)

    def test_full_envelope_and_feedback_are_label_independent(self):
        request=json.loads(self.request);validate_envelope(self.request)
        self.assertEqual(request['generation']['max_new_tokens'],96)
        feedback=json.loads(build_request('cal-01',CALIBRATION_INVALID_OUTPUT))
        self.assertEqual(feedback['messages'][:2],request['messages'])
        self.assertEqual(feedback['messages'][2],{'role':'assistant','content':CALIBRATION_INVALID_OUTPUT})
        valid_but_wrong=json.dumps({'decision':'route','owner':'quartz','reason':'diagnosed_component','evidence_ids':['e01']})
        with self.assertRaises(ValueError): build_request('cal-01',valid_but_wrong)
        with self.assertRaises(ValueError): build_request('hold-01')
        for key,value in [('model','different'),('revision','different'),('dtype','float32'),('add_generation_prompt',1),('extra',0)]:
            wrong=copy.deepcopy(request);wrong[key]=value
            with self.assertRaises(ValueError): validate_envelope(json.dumps(wrong))
        for key,value in [('do_sample',True),('num_beams',True),('max_new_tokens',True),('max_new_tokens',95)]:
            wrong=copy.deepcopy(request);wrong['generation'][key]=value
            with self.assertRaises(ValueError): validate_envelope(json.dumps(wrong))

    def test_plan_is_calibration_only_unmeasured_without_hidden_warmup(self):
        plan=calibration_plan()
        self.assertFalse(plan['execution_authorized']);self.assertEqual(plan['new_model_calls'],0)
        self.assertEqual(plan['max_generation_calls'],8)
        self.assertEqual(len(plan['schedule']),8)
        self.assertEqual(sum(s['measurement_planned'] for s in plan['schedule']),8)
        self.assertEqual({s['case_id'] for s in plan['schedule']},{'cal-01','cal-02','cal-03','cal-04'})
        self.assertTrue(all(s['input_tokens'] is None for s in plan['schedule']))
        self.assertEqual(plan['max_total_quoted_tokens'],8*(2048+96))
        self.assertEqual(plan['max_job_elapsed_ms'],1200000)

    def test_other_threads_cannot_use_client(self):
        quote=self.client.prepare(self.request)
        errors=[]
        def attempt():
            try:self.client.generate(quote)
            except BridgeError as error:errors.append(error)
        thread=threading.Thread(target=attempt);thread.start();thread.join()
        self.assertEqual(len(errors),1);self.assertEqual(self.worker.generated,[])
        self.assertEqual(len(self.worker.commands),1)

    def test_malformed_returned_measurements_preserved_without_retry(self):
        mutations = [('input_tokens',174),('input_tokens',True),('output_tokens',-1),
                     ('output_tokens',True),('output_tokens',97),('generation_us',2**63),
                     ('generation_us',-1),('hit_cap',True),('ended_eos',1),('text',None)]
        for key,value in mutations:
            worker=FakeWorker();returned=[]
            def transport(command):
                reply=worker.exchange(command)
                if command['op']=='generate': reply[key]=value;returned.append(copy.deepcopy(reply))
                return reply
            client=PreparedBridge(transport);quote=client.prepare(self.request)
            with self.assertRaises(BridgeError) as caught:client.generate(quote)
            self.assertEqual(caught.exception.reply,returned[0]);self.assertEqual(client.state,'uncertain')
            before=len(worker.commands)
            with self.assertRaises(BridgeError):client.generate(quote)
            self.assertEqual(len(worker.commands),before);self.assertEqual(len(worker.generated),1)
        worker=FakeWorker()
        def zero_eos(command):
            reply=worker.exchange(command)
            if command['op']=='generate':reply['output_tokens']=0
            return reply
        client=PreparedBridge(zero_eos);quote=client.prepare(self.request)
        with self.assertRaises(BridgeError):client.generate(quote)

    def test_bad_preparation_or_discard_receipt_poison_client(self):
        for field,value in [('input_tokens',True),('input_tokens',2049),('input_tokens',-1),('max_new_tokens',True),('max_new_tokens',95),('handle','lost')]:
            worker=FakeWorker()
            def transport(command):
                reply=worker.exchange(command);reply[field]=value;return reply
            client=PreparedBridge(transport)
            with self.assertRaises(BridgeError) as caught:client.prepare(self.request)
            self.assertEqual(caught.exception.reply[field],value);self.assertEqual(client.state,'uncertain')
            with self.assertRaises(BridgeError):client.prepare(self.request)
            self.assertEqual(len(worker.commands),1)
        worker=FakeWorker()
        def bad_discard(command):
            reply=worker.exchange(command)
            return {'discarded':'wrong'} if command['op']=='discard' else reply
        client=PreparedBridge(bad_discard);quote=client.prepare(self.request)
        with self.assertRaises(BridgeError):client.discard(quote)
        self.assertEqual(client.state,'uncertain');self.assertIsNone(worker.handler.pending)

    def test_prepare_thread_and_reentrant_operations_are_refused(self):
        errors=[]
        def other_thread():
            try:self.client.prepare(self.request)
            except BridgeError:errors.append(True)
        thread=threading.Thread(target=other_thread);thread.start();thread.join()
        self.assertEqual(errors,[True]);self.assertEqual(self.worker.commands,[])
        worker=FakeWorker();client=None
        def reentrant(command):
            with self.assertRaises(BridgeError):client.prepare(self.request)
            return worker.exchange(command)
        client=PreparedBridge(reentrant);quote=client.prepare(self.request);client.discard(quote)
        self.assertEqual([c['op'] for c in worker.commands],['prepare','discard'])

    def test_transport_cannot_change_quote_used_for_reply_validation(self):
        worker=FakeWorker();quote=None
        def transport(command):
            reply=worker.exchange(command)
            if command['op']=='generate':
                object.__setattr__(quote,'input_tokens',174)
                reply['input_tokens']=174
            return reply
        client=PreparedBridge(transport);quote=client.prepare(self.request)
        with self.assertRaises(BridgeError) as caught:client.generate(quote)
        self.assertEqual(caught.exception.reply['input_tokens'],174)
        self.assertEqual(client.state,'uncertain')

    def test_offline_cli_has_no_live_path_or_overwrite(self):
        import pathlib,subprocess,sys,tempfile
        script=pathlib.Path(__file__).with_name('incident_preflight.py')
        with tempfile.TemporaryDirectory() as tmp:
            out=pathlib.Path(tmp)/'plan.json'
            command=[sys.executable,str(script),'--out',str(out)]
            self.assertNotEqual(subprocess.run(command+['--live'],capture_output=True).returncode,0)
            self.assertFalse(out.exists())
            self.assertEqual(subprocess.run(command,capture_output=True).returncode,0)
            raw=out.read_bytes()
            self.assertNotEqual(subprocess.run(command,capture_output=True).returncode,0)
            self.assertEqual(out.read_bytes(),raw)

    def test_one_exclusive_client_can_run_all_eight_planned_requests(self):
        old=[]
        for slot in calibration_plan()['schedule']:
            quote=self.client.prepare(slot['request_utf8'])
            for prior in old:
                with self.assertRaises(BridgeError):self.client.generate(prior)
                with self.assertRaises(BridgeError):self.client.discard(prior)
            reply=self.client.generate(quote)
            self.assertEqual(reply['input_tokens'],173)
            self.assertEqual(self.client.state,'consumed')
            old.append(quote)
        self.assertEqual(len(self.worker.tokenized),8)
        self.assertEqual(len(self.worker.generated),8)
        self.assertEqual([c['op'] for c in self.worker.commands],['prepare','generate']*8)
        for inputs,_ in self.worker.generated:
            self.assertTrue(any(inputs is tokenized[0] for tokenized in self.worker.tokenized))

    def test_cap_eos_boundary_and_missing_fields_preserve_protocol_semantics(self):
        for output,eos in [(96,True),(96,False),(0,False)]:
            worker=FakeWorker()
            def transport(command):
                reply=worker.exchange(command)
                if command['op']=='generate':reply.update(output_tokens=output,hit_cap=(output==96),ended_eos=eos)
                return reply
            client=PreparedBridge(transport);quote=client.prepare(self.request)
            self.assertEqual(client.generate(quote)['output_tokens'],output)
        worker=FakeWorker()
        def missing(command):
            reply=worker.exchange(command)
            if command['op']=='generate':reply.pop('output_tokens')
            return reply
        client=PreparedBridge(missing);quote=client.prepare(self.request)
        with self.assertRaises(BridgeError) as caught:client.generate(quote)
        self.assertNotIn('output_tokens',caught.exception.reply)
        self.assertEqual(client.state,'uncertain')

    def test_plan_content_and_fingerprint_are_frozen_without_heldout_content(self):
        from incident_routing import load_workload, digest
        plan=calibration_plan();fingerprint=plan.pop('plan_sha256')
        self.assertEqual(fingerprint,digest(plan))
        self.assertEqual(len(plan['worker_source_sha256']),64)
        text=json.dumps(plan)
        for case in load_workload()['cases']['cases']:
            if case['split']=='heldout':
                self.assertNotIn(case['case_id'],text)
                for evidence in case['evidence']:self.assertNotIn(evidence['text'],text)
        for slot in plan['schedule']:
            self.assertEqual(slot['measurement_status'],'not_executed')
            self.assertNotIn('measured',slot)
        self.assertEqual([slot['readiness'] for slot in plan['schedule']].count('cold_first_generation_after_load'),1)

if __name__=='__main__':unittest.main()
