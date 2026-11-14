"""Opt-in end-to-end checks against a precompiled Rust fixture collector.

Build the recovery_policy_eval example separately, then set
KAIROTENSOR_RECOVERY_RUNNER to its executable and run unittest discovery.
These tests never invoke cargo, install dependencies, or request live inference.
"""
import json
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
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.trace_path = Path(cls.directory.name, 'trace.json')
        cls.command = [str(Path(RUNNER).resolve()), '--trace', str(cls.trace_path)]
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

    def database(self):
        db = sqlite3.connect(f'{self.db_path.as_uri()}?mode=ro', uri=True)
        db.row_factory = sqlite3.Row
        return db

    def test_collected_trace_scores_and_affordable_totals_reconcile_sqlite(self):
        self.assertEqual(self.trace['schema_version'], 2)
        self.assertEqual(self.report['mode'], 'synthetic_policy_fixture')
        self.assertEqual(self.report['new_model_calls'], 0)
        self.assertEqual(len(self.trace['cases']), 10)
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
        self.assertEqual(manifest['corpus']['corpus_id'], 'ci-report-recovery-v1')
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
                self.assertEqual(binding['recovery_key'], 'ci-report-recovery-v1/' + case['case_id'])
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

    def test_same_destination_refused_without_mutating_existing_artifacts(self):
        paths = [self.trace_path, self.raw_path, self.db_path]
        before = [path.read_bytes() for path in paths]
        result = subprocess.run(self.command, text=True, capture_output=True, timeout=120)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([path.read_bytes() for path in paths], before)


if __name__ == '__main__':
    unittest.main()
