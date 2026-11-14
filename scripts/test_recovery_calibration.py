"""Offline contracts for the bounded calibration lab. Never load a model."""
import json
from pathlib import Path
import unittest

import recovery_calibration as lab


class CalibrationLabTests(unittest.TestCase):
    def test_plan_has_eight_unique_cells_and_only_two_calibration_cases(self):
        protocol = lab.load_protocol()
        self.assertEqual({case['case_id'] for case in protocol['cases']}, {'cal-pass', 'cal-repair'})
        plan = lab.call_plan(protocol)
        self.assertEqual(len(plan), 8)
        self.assertEqual(len({(cell['case_id'], cell['dtype'], cell['format']) for cell in plan}), 8)
        self.assertEqual({cell['dtype'] for cell in plan}, {'bfloat16', 'float32'})
        self.assertEqual({cell['format'] for cell in plan}, {'free', 'candidate_trie'})

    def test_protocol_matches_pins_and_existing_calibration_without_loading_heldout_into_artifacts(self):
        protocol = lab.load_protocol()
        self.assertEqual(protocol['model'], 'Qwen/Qwen2.5-1.5B-Instruct')
        self.assertEqual(protocol['revision'], '989aa7980e4cf806f80c7fef2b1adb7bc71aa306')
        self.assertEqual(protocol['required_torch'], '2.6.0+cpu')
        self.assertEqual(protocol['required_transformers'], '4.48.3')
        self.assertEqual(lab.digest(lab.PROTOCOL_PATH.read_bytes()), lab.PROTOCOL_SHA256)
        self.assertEqual(lab.digest(lab.PROMPT_PATH.read_bytes()), protocol['prompt_sha256'])
        corpus = json.loads((lab.ROOT / 'examples/recovery_policy_eval/corpus-v1.json').read_text())
        calibration = {c['case_id']: c for c in corpus['cases'] if c['split'] == 'calibration'}
        for case in protocol['cases']:
            self.assertEqual(case['reports'], calibration[case['case_id']]['reports'])
            self.assertEqual(case['reference'], calibration[case['case_id']]['initial_reference'])

    def test_candidates_render_evidence_but_leave_status_to_model(self):
        case = lab.load_protocol()['cases'][1]
        candidates = lab.structured_candidates(case['reports'], case['admitted'])
        self.assertEqual([json.loads(c)['status'] for c in candidates], ['pass', 'fail'])
        self.assertTrue(all(json.loads(c)['release'] == 'cal-B' for c in candidates))
        self.assertTrue(all(json.loads(c)['tests'] == '11/12' for c in candidates))
        self.assertEqual(len(candidates), 2)
        self.assertEqual(lab.structured_candidates(case['reports'], True), candidates)

    def test_strict_validator_remains_strict(self):
        expected = lab.load_protocol()['cases'][0]['reference']
        valid = json.dumps(expected)
        self.assertTrue(lab.strict_validate(valid, expected))
        for bad in ('```json\n' + valid + '\n```', valid + ' prose', '{}',
                    '{"release":"cal-A","release":"cal-A","tests":"12/12","status":"pass"}'):
            self.assertFalse(lab.strict_validate(bad, expected))

