"""Offline auditor regressions; optional real Rust fixture generation, no live calls."""
import hashlib
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
import zipfile

from audit_recovery_bundle import audit_bundle, validate_output, validate_request_envelope


class ArchivedBundleTests(unittest.TestCase):
    def test_frozen_live_bundles_reconcile_without_inference(self):
        root = Path(__file__).resolve().parents[1] / 'docs/evaluations'
        for version, digest, tokens in [
            ('v1', '067b7b00a8ecf725b11e900eb62ea81ec72c63616e994b3e62ddae4eafec57fa', 2978),
            ('v2', '74e8895335f304be0db1d8c705b1e448cb0588dd1b1ccff7296e4a2ff6f8af0d', 5241),
        ]:
            archive = root / f'recovery-policy-{version}-2026-11-14.zip'
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), digest)
            with tempfile.TemporaryDirectory() as folder, zipfile.ZipFile(archive) as bundle:
                for name in bundle.namelist():
                    self.assertFalse(Path(name).is_absolute())
                    self.assertNotIn('..', Path(name).parts)
                bundle.extractall(folder)
                result = audit_bundle(folder)
                self.assertEqual(result['status'], 'verified_complete')
                self.assertEqual(result['actual_backend_calls'], 20)
                self.assertEqual(result['charged_tokens'], tokens)
                self.assertEqual(result['outstanding_tokens'], 0)
                for policy in result['summary']['heldout'].values():
                    self.assertEqual(policy['success'], 0)


