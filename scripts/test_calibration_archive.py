"""Audit the approved frozen calibration archive without model dependencies."""
import hashlib
import json
from pathlib import Path
import unittest
import zipfile

from recovery_calibration import canonical, parse_json, strict_validate


class CalibrationArchiveTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        path = Path(__file__).resolve().parents[1] / 'docs/evaluations/recovery-calibration-2026-11-14.zip'
        cls.archive = path.read_bytes()
        with zipfile.ZipFile(path) as bundle:
            cls.files = {name: bundle.read(name) for name in bundle.namelist()}
        cls.result = parse_json(cls.files['result.json'].decode())

    def test_original_zip_and_bounded_manifest_match(self):
        self.assertEqual(hashlib.sha256(self.archive).hexdigest(),
                         '7d982eeb43c4d5ac99f01b9b5081edb2da726defaff19b8978c19752dc2b4cc7')
        manifest = parse_json(self.files['manifest.json'].decode())
        self.assertEqual(set(self.files), {'result.json', 'environment.txt', 'stderr.log', 'stdout.log', 'manifest.json'})
        for entry in manifest['files']:
            self.assertEqual(entry['status'], 'copied')
            self.assertEqual(entry['bytes'], len(self.files[entry['name']]))
        self.assertLessEqual(sum(e['bytes'] for e in manifest['files']), manifest['uncompressed_payload_limit'])

    def test_eight_cell_plan_and_provenance_reconcile(self):
        result = self.result
        self.assertEqual(result['status'], 'complete')
        self.assertEqual(result['mode'], 'live_controlled_calibration')
        for key in ('new_model_calls', 'confirmed_model_calls', 'potential_model_calls',
                    'backend_calls_started', 'backend_calls_completed'):
            self.assertEqual(result[key], 8)
        self.assertEqual(result['unresolved_backend_calls'], 0)
        manifest = result['manifest']
        protocol = manifest['protocol']
        self.assertEqual(parse_json(manifest['protocol_utf8']), protocol)
        self.assertEqual(hashlib.sha256(manifest['protocol_utf8'].encode()).hexdigest(), manifest['protocol_sha256'])
        self.assertEqual(hashlib.sha256(manifest['prompt_utf8'].encode()).hexdigest(), protocol['prompt_sha256'])
        self.assertEqual(protocol['model'], 'Qwen/Qwen2.5-1.5B-Instruct')
        self.assertEqual(protocol['revision'], '989aa7980e4cf806f80c7fef2b1adb7bc71aa306')
        self.assertEqual(manifest['machine']['cpu_model'], 'AMD EPYC 7763 64-Core Processor')
        self.assertEqual([{k: c[k] for k in ('case_id', 'dtype', 'format')} for c in result['calls']], protocol['call_order'])

    def test_usage_constraints_and_strict_outcomes_reconcile(self):
        result = self.result
        cases = {c['case_id']: c for c in result['manifest']['protocol']['cases']}
        totals = {}
        for call in result['calls']:
            self.assertEqual(call['status'], 'complete')
            self.assertIs(call['generation_invoked'], True)
            self.assertEqual(call['input_tokens'], len(call['input_ids']))
            self.assertEqual(call['input_ids_sha256'], hashlib.sha256(canonical(call['input_ids']).encode()).hexdigest())
            reply = call['raw_response']
            self.assertEqual(call['output_tokens'], len(reply['completion_ids']))
            self.assertEqual(call['charged_tokens'], call['input_tokens'] + call['output_tokens'])
            self.assertTrue(call['ended_eos'])
            self.assertFalse(call['hit_cap'])
            correct = strict_validate(reply['text'], cases[call['case_id']]['reference'])
            self.assertEqual(call['correct'], correct)
            totals.setdefault((call['dtype'], call['format']), []).append(correct)
            if call['format'] == 'candidate_trie':
                index = call['candidate_strings'].index(reply['text'])
                self.assertEqual(reply['completion_ids'][:-1], call['candidate_token_ids'][index])
                self.assertIn(reply['completion_ids'][-1], call['eos_token_ids'])
            else:
                self.assertTrue(reply['text'].startswith('```json\n'))
        self.assertEqual(sum(c['charged_tokens'] for c in result['calls']), 1640)
        for (dtype, mode), outcomes in totals.items():
            self.assertEqual(len(outcomes), 2)
            self.assertEqual(sum(outcomes), 1 if mode == 'candidate_trie' else 0)


if __name__ == '__main__':
    unittest.main()
