import copy
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from incident_routing import ROOT, VERSION, load_workload, prepare, validate_output, audit, digest, read_json

class IncidentRoutingTests(unittest.TestCase):
    def setUp(self):
        self.workload = load_workload()
        self.prepared = prepare(self.workload, 'calibration')

    def response_fixture(self, prepared=None):
        prepared = prepared or self.prepared
        labels = {r['case_id']: r['expected'] for r in self.workload['labels']['labels']}
        return {'schema_version': 1, 'version': VERSION, 'mode': 'synthetic_response_fixture',
                'prepared_sha256': digest(prepared), 'complete': True,
                'responses': [{'case_id': r['case_id'], 'request_sha256': r['request_sha256'],
                               'state': 'received' if r['request'] is not None else 'not_requested',
                               'receipt_eligibility': 'current' if r['request'] is not None else None,
                               'output': json.dumps(labels[r['case_id']]) if r['request'] is not None else None}
                              for r in prepared['requests']]}

    def test_frozen_workload_and_calibration_only_artifacts(self):
        self.assertEqual(len(self.workload['cases']['cases']), 12)
        self.assertEqual(len(self.prepared['requests']), 4)
        self.assertEqual(self.prepared, prepare(self.workload, 'calibration'))
        for case in self.workload['cases']['cases'][4:]:
            for evidence in case['evidence']:
                self.assertNotIn(evidence['text'], json.dumps(self.prepared))

    def test_request_allowlist_excludes_gold_and_split_metadata(self):
        altered = copy.deepcopy(self.workload)
        altered['labels']['labels'][0]['expected']['reason'] = 'SECRET-GOLD-CANARY'
        self.assertEqual(self.prepared, prepare(altered, 'calibration'))
        for record in self.prepared['requests']:
            request = json.loads(record['request'])
            self.assertEqual(set(request), {'messages'})
            user = json.loads(request['messages'][1]['content'])
            self.assertEqual(set(user), {'question', 'owner_ids', 'evidence'})
            for forbidden in ['expected', 'family_id', 'split', 'case_id']:
                self.assertNotIn(forbidden, user)

    def test_digest_mutation_and_rewritten_freeze_fail_closed(self):
        for name in ['prompt.json', 'cases.json', 'labels.json', 'contract.json', 'freeze.json']:
            with tempfile.TemporaryDirectory() as tmp:
                target = Path(tmp) / 'corpus'
                shutil.copytree(ROOT, target)
                path = target / name
                path.write_bytes(path.read_bytes() + b' ')
                with self.assertRaises(ValueError): load_workload(target)

    def test_strict_protocol_validator_is_not_gold_oracle(self):
        case = self.workload['cases']['cases'][0]
        expected = copy.deepcopy(self.workload['labels']['labels'][0]['expected'])
        expected['owner'] = 'quartz'
        self.assertIsNotNone(validate_output(json.dumps(expected), case))
        response = self.response_fixture()
        response['responses'][0]['output'] = json.dumps(expected)
        score = audit(self.workload, self.prepared, response)
        self.assertTrue(score['scored'])
        self.assertFalse(score['rows'][0]['semantic_exact'])
        self.assertFalse(score['model_efficacy_claim'])

    def test_malformed_duplicate_unknown_citations_and_owner_rejected(self):
        case = self.workload['cases']['cases'][0]
        good = self.workload['labels']['labels'][0]['expected']
        variants = ['{}', '[]', 'null', json.dumps(good) + ' trailing',
                    '{"decision":"route","decision":"route"}', '{"decision":NaN}']
        for field, value in [('extra', 1), ('owner', 'unknown'), ('evidence_ids', ['missing']),
                             ('evidence_ids', ['e01', 'e01']), ('evidence_ids', []), ('reason', True)]:
            bad = dict(good); bad[field] = value; variants.append(json.dumps(bad))
        for raw in variants:
            self.assertIsNone(validate_output(raw, case), raw)
        response = self.response_fixture(); response['responses'][0]['output'] = '{}'
        result = audit(self.workload, self.prepared, response)
        self.assertTrue(result['scored']); self.assertFalse(result['rows'][0]['protocol_valid'])

    def test_complete_fixture_scoring_and_citation_set_order(self):
        responses = self.response_fixture()
        output = json.loads(responses['responses'][0]['output'])
        output['evidence_ids'].reverse()
        responses['responses'][0]['output'] = json.dumps(output)
        result = audit(self.workload, self.prepared, responses)
        self.assertTrue(result['scored']); self.assertEqual(result['summary']['joint_exact'], 4)
        self.assertEqual(result['summary']['current_joint_success'], 4)
        self.assertFalse(result['model_efficacy_claim'])

    def test_missing_unknown_incomplete_and_corrupt_bundle_never_score(self):
        mutations = [lambda r: r['responses'].pop(), lambda r: r.update(complete=False),
                     lambda r: r['responses'].append(r['responses'][0]),
                     lambda r: r['responses'][0].update(case_id='invented'),
                     lambda r: r['responses'][0].update(request_sha256='0' * 64),
                     lambda r: r['responses'][0].update(state='unknown', output=None, receipt_eligibility=None),
                     lambda r: r.update(extra=True)]
        for mutate in mutations:
            response = self.response_fixture(); mutate(response)
            result = audit(self.workload, self.prepared, response)
            self.assertFalse(result['scored']); self.assertIsNone(result['summary'])

    def test_all_selected_evidence_required_and_not_transmitted_on_refusal(self):
        for state in ['missing', 'stale', 'refused']:
            states = copy.deepcopy(self.prepared['states'])
            states[0]['evidence'][0]['state'] = state
            prepared = prepare(self.workload, 'calibration', states)
            self.assertIsNone(prepared['requests'][0]['request'])
            self.assertEqual(prepared['requests'][0]['refusal'], 'evidence_unavailable')
            score = audit(self.workload, prepared, self.response_fixture(prepared))
            self.assertTrue(score['scored']); self.assertEqual(score['summary']['not_requested'], 1)
            self.assertEqual(score['summary']['received'], 3)

    def test_stale_receipt_cannot_count_current_success(self):
        response = self.response_fixture(); response['responses'][0]['receipt_eligibility'] = 'stale'
        score = audit(self.workload, self.prepared, response)
        self.assertTrue(score['rows'][0]['semantic_exact'])
        self.assertFalse(score['rows'][0]['current_joint_success'])
        self.assertEqual(score['summary']['current_joint_success'], 3)

    def test_prepared_tampering_and_missing_evidence_state_rejected(self):
        corrupted = copy.deepcopy(self.prepared); corrupted['requests'][0]['request'] += ' '
        self.assertFalse(audit(self.workload, corrupted, self.response_fixture(corrupted))['scored'])
        states = copy.deepcopy(self.prepared['states']); states[0]['evidence'].pop()
        with self.assertRaises(ValueError): prepare(self.workload, 'calibration', states)

    def test_duplicate_json_keys_are_rejected_recursively(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'bad.json'
            path.write_text('{"a":{"x":1,"\\u0078":2}}')
            with self.assertRaises(ValueError): read_json(path)

    def test_duplicate_cases_split_family_and_evidence_leakage_rejected(self):
        mutations = [lambda w: w['cases']['cases'][1].update(case_id=w['cases']['cases'][0]['case_id']),
                     lambda w: w['cases']['cases'][4].update(family_id=w['cases']['cases'][0]['family_id']),
                     lambda w: w['cases']['cases'][0].update(split='unknown'),
                     lambda w: w['cases']['cases'][4]['evidence'][0].update(text=w['cases']['cases'][0]['evidence'][0]['text']),
                     lambda w: w['cases']['cases'][0]['evidence'].append(w['cases']['cases'][0]['evidence'][0])]
        for mutate in mutations:
            workload = copy.deepcopy(self.workload); mutate(workload)
            with self.assertRaises(ValueError): prepare(workload)

    def test_recomputed_file_manifest_cannot_redefine_frozen_version(self):
        import hashlib
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / 'corpus'; shutil.copytree(ROOT, target)
            content = target / 'prompt.json'; content.write_bytes(content.read_bytes() + b' ')
            freeze = read_json(target / 'freeze.json')
            freeze['sha256']['prompt.json'] = hashlib.sha256(content.read_bytes()).hexdigest()
            (target / 'freeze.json').write_text(json.dumps(freeze))
            with self.assertRaises(ValueError): load_workload(target)

    def test_citation_accuracy_separate_from_semantic_accuracy_and_splits(self):
        prepared = prepare(self.workload, 'all')
        response = self.response_fixture(prepared)
        answer = json.loads(response['responses'][0]['output'])
        answer['evidence_ids'] = ['e01', 'e03']
        response['responses'][0]['output'] = json.dumps(answer)
        result = audit(self.workload, prepared, response)
        self.assertTrue(result['rows'][0]['semantic_exact'])
        self.assertFalse(result['rows'][0]['citation_set_exact'])
        self.assertEqual(result['summary']['by_split']['calibration']['joint_exact'], 3)
        self.assertEqual(result['summary']['by_split']['heldout']['joint_exact'], 8)

    def test_cli_exclusive_preparation_live_refusal_and_unscored_incomplete(self):
        import subprocess
        import sys
        script = Path(__file__).with_name('incident_routing.py')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); out = root / 'prepared'
            args = [sys.executable, str(script), 'prepare', '--out', str(out)]
            self.assertEqual(subprocess.run(args, capture_output=True).returncode, 0)
            original = (out / 'prepared.json').read_bytes()
            self.assertNotEqual(subprocess.run(args, capture_output=True).returncode, 0)
            self.assertEqual((out / 'prepared.json').read_bytes(), original)
            denied = root / 'live'
            self.assertNotEqual(subprocess.run([sys.executable, str(script), 'prepare', '--out', str(denied), '--live'], capture_output=True).returncode, 0)
            self.assertFalse(denied.exists())
            response = self.response_fixture(); response['complete'] = False
            rp = root / 'responses.json'; rp.write_text(json.dumps(response))
            audit_run = subprocess.run([sys.executable, str(script), 'audit', '--prepared', str(out / 'prepared.json'), '--responses', str(rp)], capture_output=True, text=True)
            self.assertEqual(audit_run.returncode, 2)
            self.assertFalse(json.loads(audit_run.stdout)['scored'])

    def test_mutated_in_memory_gold_cannot_score_under_original_freeze(self):
        changed = copy.deepcopy(self.workload)
        changed['labels']['labels'][0]['expected']['owner'] = 'quartz'
        response = self.response_fixture()
        value = json.loads(response['responses'][0]['output']); value['owner'] = 'quartz'
        response['responses'][0]['output'] = json.dumps(value)
        result = audit(changed, self.prepared, response)
        self.assertFalse(result['scored']); self.assertIsNone(result['summary'])

    def test_excessive_json_nesting_is_controlled_refusal(self):
        raw = '[' * 10000 + '0' + ']' * 10000
        self.assertIsNone(validate_output(raw, self.workload['cases']['cases'][0]))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'deep.json'; path.write_text(raw)
            with self.assertRaises(ValueError): read_json(path)

    def test_malformed_metadata_shapes_never_escape_audit(self):
        bad_values = [None, [], {}, True, 0, 1.5]
        for key in self.response_fixture():
            for value in bad_values:
                original = self.response_fixture()[key]
                if type(value) is type(original) and value == original: continue
                response = self.response_fixture(); response[key] = value
                result = audit(self.workload, self.prepared, response)
                self.assertFalse(result['scored'], (key, value))
        for key in self.response_fixture()['responses'][0]:
            for value in bad_values:
                original = self.response_fixture()['responses'][0][key]
                if type(value) is type(original) and value == original: continue
                response = self.response_fixture(); response['responses'][0][key] = value
                result = audit(self.workload, self.prepared, response)
                self.assertFalse(result['scored'], (key, value))
        for key in self.prepared:
            for value in bad_values:
                original = self.prepared[key]
                if type(value) is type(original) and value == original: continue
                prepared = copy.deepcopy(self.prepared); prepared[key] = value
                response = self.response_fixture()
                result = audit(self.workload, prepared, response)
                self.assertFalse(result['scored'], (key, value))

if __name__ == '__main__': unittest.main()