class CandidateTrieTests(unittest.TestCase):
    def setUp(self):
        self.tokenizer = lab.ByteTokenizer()
        self.case = lab.load_protocol()['cases'][1]
        self.candidates = lab.structured_candidates(self.case['reports'], True)
        self.trie = lab.CandidateTrie(self.tokenizer, self.candidates, [256], 96)

    def test_every_candidate_prefix_is_valid_and_eos_only_terminal(self):
        for path in self.trie.token_paths:
            for index, token in enumerate(path):
                allowed = self.trie.allowed(path[:index])
                self.assertIn(token, allowed)
                self.assertNotIn(256, allowed)
            self.assertEqual(self.trie.allowed(path), [256])
            text = self.tokenizer.decode(list(path))
            self.trie.verify(list(path) + [256], text)
        with self.assertRaisesRegex(ValueError, 'departed'):
            self.trie.allowed([999])

    def test_callback_preserves_exact_prompt_and_rejects_unknown_batch(self):
        callback = self.trie.callback([700, 701])
        self.assertEqual(callback(0, [700, 701]), self.trie.allowed([]))
        with self.assertRaisesRegex(ValueError, 'prompt'):
            callback(0, [700, 702])
        with self.assertRaisesRegex(ValueError, 'one generation'):
            callback(1, [700, 701])

    def test_swapped_candidate_text_or_missing_eos_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'token path'):
            self.trie.verify(list(self.trie.token_paths[0]) + [256], self.candidates[1])
        with self.assertRaisesRegex(ValueError, 'EOS'):
            self.trie.verify(list(self.trie.token_paths[0]), self.candidates[0])
        with self.assertRaisesRegex(ValueError, 'complete candidate'):
            self.trie.verify([123, 256], self.candidates[0])

    def test_cap_and_roundtrip_fail_before_generation(self):
        with self.assertRaisesRegex(ValueError, 'exceeds'):
            lab.CandidateTrie(self.tokenizer, self.candidates, [256], 2)
        class ChangedDecode(lab.ByteTokenizer):
            def decode(self, tokens, **kwargs):
                return super().decode(tokens, **kwargs) + ' '
        with self.assertRaisesRegex(ValueError, 'round-trip'):
            lab.CandidateTrie(ChangedDecode(), self.candidates, [256], 96)
        with self.assertRaisesRegex(ValueError, 'EOS before'):
            lab.CandidateTrie(self.tokenizer, self.candidates, [123], 96)

    def test_invalid_or_conflicting_evidence_is_refused_without_reference_input(self):
        good = self.case['reports']
        bad_reports = [[], good + good, [{'release': 'x', 'passed': True, 'total': 3}],
                       [{'release': 'x', 'passed': 4, 'total': 3}],
                       [{'release': '', 'passed': 1, 'total': 3}],
                       [{'release': 'x', 'passed': 0, 'total': 0}],
                       [{'release': 'x', 'passed': 1}],
                       [{'release': 'x', 'passed': 1, 'total': 3, 'status': 'pass'}]]
        for reports in bad_reports:
            with self.subTest(reports=reports), self.assertRaises(ValueError):
                lab.structured_candidates(reports, True)
        with self.assertRaises(ValueError):
            lab.structured_candidates(good, False)


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        import tempfile
        self.directory = tempfile.TemporaryDirectory()
        self.output = Path(self.directory.name) / 'result.json'

    def tearDown(self):
        self.directory.cleanup()

    def test_fixture_exactly_eight_calls_four_profiles_and_no_oracle_status(self):
        result = lab.run(self.output)
        self.assertEqual(result['status'], 'complete')
        self.assertEqual(result['new_model_calls'], 0)
        self.assertEqual(result['backend_calls_started'], 8)
        self.assertEqual(result['backend_calls_completed'], 8)
        self.assertEqual(len(result['latency_calibration']), 4)
        self.assertEqual(len(result['admission_diagnostics']), 8)
        self.assertEqual(json.loads(self.output.read_text()), result)
        self.assertEqual(json.loads(result['manifest']['protocol_utf8']), result['manifest']['protocol'])
        self.assertEqual(lab.digest(result['manifest']['protocol_utf8']), result['manifest']['protocol_sha256'])
        loaded = [e for e in result['events'] if e['phase'] == 'loaded']
        self.assertEqual([e['dtype'] for e in loaded], ['bfloat16', 'float32'])
        self.assertTrue(all(e['metadata']['model_loaded'] is False for e in loaded))
        for call in result['calls']:
            self.assertEqual(call['raw_response']['effective_generation_config'], call['effective_generation_config'])
            self.assertEqual(lab.digest(lab.canonical(call['input_ids'])), call['input_ids_sha256'])
            self.assertEqual(lab.digest(call['rendered_chat']), call['rendered_chat_sha256'])
            self.assertEqual(call['charged_tokens'], call['input_tokens'] + call['output_tokens'])
            self.assertTrue(call['generation_invoked'])
            self.assertTrue(call['ended_eos'])
            self.assertFalse(call['hit_cap'])
            self.assertEqual(call['correct'], call['format'] == 'candidate_trie' and call['case_id'] == 'cal-pass')
            user = json.loads(call['messages'][1]['content'])
            self.assertEqual(set(user), {'ci_reports'})
            self.assertNotIn('reference', call['request_utf8'])
        for estimate in result['latency_calibration']:
            self.assertEqual(estimate['sample_count'], 2)
            self.assertGreater(estimate['max_duration_ms'], 0)
        for decision in result['admission_diagnostics']:
            self.assertEqual(decision['new_generation_calls'], 0)
            self.assertEqual(decision['kind'], 'hypothetical_post_preparation_window')
        self.assertEqual({case['case_id'] for case in result['manifest']['protocol']['cases']}, {'cal-pass', 'cal-repair'})
        self.assertNotIn('fresh-301', self.output.read_text())

    def test_destination_reuse_refused_before_initializing_backend(self):
        self.output.write_text('existing')
        called = []
        with self.assertRaises(FileExistsError):
            lab.run(self.output, backend_factory=lambda p: called.append(p))
        self.assertEqual(called, [])
        self.assertEqual(self.output.read_text(), 'existing')

    def test_generation_error_records_uncertain_started_call_and_does_not_retry(self):
        class Lost(lab.FixtureBackend):
            def generate(self, *args):
                raise RuntimeError('lost response')
        with self.assertRaisesRegex(RuntimeError, 'lost response'):
            lab.run(self.output, backend_factory=Lost)
        result = json.loads(self.output.read_text())
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['backend_calls_started'], 1)
        self.assertEqual(result['backend_calls_completed'], 0)
        self.assertEqual(result['calls'][0]['status'], 'started')
        self.assertIsNone(result['calls'][0]['generation_invoked'])
        self.assertTrue(result['calls'][0]['backend_invoked'])
        self.assertEqual(result['unresolved_backend_calls'], 1)
        self.assertNotIn('charged_tokens', result['calls'][0])

    def test_live_backend_exception_does_not_confirm_native_model_entry_or_usage(self):
        for message in ('config failed before native generation', 'native response lost'):
            class Failed(lab.FixtureBackend):
                def generate(self, *args):
                    raise RuntimeError(message)
            with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                lab.run(self.output, live=True, backend_factory=Failed)
            artifact = json.loads(self.output.read_text())
            self.assertIsNone(artifact['new_model_calls'])
            self.assertEqual(artifact['confirmed_model_calls'], 0)
            self.assertEqual(artifact['potential_model_calls'], 1)
            self.assertEqual(artifact['unresolved_backend_calls'], 1)
            self.assertTrue(artifact['calls'][0]['backend_invoked'])
            self.assertIsNone(artifact['calls'][0]['generation_invoked'])
            self.assertNotIn('charged_tokens', artifact['calls'][0])
            self.output.unlink()

    def test_load_failure_cleans_up_and_preserves_partial_artifact(self):
        cleaned = []
        class BrokenLoad(lab.FixtureBackend):
            def load(self, dtype):
                self.dtype = dtype
                raise RuntimeError('out of memory')
            def unload(self):
                cleaned.append(self.dtype)
                super().unload()
        with self.assertRaisesRegex(RuntimeError, 'out of memory'):
            lab.run(self.output, backend_factory=BrokenLoad)
        self.assertEqual(cleaned, ['bfloat16'])
        result = json.loads(self.output.read_text())
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['backend_calls_started'], 0)
        self.assertIn('out of memory', result['error'])

    def test_changed_rendered_chat_or_generation_config_fails_closed(self):
        class BadPrepare(lab.FixtureBackend):
            def prepare(self, messages):
                prepared = super().prepare(messages)
                prepared['rendered_chat'] += ' altered'
                return prepared
        with self.assertRaisesRegex(ValueError, 'rendered chat'):
            lab.run(self.output, backend_factory=BadPrepare)
        self.assertEqual(json.loads(self.output.read_text())['backend_calls_started'], 0)
        self.output.unlink()
        class BadConfig(lab.FixtureBackend):
            def generate(self, *args):
                response = super().generate(*args)
                response['effective_generation_config'] = {'max_new_tokens': 1000}
                return response
        with self.assertRaisesRegex(ValueError, 'settings changed'):
            lab.run(self.output, backend_factory=BadConfig)
        self.assertEqual(json.loads(self.output.read_text())['backend_calls_completed'], 0)

    def test_flush_crossing_walltime_never_invokes_generation(self):
        from unittest.mock import patch
        elapsed = [0]
        invoked = []
        original = lab.Journal.event
        def event(journal, phase, **fields):
            original(journal, phase, **fields)
            if phase == 'generation_started':
                elapsed[0] = 1500 * 1_000_000_000
        class Tracking(lab.FixtureBackend):
            def generate(self, *args):
                invoked.append(True)
                return super().generate(*args)
        with patch.object(lab.Journal, 'event', event), self.assertRaisesRegex(ValueError, 'walltime'):
            lab.run(self.output, backend_factory=Tracking, clock=lambda: elapsed[0])
        self.assertEqual(invoked, [])
        result = json.loads(self.output.read_text())
        self.assertFalse(result['calls'][0]['generation_invoked'])
        self.assertEqual(result['new_model_calls'], 0)

    def test_hard_exit_during_generate_preserves_unknown_invocation_not_false_zero(self):
        import subprocess
        import sys
        program = """
import os,sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import recovery_calibration as lab
class Crash(lab.FixtureBackend):
    def generate(self,*args):
        os._exit(71)
lab.run(Path(sys.argv[2]),live=True,backend_factory=Crash)
"""
        completed = subprocess.run([sys.executable, '-c', program, str(lab.ROOT / 'scripts'), str(self.output)],
                                   capture_output=True, text=True)
        self.assertEqual(completed.returncode, 71, completed.stderr)
        result = json.loads(self.output.read_text())
        self.assertEqual(result['status'], 'running')
        self.assertIsNone(result['new_model_calls'])
        self.assertEqual(result['confirmed_model_calls'], 0)
        self.assertEqual(result['potential_model_calls'], 1)
        self.assertEqual(result['unresolved_backend_calls'], 1)
        self.assertIsNone(result['calls'][0]['generation_invoked'])
        self.assertNotIn('charged_tokens', result['calls'][0])

    def test_live_unknown_hardware_fails_before_backend_initialization(self):
        from unittest.mock import patch
        unknown = lab.machine_metadata()
        unknown.update(cpu_model=None, flags=[], affinity=None)
        initialized = []
        with patch.object(lab, 'machine_metadata', return_value=unknown), self.assertRaisesRegex(ValueError, 'known CPU'):
            lab.run(self.output, live=True, backend_factory=lambda p: initialized.append(p))
        self.assertEqual(initialized, [])
        artifact = json.loads(self.output.read_text())
        self.assertEqual(artifact['status'], 'failed')
        self.assertEqual(artifact['new_model_calls'], 0)

    def test_load_journal_flush_crossing_walltime_prevents_model_load(self):
        from unittest.mock import patch
        elapsed = [0]
        loaded = []
        original = lab.Journal.event
        def event(journal, phase, **fields):
            original(journal, phase, **fields)
            if phase == 'load_started':
                elapsed[0] = 1500 * 1_000_000_000
        class Tracking(lab.FixtureBackend):
            def load(self, dtype):
                loaded.append(dtype)
                return super().load(dtype)
        with patch.object(lab.Journal, 'event', event), self.assertRaisesRegex(ValueError, 'walltime'):
            lab.run(self.output, backend_factory=Tracking, clock=lambda: elapsed[0])
        self.assertEqual(loaded, [])
        self.assertEqual(json.loads(self.output.read_text())['backend_calls_started'], 0)

    def test_hard_eight_call_limit_survives_an_invalid_extended_plan(self):
        from unittest.mock import patch
        protocol = lab.load_protocol()
        extended = lab.call_plan(protocol)
        extended.append(dict(extended[-1], ordinal=9))
        invoked = []
        class Tracking(lab.FixtureBackend):
            def generate(self, *args):
                invoked.append(True)
                return super().generate(*args)
        with patch.object(lab, 'call_plan', return_value=extended), self.assertRaisesRegex(ValueError, 'eight-call'):
            lab.run(self.output, backend_factory=Tracking)
        self.assertEqual(len(invoked), 8)
        self.assertEqual(json.loads(self.output.read_text())['backend_calls_started'], 8)

    def test_fixture_does_not_import_torch_or_transformers(self):
        from unittest.mock import patch
        import builtins
        original = builtins.__import__
        def guarded(name, *args, **kwargs):
            if name.split('.')[0] in ('torch', 'transformers'):
                raise AssertionError('fixture tried to import a model runtime')
            return original(name, *args, **kwargs)
        with patch('builtins.__import__', guarded):
            lab.run(self.output)


if __name__ == '__main__':
    unittest.main()
