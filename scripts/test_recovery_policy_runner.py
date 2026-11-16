"""Opt-in end-to-end checks against a precompiled Rust fixture collector.

Build the recovery_policy_eval example separately, then set
KAIROTENSOR_RECOVERY_RUNNER to its executable and run unittest discovery.
These tests never invoke cargo, install dependencies, or request live inference.
"""
import json
import hashlib
from contextlib import closing, contextmanager
from datetime import datetime
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

from recovery_policy_eval import evaluate

RUNNER = os.environ.get('KAIROTENSOR_RECOVERY_RUNNER')


@unittest.skipUnless(RUNNER, 'set KAIROTENSOR_RECOVERY_RUNNER to a precompiled fixture collector')
class RecoveryPolicyRunnerTests(unittest.TestCase):
    collector_args = ()
    corpus_id = 'ci-report-recovery-v1'
    expected_cases = 10
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.trace_path = Path(cls.directory.name, 'trace.json')
        cls.command = [str(Path(RUNNER).resolve()), '--trace', str(cls.trace_path), *cls.collector_args]
        completed = subprocess.run(cls.command, text=True, capture_output=True, timeout=120)
        if completed.returncode:
            cls.directory.cleanup()
            raise AssertionError(f'fixture collector failed: {completed.stderr}\n{completed.stdout}')
        cls.trace = json.loads(cls.trace_path.read_text())
        cls.raw_path = Path(str(cls.trace_path) + '.raw.json')
        cls.db_path = Path(str(cls.trace_path) + '.sqlite')
        cls.raw = json.loads(cls.raw_path.read_text())
        cls.report = evaluate(cls.trace)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    @contextmanager
    def database(self):
        with closing(sqlite3.connect(f'{self.db_path.as_uri()}?mode=ro', uri=True)) as db, db:
            db.row_factory = sqlite3.Row
            yield db

    def test_collected_trace_scores_and_affordable_totals_reconcile_sqlite(self):
        self.assertEqual(self.trace['schema_version'], 2)
        self.assertEqual(self.report['mode'], 'synthetic_policy_fixture')
        self.assertEqual(self.report['new_model_calls'], 0)
        self.assertEqual(len(self.trace['cases']), self.expected_cases)
        rows = [r for r in self.report['cases'] if r['policy'] == 'affordable_retry']
        with self.database() as db:
            self.assertEqual(sum(r['attempts'] for r in rows), db.execute('SELECT COUNT(*) FROM dispatches').fetchone()[0])
            self.assertEqual(sum(r['charged_tokens'] for r in rows), db.execute('SELECT COALESCE(SUM(usage),0) FROM deliveries').fetchone()[0])
            held = db.execute("""SELECT COALESCE(SUM(t.input_tokens+t.output_cap),0)
                FROM token_reservations t JOIN recovery_attempts r ON r.attempt_id=t.attempt_id
                LEFT JOIN deliveries d ON d.attempt_id=t.attempt_id
                WHERE t.granted=1 AND d.attempt_id IS NULL AND r.state IN ('reserved','sent')""").fetchone()[0]
            self.assertEqual(sum(r['reserved_tokens'] for r in rows), held)
            self.assertEqual(db.execute('PRAGMA foreign_key_check').fetchall(), [])

    def test_each_recorded_response_matches_dispatch_quote_delivery_and_captured_payload(self):
        with self.database() as db:
            checked = set()
            for case in self.trace['cases']:
                cells = [(case['initial_context'], case['initial'])]
                retry = case['retries']['affordable_retry']
                if retry is not None:
                    cells.append((retry['context'], retry['attempt']))
                for context, attempt in cells:
                    if attempt is None:
                        continue
                    matches = db.execute("""SELECT a.id, a.view_epoch, x.request, t.input_tokens, t.output_cap,
                        t.granted, d.disposition, d.usage, d.output, r.ordinal, w.scope, w.actor
                        FROM attempts a JOIN dispatches x ON x.attempt_id=a.id
                        JOIN token_reservations t ON t.attempt_id=a.id
                        JOIN recovery_attempts r ON r.attempt_id=a.id
                        JOIN recovery_runs w ON w.id=r.recovery_id
                        LEFT JOIN deliveries d ON d.attempt_id=a.id
                        WHERE x.request=?""", (context['request'].encode('utf-8'),)).fetchall()
                    self.assertEqual(len(matches), 1, case['case_id'])
                    stored = matches[0]
                    self.assertNotIn(stored['id'], checked)
                    checked.add(stored['id'])
                    self.assertEqual(stored['input_tokens'], context['input_tokens'])
                    self.assertEqual(stored['output_cap'], context['output_cap'])
                    self.assertEqual(stored['granted'], 1)
                    self.assertEqual(stored['usage'], attempt['charged_tokens'])
                    self.assertEqual(stored['disposition'], attempt['disposition'])
                    # Fixture tokenization is UTF-8 bytes, not model tokens.
                    self.assertEqual(stored['usage'], len(stored['request']) + len(stored['output']))
                    request = json.loads(context['request'])
                    user = next(m for m in request['messages'] if m['role'] == 'user')
                    payload = json.loads(user['content'])
                    evidence = db.execute("""SELECT e.payload FROM attempt_inputs i
                        JOIN evidence e ON e.id=i.evidence_id WHERE i.attempt_id=? ORDER BY i.position""", (stored['id'],)).fetchall()
                    self.assertEqual(payload['ci_reports'], [json.loads(r['payload']) for r in evidence])
                    self.assertEqual('validation_error' in payload, stored['ordinal'] == 1)
            self.assertEqual(len(checked), db.execute('SELECT COUNT(*) FROM dispatches').fetchone()[0])

    def test_raw_manifest_and_event_ledger_reconcile_the_actual_execution(self):
        self.assertEqual(self.raw['artifact_version'], 1)
        self.assertEqual(self.raw['status'], 'complete')
        self.assertEqual(self.raw['mode'], 'deterministic_byte_token_fixture')
        self.assertEqual(self.raw['new_model_calls'], 0)
        self.assertEqual(self.raw['completed_cases'], self.trace['cases'])
        manifest = self.raw['manifest']
        self.assertEqual(json.loads(manifest['corpus_utf8']), manifest['corpus'])
        self.assertEqual(Path(manifest['database']), self.db_path)
        self.assertIsNone(manifest['model'])
        self.assertIsNone(manifest['model_revision'])
        self.assertIsNone(manifest['tokenizer_revision'])
        self.assertEqual(manifest['corpus']['corpus_id'], self.corpus_id)
        if self.corpus_id == 'ci-report-recovery-v1':
            self.assertEqual(manifest['system_prompt'], 'Read ci_reports. Return only JSON string fields release, tests, status. Use a report only when exactly one complete report is present. Copy release. Format tests as passed/total. status is pass only when passed equals total; otherwise fail. Do not invent a verdict for conflicting or incomplete reports.')
            self.assertEqual(manifest['feedback_prompt'], 'Return only complete JSON matching the current admitted reports: release, passed/total tests, and pass/fail status. Do not choose between conflicting reports.')
        self.assertEqual({c['case_id'] for c in manifest['corpus']['cases']},
                         {c['case_id'] for c in self.trace['cases']})
        events = self.raw['events']
        receipts = [e for e in events if e['phase'] == 'received']
        self.assertEqual(self.raw['actual_backend_calls'], len(receipts))
        with self.database() as db:
            self.assertEqual(len(receipts), db.execute('SELECT COUNT(*) FROM dispatches').fetchone()[0])
            for case in self.trace['cases']:
                case_events = [e for e in events if e['case_id'] == case['case_id']]
                self.assertEqual(case_events[0]['phase'], 'case_started')
                self.assertEqual(case_events[-1]['phase'], 'case_completed')
                start = case_events[0]
                binding = db.execute('SELECT budget_id,recovery_key FROM recovery_runs WHERE id=?',
                                     (start['recovery_id'],)).fetchone()
                self.assertEqual(binding['budget_id'], start['budget_id'])
                self.assertEqual(binding['recovery_key'], self.corpus_id + '/' + case['case_id'])
                spent, held = 0, 0
                previous_receipt_ns = 0
                phases = {}
                expected_cells = [(case['initial_context'], case['initial'])]
                if case['retries']['affordable_retry'] is not None:
                    retry = case['retries']['affordable_retry']
                    expected_cells.append((retry['context'], retry['attempt']))
                observed_cells = []
                for event in case_events:
                    phase = event['phase']
                    attempt_id = event.get('attempt_id')
                    if attempt_id is not None:
                        phases.setdefault(attempt_id, []).append(phase)
                    if phase == 'prepared':
                        captured = db.execute("""SELECT e.id,e.payload FROM attempt_inputs i
                            JOIN evidence e ON e.id=i.evidence_id WHERE i.attempt_id=? ORDER BY i.position""",
                            (attempt_id,)).fetchall()
                        self.assertEqual(event['evidence'], [dict(id=r['id'], payload_utf8=r['payload'].decode()) for r in captured])
                    elif phase == 'claimed':
                        quote = db.execute('SELECT input_tokens+output_cap FROM token_reservations WHERE attempt_id=?',
                                           (attempt_id,)).fetchone()[0]
                        held += quote
                    elif phase == 'received':
                        delivery = db.execute('SELECT output,usage FROM deliveries WHERE attempt_id=?', (attempt_id,)).fetchone()
                        self.assertEqual(delivery['output'].decode(), event['response'])
                        self.assertEqual(delivery['usage'], event['input_tokens'] + event['output_tokens'])
                        self.assertTrue(event['details']['synthetic_response'])
                    elif phase == 'settled':
                        observed_cells.append((event['context'], event['attempt']))
                        quote = event['context']['input_tokens'] + event['context']['output_cap']
                        held -= quote
                        spent += event['attempt']['charged_tokens']
                        self.assertGreaterEqual(event['admission_ns'], previous_receipt_ns)
                        self.assertGreaterEqual(event['receipt_ns'], event['admission_ns'])
                        previous_receipt_ns = event['receipt_ns']
                    if 'ledger' in event:
                        self.assertEqual(event['ledger'], dict(budget_id=start['budget_id'],
                            limit=case['budget_tokens'], spent=spent, reserved=held))
                self.assertEqual(observed_cells, expected_cells)
                for sequence in phases.values():
                    self.assertEqual(sequence, ['prepared', 'claimed', 'received', 'settled'])
                charged = db.execute("""SELECT COALESCE(SUM(d.usage),0) FROM deliveries d
                    JOIN token_reservations t ON t.attempt_id=d.attempt_id WHERE t.budget_id=?""",
                    (start['budget_id'],)).fetchone()[0]
                self.assertEqual(charged, spent)
                self.assertEqual(held, 0)

    def test_validator_matches_independent_reference_without_relaxation(self):
        manifest = self.raw['manifest']
        self.assertEqual(manifest['validator_version'], 'independent-report-verdict/1')
        self.assertEqual(manifest['corpus']['validator_version'], 'independent-report-verdict/1')
        definitions = {c['case_id']: c for c in manifest['corpus']['cases']}
        with self.database() as db:
            for event in self.raw['events']:
                if event['phase'] != 'settled':
                    continue
                row = db.execute("""SELECT r.ordinal,d.output,d.disposition FROM recovery_attempts r
                    JOIN deliveries d ON d.attempt_id=r.attempt_id WHERE r.attempt_id=?""",
                    (event['attempt_id'],)).fetchone()
                reference_key = 'initial_reference' if row['ordinal'] == 0 else 'retry_reference'
                expected = definitions[event['case_id']][reference_key]
                try:
                    parsed = json.loads(row['output'])
                except (ValueError, UnicodeError):
                    parsed = None
                correct = (expected is not None and type(parsed) is dict
                           and set(parsed) == {'release', 'tests', 'status'}
                           and all(type(v) is str for v in parsed.values()) and parsed == expected)
                self.assertEqual(event['attempt']['correct'], correct)
                self.assertEqual(row['disposition'] == 'accepted', correct)

    def test_same_destination_refused_without_mutating_existing_artifacts(self):
        paths = [self.trace_path, self.raw_path, self.db_path]
        before = [path.read_bytes() for path in paths]
        result = subprocess.run(self.command, text=True, capture_output=True, timeout=120)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([path.read_bytes() for path in paths], before)


