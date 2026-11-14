import copy
import contextlib
import sys
import unittest
from types import SimpleNamespace

from incident_preflight import build_request
from incident_transformers_backend import Backend, InvocationApproval, REQUIRED_VERSIONS

from incident_transformers_fake import FakeDependencies

def backend(fault=None):
    dependencies=FakeDependencies(fault)
    return Backend(approval=InvocationApproval('offline_fake_dependencies'),dependency_factory=lambda:dependencies),dependencies

def prepare(worker):return worker.handle({'op':'prepare','request_utf8':build_request('cal-01')})
def generate(worker,prepared):
    quote=prepared['reply']
    return worker.handle({'op':'generate','handle':quote['handle'],'input_tokens':quote['input_tokens'],'max_new_tokens':quote['max_new_tokens']})

class BackendTests(unittest.TestCase):
    def test_denial_precedes_factory_and_imports(self):
        from unittest.mock import patch
        import incident_transformers_backend as module
        with patch.object(module.importlib,'import_module',side_effect=AssertionError('dependency import attempted')) as imports, patch.object(module.importlib.metadata,'version',side_effect=AssertionError('dependency inspection attempted')) as versions:
            for approval in (None,InvocationApproval(),InvocationApproval('arbitrary'),{'kind':'local_cached_model'}):
                with self.subTest(approval=approval),self.assertRaises(PermissionError):
                    Backend(approval=approval,dependency_factory=lambda:self.fail('factory called'))
            with self.assertRaises(PermissionError):Backend()
            with self.assertRaises(PermissionError):
                Backend(approval=InvocationApproval('local_cached_model'),dependency_factory=lambda:self.fail('factory called'))
            imports.assert_not_called();versions.assert_not_called()
    def test_exact_retained_tensors_config_and_wire(self):
        worker,deps=backend();ready=worker.ready();p=prepare(worker)
        self.assertIsNone(p['error']);self.assertEqual(p['reply']['input_tokens'],173)
        self.assertEqual(p['observation']['input_ids'],[list(range(1,174))])
        self.assertEqual(deps.tokenize_calls,1);self.assertEqual(deps.render_calls,1)
        r=generate(worker,p);self.assertIsNone(r['error'])
        self.assertIs(deps.received_inputs['input_ids'],deps.last_inputs['input_ids'])
        self.assertIs(deps.received_inputs['attention_mask'],deps.last_inputs['attention_mask'])
        self.assertEqual(deps.received_config.to_dict(),p['observation']['effective_generation_config'])
        self.assertEqual(r['reply']['output_tokens'],7);self.assertTrue(r['reply']['ended_eos'])
        self.assertEqual(r['observation']['continuation_ids'],[4]*6+[99]);self.assertEqual(deps.decoded_ids,[4]*6+[99])
        self.assertEqual(ready['runtime_sha256'],r['runtime_sha256'])
        self.assertEqual(generate(worker,p)['reply'],None);self.assertEqual(deps.generate_calls,1)
    def test_prepare_rejects_bad_shapes_without_generate(self):
        for fault in ('bool_id','float_id','batch','mask','bool_mask','too_long'):
            with self.subTest(fault=fault):
                worker,deps=backend(fault);r=prepare(worker)
                self.assertIsNone(r['reply']);self.assertIsNotNone(r['error']);self.assertEqual(deps.generate_calls,0)
    def test_mutation_and_quote_mismatch_refused(self):
        worker,deps=backend();p=prepare(worker);deps.last_inputs['input_ids'].values[0][0]=8
        r=generate(worker,p);self.assertIsNone(r['reply']);self.assertEqual(deps.generate_calls,0)
        worker,deps=backend();p=prepare(worker);p['reply']['input_tokens']+=1
        self.assertIsNone(generate(worker,p)['reply']);self.assertEqual(deps.generate_calls,0)
    def test_invalid_generation_and_exception_consume_handle(self):
        for fault in ('prefix','overcap','raise'):
            with self.subTest(fault=fault):
                worker,deps=backend(fault);p=prepare(worker);r=generate(worker,p)
                self.assertIsNone(r['reply']);self.assertIsNotNone(r['error']);self.assertEqual(deps.generate_calls,1)
                if fault in ('prefix','overcap'):
                    self.assertFalse(r['observation']['usage_known'])
                    self.assertIn('raw_output_ids',r['observation'])
                    self.assertNotIn('continuation_ids',r['observation'])
                self.assertIsNone(generate(worker,p)['reply']);self.assertEqual(deps.generate_calls,1)
    def test_decode_failure_preserves_known_usage_not_fabricated_reply(self):
        worker,deps=backend('decode');r=generate(worker,prepare(worker))
        self.assertIsNone(r['reply']);self.assertIsNotNone(r['error'])
        self.assertEqual(r['observation']['output_tokens'],7);self.assertEqual(r['observation']['continuation_ids'],[4]*6+[99])
        self.assertTrue(r['observation']['usage_known'])
    def test_eos_at_cap_counts_every_generated_id(self):
        worker,deps=backend('cap_eos');r=generate(worker,prepare(worker))
        self.assertEqual(r['reply']['output_tokens'],96);self.assertTrue(r['reply']['hit_cap']);self.assertTrue(r['reply']['ended_eos'])
    def test_discard_and_ready_copy(self):
        worker,deps=backend();r=worker.ready();r['observation']['model']='mutated'
        self.assertNotEqual(worker.ready()['observation']['model'],'mutated')
        p=prepare(worker);q=p['reply'];r=worker.handle({'op':'discard','handle':q['handle']})
        self.assertEqual(r['reply'],{'discarded':q['handle']});self.assertIsNone(generate(worker,p)['reply'])
        self.assertIsNotNone(prepare(worker)['reply'])
    def test_local_only_load_arguments_and_pinned_versions(self):
        worker,deps=backend()
        for _,_,args in deps.loads:
            self.assertTrue(args['local_files_only']);self.assertFalse(args['trust_remote_code']);self.assertFalse(args['token'])
        bad=FakeDependencies();bad.versions['transformers']='other'
        with self.assertRaises(ValueError):Backend(approval=InvocationApproval('offline_fake_dependencies'),dependency_factory=lambda:bad)
        self.assertFalse(bad.loads)

    def test_eight_frozen_slots_share_one_backend_without_retoke(self):
        from incident_preflight import calibration_plan
        worker,deps=backend();charged=0
        for slot in calibration_plan()['schedule']:
            p=worker.handle({'op':'prepare','request_utf8':slot['request_utf8']})
            self.assertIsNone(p['error'],p)
            r=generate(worker,p);self.assertIsNone(r['error'],r)
            charged+=r['reply']['input_tokens']+r['reply']['output_tokens']
        self.assertEqual(charged,1592);self.assertEqual(deps.generate_calls,8)
        self.assertEqual(deps.tokenize_calls,8);self.assertEqual(deps.render_calls,8)
    def test_observed_dtype_device_refused(self):
        for fault in ('dtype','device','load_failure','runtime_mismatch','eos_config'):
            with self.subTest(fault=fault),self.assertRaises((ValueError,RuntimeError)):backend(fault)
    def test_restart_foreign_handle_and_no_direct_generation(self):
        first,deps=backend();p=prepare(first);second,other=backend()
        self.assertIsNone(generate(second,p)['reply']);self.assertEqual(other.generate_calls,0)
        self.assertIsNone(first.handle({'model':'whatever'})['reply']);self.assertEqual(deps.generate_calls,0)
    def test_mutated_configuration_never_invokes_model(self):
        worker,deps=backend();p=prepare(worker)
        worker.handler.pending[2]['config'].max_new_tokens=95
        r=generate(worker,p);self.assertIsNone(r['reply']);self.assertEqual(deps.generate_calls,0)
    def test_output_batch_failure_is_not_a_measured_receipt(self):
        worker,deps=backend('output_batch');r=generate(worker,prepare(worker))
        self.assertIsNone(r['reply']);self.assertIsNotNone(r['error'])
        self.assertEqual(deps.generate_calls,1)

    def test_unknown_handle_and_bad_operation_never_retokenize_or_generate(self):
        worker,deps=backend();p=prepare(worker)
        for command in ({'op':'generate','handle':'0'*32,'input_tokens':173,'max_new_tokens':96},
                        {'op':'discard','handle':'0'*32}, {'op':'unknown'}, []):
            with self.subTest(command=command):
                response=worker.handle(command)
                self.assertIsNone(response['reply']);self.assertIsNotNone(response['error'])
        self.assertEqual(deps.tokenize_calls,1);self.assertEqual(deps.generate_calls,0)
        self.assertIsNotNone(generate(worker,p)['reply'])
    def test_effective_response_config_is_exact_independent_snapshot(self):
        worker,deps=backend();p=prepare(worker)
        config=p['observation']['effective_generation_config']
        self.assertEqual(config['max_new_tokens'],96);self.assertIs(config['do_sample'],False)
        self.assertEqual(config['num_beams'],1);self.assertEqual(config['num_return_sequences'],1)
        self.assertIs(config['return_dict_in_generate'],False)
        config['max_new_tokens']=1
        response=generate(worker,p)
        self.assertIsNone(response['error']);self.assertEqual(deps.received_config.max_new_tokens,96)

    def test_runtime_cpu_and_tokenizer_provenance_is_safe_and_synthetic(self):
        worker,deps=backend();runtime=worker.ready()['observation']
        self.assertEqual(runtime['cpu_identity'],{'source':'synthetic','model_names':['synthetic-cpu'],'flags':['synthetic'],'logical_processor_count':1})
        config=runtime['tokenizer_config']
        self.assertEqual(config['model_max_length'],2048);self.assertEqual(config['padding_side'],'right')
        self.assertEqual(config['special_tokens_map'],{'eos_token':'<eos>'})
        self.assertEqual(config['init_kwargs'],{'clean_up_tokenization_spaces':False,'add_bos_token':False})
        self.assertNotIn('private-cache',str(runtime));self.assertNotIn('secret',str(runtime))
        self.assertIn('name_or_path',config['omitted_init_keys']);self.assertIn('token',config['omitted_init_keys'])
    def test_cpu_parser_keeps_only_model_and_common_flags(self):
        from incident_transformers_backend import _parse_cpuinfo
        value=_parse_cpuinfo('processor: 0\nmodel name: CPU A\nflags: a b c\nSerial: secret\n\nprocessor: 1\nmodel name: CPU B\nflags: a c d\n')
        self.assertEqual(value,{'source':'linux_proc_cpuinfo','model_names':['CPU A','CPU B'],'flags':['a','c'],'logical_processor_count':2})
        for bad in ('','processor: 0\nflags: a b','processor: 0\nmodel name: CPU','model name: CPU\nflags:'):
            with self.subTest(bad=bad),self.assertRaises(ValueError):_parse_cpuinfo(bad)
    def test_missing_cpu_metadata_fails_closed_without_reading_live_host(self):
        from unittest.mock import patch
        from incident_transformers_backend import _cpu_identity
        with patch('incident_transformers_backend.Path.read_text',side_effect=FileNotFoundError('absent')):
            with self.assertRaises((ValueError,OSError)):_cpu_identity(False)
            self.assertEqual(_cpu_identity(True)['source'],'synthetic')

    def test_tokenizer_metadata_types_and_added_token_flags(self):
        from incident_transformers_backend import _tokenizer_config
        worker,deps=backend();tokenizer=worker.tokenizer
        tokenizer.model_max_length=True
        with self.assertRaises(ValueError):_tokenizer_config(tokenizer)
        tokenizer.model_max_length=2048
        AddedToken=type('AddedToken',(),{})
        item=AddedToken();item.content='<special>';item.single_word=False;item.lstrip=True
        item.rstrip=False;item.normalized=False;item.special=True
        tokenizer.special_tokens_map={'eos_token':item}
        value=_tokenizer_config(tokenizer)
        self.assertEqual(value['special_tokens_map']['eos_token']['added_token'],{'content':'<special>','single_word':False,'lstrip':True,'rstrip':False,'normalized':False,'special':True})
        item.lstrip=1
        with self.assertRaises(ValueError):_tokenizer_config(tokenizer)

if __name__=='__main__':unittest.main()
