"""Offline SQLite ownership regressions; real handles and archived bytes only."""
from contextlib import closing, contextmanager
import gc
import hashlib
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

from audit_recovery_bundle import audit_bundle
import incident_calibration_controller as controller
from incident_preflight import calibration_plan
import test_recovery_policy_runner as runner_tests


class SQLiteLifetimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.out = Path(temporary.name)

    @contextmanager
    def owned_connections(self, *, factory=sqlite3.Connection):
        """Observe real connections, then collect without retaining their handles."""
        connect = sqlite3.connect
        connections = []

        def open_connection(*args, **kwargs):
            db = connect(*args, factory=factory, **kwargs)
            connections.append(db)
            return db

        gc.collect()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter('always', ResourceWarning)
            with patch.object(sqlite3, 'connect', open_connection):
                yield
            closed = []
            for db in connections:
                try:
                    db.in_transaction
                except sqlite3.ProgrammingError:
                    closed.append(True)
                else:
                    closed.append(False)
            if connections:
                del db
            connections.clear()
            gc.collect()
        resource_warnings = [str(w.message) for w in caught
                             if issubclass(w.category, ResourceWarning)]
        self.assertTrue(closed, 'expected an owned connection')
        self.assertTrue(all(closed), f'open SQLite handles: {closed}; warnings: {resource_warnings}')
        self.assertEqual(resource_warnings, [])

    def archived_bundle(self):
        archive = Path(__file__).resolve().parents[1] / 'docs/evaluations/recovery-policy-v1-2026-11-14.zip'
        self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(),
                         '067b7b00a8ecf725b11e900eb62ea81ec72c63616e994b3e62ddae4eafec57fa')
        with zipfile.ZipFile(archive) as bundle:
            bundle.extractall(self.out)

    def hashes(self):
        return {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in self.out.iterdir() if p.is_file()}

    def test_complete_archive_closes_and_preserves_score_and_bytes(self):
        self.archived_bundle()
        before = self.hashes()
        with self.owned_connections():
            result = audit_bundle(self.out)
            self.assertEqual(result['status'], 'verified_complete')
            self.assertEqual(result['charged_tokens'], 2978)
            self.assertEqual(result['outstanding_tokens'], 0)
            # Historical producer count; this read-only audit makes no calls.
            self.assertEqual(result['new_model_calls'], 20)
        self.assertEqual(self.hashes(), before)

    def test_incomplete_archive_closes_without_scoring_or_changing_bytes(self):
        self.archived_bundle()
        path = self.out / 'trace.json.raw.json'
        raw = json.loads(path.read_bytes())
        raw['status'] = 'failed'
        path.write_text(json.dumps(raw))
        before = self.hashes()
        with self.owned_connections():
            result = audit_bundle(self.out)
            self.assertEqual(result['status'], 'incomplete_not_scored')
            self.assertEqual(result['unresolved_attempts'], 0)
            self.assertNotIn('summary', result)
            self.assertNotIn('charged_tokens', result)
        self.assertEqual(self.hashes(), before)

    def test_rejected_archive_closes_and_preserves_error_and_bytes(self):
        self.archived_bundle()
        path = self.out / 'trace.json.raw.json'
        raw = json.loads(path.read_bytes())
        raw['actual_backend_calls'] = 999
        path.write_text(json.dumps(raw))
        before = self.hashes()
        with self.owned_connections():
            with self.assertRaisesRegex(ValueError, 'bundle mismatch: backend call count'):
                audit_bundle(self.out)
        self.assertEqual(self.hashes(), before)

    def helper_transaction(self, exceptional):
        path = self.out / 'helper.sqlite'
        writable = self.out / 'writable.sqlite'
        with closing(sqlite3.connect(path)):
            pass
        with closing(sqlite3.connect(writable)) as db:
            db.execute('CREATE TABLE writes(value TEXT)')
        helper = runner_tests.RecoveryPolicyRunnerTests('test_collected_trace_scores_and_affordable_totals_reconcile_sqlite')
        helper.db_path = path
        original = AssertionError('assertion inside helper context')

        def write():
            with helper.database() as db:
                db.execute('ATTACH DATABASE ? AS writable', (str(writable),))
                db.execute("INSERT INTO writable.writes VALUES('kept')")
                if exceptional:
                    raise original

        with self.owned_connections():
            if exceptional:
                with self.assertRaises(AssertionError) as caught:
                    write()
                self.assertIs(caught.exception, original)
                original.__traceback__ = None
            else:
                write()
        with closing(sqlite3.connect(writable)) as db:
            self.assertEqual(db.execute('SELECT value FROM writes').fetchall(),
                             [] if exceptional else [('kept',)])

    def test_helper_commits_and_closes_on_success(self):
        self.helper_transaction(False)

    def test_helper_rolls_back_and_closes_on_assertion(self):
        self.helper_transaction(True)

    def test_connect_pragma_failure_closes_and_keeps_original_database_error(self):
        path = self.out / 'malformed.sqlite'
        path.write_bytes(b'this is not a SQLite database')
        with self.owned_connections():
            with self.assertRaisesRegex(sqlite3.DatabaseError, 'file is not a database'):
                controller._connect(path)
        self.assertEqual(path.read_bytes(), b'this is not a SQLite database')

    def test_initialize_event_failure_rolls_back_closes_and_keeps_original_error(self):
        (self.out / 'manifest.json').write_text('{}')
        original = RuntimeError('injected initial event failure')
        with self.owned_connections(), patch.object(controller, '_event', side_effect=original):
            with self.assertRaises(RuntimeError) as caught:
                controller._initialize(self.out, calibration_plan(), controller._START_MODE)
            self.assertIs(caught.exception, original)
            original.__traceback__ = None
        with closing(sqlite3.connect(self.out / 'ledger.sqlite')) as db:
            self.assertEqual(db.execute('SELECT count(*) FROM slots').fetchone()[0], 0)

    def test_initialize_success_transfers_open_committed_connection(self):
        (self.out / 'manifest.json').write_text('{}')
        with closing(controller._initialize(self.out, calibration_plan(), controller._START_MODE)) as db:
            self.assertEqual(db.execute('SELECT count(*) FROM slots').fetchone()[0], 8)
            self.assertFalse(db.in_transaction)
            self.assertEqual(db.execute('SELECT event FROM events').fetchall(), [('planned',)])
        with closing(sqlite3.connect(self.out / 'ledger.sqlite')) as db:
            self.assertEqual(db.execute('SELECT count(*) FROM slots').fetchone()[0], 8)

    def test_connect_success_transfers_open_handle(self):
        with closing(controller._connect(self.out / 'normal.sqlite')) as db:
            self.assertEqual(db.execute('PRAGMA journal_mode').fetchone()[0], 'delete')
            self.assertEqual(db.execute('PRAGMA synchronous').fetchone()[0], 2)

    def test_audit_snapshot_query_failure_closes_and_leaves_unknown_accounting(self):
        (self.out / 'manifest.json').write_text('{}')
        with closing(controller._initialize(self.out, calibration_plan(), controller._START_MODE)):
            pass
        before = self.hashes()

        class FailingSnapshot(sqlite3.Connection):
            def execute(self, sql, *args, **kwargs):
                if sql.startswith('SELECT slot,'):
                    raise sqlite3.OperationalError('injected snapshot query failure')
                return super().execute(sql, *args, **kwargs)

        with self.owned_connections(factory=FailingSnapshot):
            report = controller.audit_run(self.out, _manifest_validator=lambda manifest: None)
            self.assertFalse(report['audit_complete'])
            self.assertFalse(report['complete'])
            self.assertIsNone(report['ledger_charged_units'])
            self.assertIsNone(report['unresolved_hold_units'])
            self.assertIn('injected snapshot query failure', report['audit_error'])
        self.assertEqual(self.hashes(), before)


if __name__ == '__main__':
    unittest.main()