@unittest.skipUnless(RUNNER, 'set KAIROTENSOR_RECOVERY_RUNNER to a precompiled fixture collector')
class RecoveryPolicyV2RunnerTests(RecoveryPolicyRunnerTests):
    collector_args = ('--corpus-version', 'v2')
    corpus_id = 'ci-report-recovery-v2'

    def test_v2_provenance_and_fresh_heldout_identity(self):
        manifest = self.raw['manifest']
        self.assertEqual(manifest['corpus']['prompt_version'], 'report-bundle/2')
        self.assertIn(self.corpus_id, self.trace['provenance'])
        old = json.loads((Path(__file__).resolve().parents[1] / 'examples/recovery_policy_eval/corpus-v1.json').read_text())
        old_heldout_ids = {c['case_id'] for c in old['cases'] if c['split'] == 'heldout'}
        old_heldout_families = {c['family'] for c in old['cases'] if c['split'] == 'heldout'}
        new_heldout = [c for c in manifest['corpus']['cases'] if c['split'] == 'heldout']
        self.assertTrue(new_heldout)
        self.assertTrue(old_heldout_ids.isdisjoint(c['case_id'] for c in new_heldout))
        self.assertTrue(old_heldout_families.isdisjoint(c['family'] for c in new_heldout))


@unittest.skipUnless(RUNNER, 'set KAIROTENSOR_RECOVERY_RUNNER to a precompiled fixture collector')
class RecoveryPolicySelectionTests(unittest.TestCase):
    def test_invalid_cli_is_rejected_before_output_files_or_directories(self):
        invalid = [('--corpus-version', 'v3'), ('--split', 'heldout'),
                   ('--corpus-version', 'v1', '--corpus-version', 'v2'),
                   ('--split', 'all', '--split', 'calibration'),
                   ('--unknown',), ('--corpus-version',), ('--split',)]
        with tempfile.TemporaryDirectory() as root:
            for index, args in enumerate(invalid):
                parent = Path(root, str(index))
                target = parent / 'trace.json'
                result = subprocess.run([str(Path(RUNNER).resolve()), '--trace', str(target), *args],
                                        text=True, capture_output=True, timeout=30)
                with self.subTest(args=args):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(parent.exists(), 'argument validation must precede output creation')

    def test_v2_calibration_only_excludes_heldout_execution_and_corpus_inputs(self):
        with tempfile.TemporaryDirectory() as root:
            trace = Path(root, 'trace.json')
            result = subprocess.run([str(Path(RUNNER).resolve()), '--trace', str(trace),
                '--corpus-version', 'v2', '--split', 'calibration'],
                text=True, capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(result.stdout)
            self.assertEqual(summary['status'], 'complete')
            self.assertEqual(summary['artifact_kind'], 'calibration_only')
            self.assertIsNone(summary['trace'])
            self.assertEqual(summary['cases'], 4)
            self.assertFalse(trace.exists(), 'calibration collection must not emit a complete policy trace')
            raw = json.loads(Path(str(trace) + '.raw.json').read_text())
            self.assertEqual(raw['status'], 'complete')
            self.assertEqual(raw['manifest']['selection'], dict(split='calibration', corpus_version='v2', case_count=4))
            self.assertEqual(raw['new_model_calls'], 0)
            self.assertEqual(raw['mode'], 'deterministic_byte_token_fixture')
            selected = raw['manifest']['corpus']['cases']
            self.assertEqual(len(selected), 4)
            self.assertTrue(all(c['split'] == 'calibration' for c in selected))
            self.assertEqual(json.loads(raw['manifest']['corpus_utf8']), raw['manifest']['corpus'])
            full_corpus = json.loads((Path(__file__).resolve().parents[1] / 'examples/recovery_policy_eval/corpus-v2.json').read_text())
            serialized = json.dumps(raw)
            for heldout in (c for c in full_corpus['cases'] if c['split'] == 'heldout'):
                self.assertNotIn(heldout['case_id'], serialized)
                self.assertNotIn(heldout['family'], serialized)
                for report in heldout['reports'] + (heldout['retry_reports'] or []):
                    self.assertNotIn(report['release'], serialized)
            ids = {c['case_id'] for c in selected}
            self.assertEqual({c['case_id'] for c in raw['completed_cases']}, ids)
            self.assertTrue(all(e['case_id'] in ids for e in raw['events']))
            with closing(sqlite3.connect(f'{Path(str(trace) + ".sqlite").as_uri()}?mode=ro', uri=True)) as db, db:
                keys = [r[0] for r in db.execute('SELECT recovery_key FROM recovery_runs')]
                self.assertEqual(set(keys), {'ci-report-recovery-v2/' + c for c in ids})
                calls = db.execute('SELECT COUNT(*) FROM dispatches').fetchone()[0]
                self.assertEqual(raw['actual_backend_calls'], calls)
            # A calibration subset must not weaken the scorer's full split guard.
            document = dict(schema_version=2, mode='synthetic_policy_fixture', provenance='calibration-only fixture',
                            cases=raw['completed_cases'])
            with self.assertRaisesRegex(ValueError, 'both calibration and heldout'):
                evaluate(document)


class RecoveryPolicyFreezeTests(unittest.TestCase):
    def test_frozen_prompt_and_corpus_hashes_and_declared_order_match_files(self):
        folder = Path(__file__).resolve().parents[1] / 'examples/recovery_policy_eval'
        prompt_bytes = (folder / 'prompt-v2.json').read_bytes()
        corpus_bytes = (folder / 'corpus-v2.json').read_bytes()
        prompt = json.loads(prompt_bytes)
        corpus = json.loads(corpus_bytes)
        prompt_freeze = json.loads((folder / 'prompt-v2-freeze.json').read_text())
        corpus_freeze = json.loads((folder / 'corpus-v2-freeze.json').read_text())
        digest = hashlib.sha256(prompt_bytes).hexdigest()
        self.assertEqual(prompt_freeze['prompt_sha256'], digest)
        self.assertEqual(corpus_freeze['prompt_sha256'], digest)
        self.assertEqual(corpus_freeze['corpus_sha256'], hashlib.sha256(corpus_bytes).hexdigest())
        self.assertEqual(prompt_freeze['frozen_at_utc'], corpus_freeze['prompt_frozen_at_utc'])
        frozen = datetime.fromisoformat(prompt_freeze['frozen_at_utc'])
        constructed = datetime.fromisoformat(corpus_freeze['constructed_at_utc'])
        self.assertIsNotNone(frozen.utcoffset())
        self.assertIsNotNone(constructed.utcoffset())
        self.assertLess(frozen, constructed)
        self.assertEqual(prompt['prompt_version'], prompt_freeze['prompt_version'])
        self.assertEqual(corpus['prompt_version'], prompt['prompt_version'])
        self.assertEqual(prompt['validator_version'], 'independent-report-verdict/1')
        self.assertEqual(corpus['validator_version'], prompt['validator_version'])
        self.assertFalse(prompt_freeze['validator_change'])

    def test_v2_retains_exact_calibration_records_and_frozen_limits(self):
        folder = Path(__file__).resolve().parents[1] / 'examples/recovery_policy_eval'
        v1 = json.loads((folder / 'corpus-v1.json').read_text())
        v2 = json.loads((folder / 'corpus-v2.json').read_text())
        prompt_freeze = json.loads((folder / 'prompt-v2-freeze.json').read_text())
        corpus_freeze = json.loads((folder / 'corpus-v2-freeze.json').read_text())
        original = [case for case in v1['cases'] if case['split'] == 'calibration']
        current = [case for case in v2['cases'] if case['split'] == 'calibration']
        self.assertEqual(current, original)
        self.assertEqual(len(current), 4)
        self.assertEqual([case['case_id'] for case in current], prompt_freeze['calibration_case_ids'])
        self.assertEqual(corpus_freeze['calibration_case_count'], 4)
        self.assertEqual(corpus_freeze['fresh_heldout_case_count'], 6)
        self.assertEqual(sum(case['split'] == 'heldout' for case in v2['cases']), 6)
        for key, value in [('budget_tokens', 4096), ('output_cap', 96), ('deadline_ms', 60000)]:
            self.assertEqual(v1[key], value)
            self.assertEqual(v2[key], value)
            self.assertEqual(prompt_freeze[key], value)


if __name__ == '__main__':
    unittest.main()