class AuditUnitTests(unittest.TestCase):
    def test_exact_validator_rejects_fences_extra_fields_and_bad_types(self):
        expected = {'release': 'r', 'tests': '1/2', 'status': 'fail'}
        self.assertTrue(validate_output(json.dumps(expected), expected))
        for text in ('```json\n' + json.dumps(expected) + '\n```',
                     json.dumps(expected | {'extra': 'x'}), json.dumps(expected | {'tests': 1}),
                     '{"release":"wrong","release":"r","tests":"1/2","status":"fail"}', '[]'):
            self.assertFalse(validate_output(text, expected))
        self.assertFalse(validate_output(json.dumps(expected), None))

    def test_live_envelope_matches_frozen_manifest_without_inference(self):
        manifest = dict(model='Qwen/Qwen2.5-1.5B-Instruct',
                        model_revision='989aa7980e4cf806f80c7fef2b1adb7bc71aa306',
                        generation=dict(do_sample=False, num_beams=1, max_new_tokens=96, dtype='bfloat16'))
        request = dict(adapter='transformers-cpu/v1', model=manifest['model'], revision=manifest['model_revision'],
                       dtype='bfloat16', add_generation_prompt=True, messages=[],
                       generation=dict(do_sample=False, num_beams=1, max_new_tokens=96))
        validate_request_envelope(request, manifest, 96, True)
        for key, value in [('model', 'wrong'), ('revision', 'wrong'), ('adapter', 'wrong'),
                           ('dtype', 'float32'), ('add_generation_prompt', False)]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_request_envelope(request | {key: value}, manifest, 96, True)
        for generation in ({'do_sample': True, 'num_beams': 1, 'max_new_tokens': 96},
                           {'do_sample': False, 'num_beams': True, 'max_new_tokens': 96},
                           {'do_sample': False, 'num_beams': 1, 'max_new_tokens': 95}):
            with self.assertRaises(ValueError):
                validate_request_envelope(request | {'generation': generation}, manifest, 96, True)
        with self.assertRaises(ValueError):
            validate_request_envelope(request, manifest | {'generation': {}}, 96, True)

    def test_missing_bundle_rejected_without_creating_any_files(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(ValueError):
                audit_bundle(Path(folder))
            self.assertEqual(list(Path(folder).iterdir()), [])


@unittest.skipUnless(os.environ.get('KAIROTENSOR_RECOVERY_RUNNER'), 'requires precompiled fixture runner')
class AuditBundleIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.folder = Path(self.tmp.name)
        subprocess.run([os.environ['KAIROTENSOR_RECOVERY_RUNNER'], '--trace', str(self.folder / 'trace.json')],
                       check=True, capture_output=True, timeout=120)

    def mutate_raw(self, change):
        path = self.folder / 'trace.json.raw.json'
        data = json.loads(path.read_text())
        change(data)
        path.write_text(json.dumps(data))

    def test_valid_bundle_is_audited_without_changing_any_bytes(self):
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in self.folder.iterdir()}
        result = audit_bundle(self.folder)
        self.assertEqual(result['status'], 'verified_complete')
        self.assertEqual(result['actual_backend_calls'], 18)
        self.assertEqual(result['new_model_calls'], 0)
        self.assertEqual(result['outstanding_tokens'], 0)
        self.assertEqual(result['cases'], 10)
        self.assertEqual(before, {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in self.folder.iterdir()})

    def test_tampered_usage_and_manifest_mode_rejected(self):
        self.mutate_raw(lambda d: d.update(actual_backend_calls=999))
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_tampered_event_quote_and_response_rejected(self):
        def corrupt(data):
            next(e for e in data['events'] if e['phase'] == 'received')['response'] = 'invented'
        self.mutate_raw(corrupt)
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_tampered_captured_payload_rejected(self):
        def corrupt(data):
            next(e for e in data['events'] if e['phase'] == 'prepared')['evidence'][0]['payload_utf8'] = '{}'
        self.mutate_raw(corrupt)
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_tampered_ledger_snapshot_rejected(self):
        def corrupt(data):
            next(e for e in data['events'] if e['phase'] == 'settled')['ledger']['spent'] += 1
        self.mutate_raw(corrupt)
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_sqlite_delivery_tamper_rejected(self):
        with closing(sqlite3.connect(self.folder / 'trace.json.sqlite')) as db, db:
            db.execute("UPDATE deliveries SET output=X'7B7D' WHERE attempt_id=1")
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_contradictory_manifest_generation_rejected(self):
        self.mutate_raw(lambda data: data['manifest'].update(generation={'do_sample': True}))
        with self.assertRaisesRegex(ValueError, 'generation'):
            audit_bundle(self.folder)

    def test_settled_stored_delivery_metadata_must_match_sqlite(self):
        def corrupt(data):
            next(e for e in data['events'] if e['phase'] == 'settled')['stored_delivery']['reason'] = 'wrong'
        self.mutate_raw(corrupt)
        with self.assertRaisesRegex(ValueError, 'stored delivery'):
            audit_bundle(self.folder)

    def test_corpus_cap_tampering_cannot_bypass_context_binding(self):
        def corrupt(data):
            data['manifest']['corpus']['output_cap'] = 95
            data['manifest']['corpus_utf8'] = json.dumps(data['manifest']['corpus'])
        self.mutate_raw(corrupt)
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_checkpointed_wal_bundle_is_refused_without_creating_sidecars(self):
        with closing(sqlite3.connect(self.folder / 'trace.json.sqlite')) as db, db:
            self.assertEqual(db.execute('PRAGMA journal_mode=WAL').fetchone()[0], 'wal')
            db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in self.folder.iterdir()}
        with self.assertRaisesRegex(ValueError, 'WAL'):
            audit_bundle(self.folder)
        self.assertEqual(before, {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in self.folder.iterdir()})

    def test_mutated_database_budget_is_rejected(self):
        with closing(sqlite3.connect(self.folder / 'trace.json.sqlite')) as db, db:
            db.execute('UPDATE token_budgets SET token_limit=1 WHERE id=1')
        with self.assertRaisesRegex(ValueError, 'database budget limit'):
            audit_bundle(self.folder)

    def test_unresolved_sent_hold_is_not_misreported_as_zero_cost(self):
        with closing(sqlite3.connect(self.folder / 'trace.json.sqlite')) as db, db:
            db.execute('DELETE FROM deliveries WHERE attempt_id=1')
        result = audit_bundle(self.folder)
        self.assertEqual(result['status'], 'incomplete_not_scored')
        self.assertEqual(result['unresolved_attempts'], 1)
        self.assertNotIn('charged_tokens', result)
        self.assertNotIn('summary', result)

    def test_v2_full_fixture_bundle_is_audited(self):
        target = self.folder / 'v2'
        target.mkdir()
        subprocess.run([os.environ['KAIROTENSOR_RECOVERY_RUNNER'], '--corpus-version', 'v2',
                        '--trace', str(target / 'trace.json')],
                       check=True, capture_output=True, timeout=120)
        self.assertEqual(audit_bundle(target)['status'], 'verified_complete')

    def test_tampered_timing_and_settled_correctness_rejected(self):
        def corrupt(data):
            next(e for e in data['events'] if e['phase'] == 'settled')['admission_ns'] += 2_000_000
        self.mutate_raw(corrupt)
        with self.assertRaises(ValueError):
            audit_bundle(self.folder)

    def test_incomplete_and_raw_only_bundles_are_not_scored(self):
        self.mutate_raw(lambda d: d.update(status='failed'))
        result = audit_bundle(self.folder)
        self.assertEqual(result['status'], 'incomplete_not_scored')
        self.assertNotIn('summary', result)
        (self.folder / 'trace.json').unlink()
        self.mutate_raw(lambda d: d.update(status='complete'))
        result = audit_bundle(self.folder)
        self.assertEqual(result['status'], 'incomplete_not_scored')


if __name__ == '__main__':
    unittest.main()
