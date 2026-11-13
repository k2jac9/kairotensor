"""Behavioral checks for a finite, non-production SQLite contract model."""

import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

import memory


class ContentContract(unittest.TestCase):
    def test_hash_separates_type_version_and_field_boundaries(self):
        self.assertNotEqual(memory.object_ref("ab", 1, b"c"),
                            memory.object_ref("a", 1, b"bc"))
        self.assertNotEqual(memory.object_ref("evidence", 1, b"same"),
                            memory.object_ref("evidence", 2, b"same"))
        self.assertNotEqual(memory.object_ref("evidence", 1, b"same"),
                            memory.object_ref("result", 1, b"same"))
        self.assertEqual(memory.object_ref("evidence", 1, b"same"),
                         memory.object_ref("evidence", 1, b"same"))


class MemoryContract(unittest.TestCase):
    def setUp(self):
        self.assertTrue(hasattr(memory, "Memory"), "Memory contract is not implemented")
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name) / "memory.sqlite"
        self.m = memory.Memory(self.path)
        self.addCleanup(self.m.close)

    def evidence(self, payload=b"observation", actor="alice", scope="task"):
        evidence_id = self.m.store_evidence(payload, source="fixture")
        self.m.admit(actor, scope, evidence_id)
        return evidence_id

    def branch(self):
        self.m.create_branch("main", b"initial state")
        return self.m.branch_state("main")

    def attempt(self, evidence_ids=(), context=b"submitted context"):
        return self.m.begin_attempt("main", "alice", "task", evidence_ids, context)

    def counts(self):
        with sqlite3.connect(f"file:{self.path}?mode=ro", uri=True) as db:
            names = [r[0] for r in db.execute(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
            return {name: db.execute(f'SELECT COUNT(*) FROM "{name}"').fetchone()[0]
                    for name in names}

    def test_identical_bytes_have_distinct_occurrences_and_independent_admission(self):
        one = self.m.store_evidence(b"same", source="sensor")
        two = self.m.store_evidence(b"same", source="sensor")
        self.assertNotEqual(one, two)
        self.assertEqual(self.m.evidence_info(one)["object_ref"],
                         self.m.evidence_info(two)["object_ref"])
        with self.assertRaises(PermissionError):
            self.m.get_evidence("alice", "task", one)
        self.m.admit("alice", "task", one)
        self.assertEqual(self.m.get_evidence("alice", "task", one), b"same")
        with self.assertRaises(PermissionError):
            self.m.get_evidence("alice", "task", two)

    def test_admission_is_actor_and_scope_specific_with_append_only_history(self):
        evidence_id = self.evidence()
        self.m.admit("alice", "task", evidence_id)  # Set no-op: no epoch change.
        self.assertEqual(self.m.view_epoch("alice", "task"), 1)
        for actor, scope in (("bob", "task"), ("alice", "other")):
            self.assertEqual(self.m.view_epoch(actor, scope), 0)
            with self.assertRaises(PermissionError):
                self.m.get_evidence(actor, scope, evidence_id)
        self.m.revoke("alice", "task", evidence_id)
        self.m.revoke("alice", "task", evidence_id)
        self.assertEqual(self.m.view_epoch("alice", "task"), 2)
        history = self.m.admission_history("alice", "task")
        self.assertEqual([(r["action"], r["epoch"]) for r in history],
                         [("ADMIT", 1), ("REVOKE", 2)])

    def test_historical_support_uses_manifest_without_regranting_raw_access(self):
        evidence_id = self.evidence()
        claim = self.m.derive("rule:v1", [[evidence_id]])
        snapshot = self.m.capture_snapshot("alice", "task")
        self.m.revoke("alice", "task", evidence_id)
        self.evidence(b"later evidence")
        self.assertEqual(self.m.snapshot_info(snapshot)["evidence_ids"], [evidence_id])
        explanation = self.m.historical_support(claim, snapshot)
        self.assertEqual(explanation["status"], "HAS_SUPPORT")
        self.assertEqual(explanation["rule_id"], "rule:v1")
        self.assertNotIn("payload", explanation)
        self.assertEqual(self.m.current_support(claim, "alice", "task")["status"],
                         "NO_ACTIVE_SUPPORT")
        with self.assertRaises(PermissionError):
            self.m.get_evidence("alice", "task", evidence_id)

    def test_alternative_support_survives_but_withdrawal_does_not_assert_false(self):
        one, two, three = (self.evidence(value) for value in (b"one", b"two", b"three"))
        claim = self.m.derive("rule:v1", [[one, two], [three]])
        sole = self.m.derive("rule:v1", [[one]])
        self.m.revoke("alice", "task", one)
        self.assertEqual(self.m.current_support(claim, "alice", "task")["active_alternatives"], [1])
        self.assertEqual(self.m.current_support(sole, "alice", "task")["status"], "NO_ACTIVE_SUPPORT")
        self.m.revoke("alice", "task", three)
        self.assertEqual(self.m.current_support(claim, "alice", "task")["status"], "NO_ACTIVE_SUPPORT")

    def test_empty_or_unknown_derivations_are_rejected_before_writing(self):
        evidence_id = self.evidence()
        before = self.counts()
        for alternatives in ([], [[]], [[evidence_id], []], [["unknown"]]):
            with self.subTest(alternatives=alternatives), self.assertRaises((ValueError, KeyError)):
                self.m.derive("rule:v1", alternatives)
            self.assertEqual(self.counts(), before)

    def test_attempt_keeps_snapshot_separate_from_actual_ordered_context(self):
        one, two, unused = (self.evidence(value) for value in (b"one", b"two", b"unused"))
        baseline = self.branch()
        context = b"exact\x00submitted\xffbytes"
        attempt = self.attempt([two, one], context)
        info = self.m.attempt_info(attempt)
        self.assertEqual(info["branch_head"], baseline["head"])
        self.assertEqual(info["branch_revision"], 0)
        self.assertEqual(info["view_epoch"], 3)
        self.assertEqual(info["input_evidence_ids"], [two, one])
        manifest = self.m.snapshot_info(info["snapshot_id"])["evidence_ids"]
        self.assertEqual(set(manifest), {one, two, unused})
        with sqlite3.connect(f"file:{self.path}?mode=ro", uri=True) as db:
            stored = db.execute("SELECT payload FROM objects WHERE ref=?", (info["context_ref"],)).fetchone()[0]
            refs = db.execute("SELECT payload FROM objects WHERE ref=?", (info["inputs_ref"],)).fetchone()[0]
        self.assertEqual(stored, context)
        self.assertEqual(json.loads(refs), [two, one])

    def test_begin_attempt_rejects_unadmitted_inputs_without_partial_writes(self):
        unadmitted = self.m.store_evidence(b"prefetched", source="cache")
        self.branch()
        before = self.counts()
        for ids in ([unadmitted], ["unknown"]):
            with self.subTest(ids=ids), self.assertRaises((PermissionError, KeyError)):
                self.attempt(ids)
            self.assertEqual(self.counts(), before)

    def test_new_admission_invalidates_an_attempt_even_without_a_prior_read(self):
        baseline = self.branch()
        attempt = self.attempt()
        self.evidence(b"new information")
        settled = self.m.settle(attempt, b"result", True, "usage-1", 11)
        self.assertEqual((settled["disposition"], settled["reason"]), ("STALE", "view_changed"))
        self.assertEqual(self.m.branch_state("main"), baseline)
        self.assertEqual(self.m.usage_total(), 11)

    def test_logging_prefetch_and_other_view_changes_do_not_invalidate(self):
        self.branch()
        attempt = self.attempt()
        self.m.log_event(b"global diagnostic")
        prefetched = self.m.store_evidence(b"unadmitted", source="prefetch")
        self.m.admit("bob", "task", prefetched)
        self.m.admit("alice", "other", prefetched)
        self.assertEqual(self.m.view_epoch("alice", "task"), 0)
        self.assertEqual(self.m.branch_state("main")["revision"], 0)
        result = self.m.settle(attempt, b"accepted", True, "usage-1", 5)
        self.assertEqual(result["disposition"], "ACCEPTED")
        self.assertEqual(self.m.branch_state("main")["revision"], 1)

    def test_independent_connections_reject_stale_branch_and_keep_its_cost(self):
        self.branch()
        first = self.attempt()
        other = memory.Memory(self.path)
        self.addCleanup(other.close)
        second = other.begin_attempt("main", "alice", "task", [], b"other input")
        self.m.settle(first, b"winner", True, "usage-1", 3)
        winner = self.m.branch_state("main")
        stale = other.settle(second, b"late", True, "usage-2", 7)
        self.assertEqual((stale["disposition"], stale["reason"]), ("STALE", "branch_changed"))
        self.assertEqual(other.branch_state("main"), winner)
        self.assertEqual(self.m.usage_total(), 10)

    def test_duplicate_delivery_is_idempotent_and_conflicts_do_not_mutate(self):
        self.branch()
        attempt = self.attempt()
        original = self.m.settle(attempt, b"result", True, "usage-1", 5)
        self.assertEqual(self.m.settle(attempt, b"result", True, "usage-1", 5), original)
        before, head = self.counts(), self.m.branch_state("main")
        conflicts = ((b"changed", True, "usage-1", 5), (b"result", False, "usage-1", 5),
                     (b"result", True, "usage-2", 5), (b"result", True, "usage-1", 6))
        for args in conflicts:
            with self.subTest(args=args), self.assertRaises(memory.Conflict):
                self.m.settle(attempt, *args)
            self.assertEqual(self.counts(), before)
            self.assertEqual(self.m.branch_state("main"), head)
        self.assertEqual(self.m.usage_total(), 5)

    def test_usage_id_cannot_settle_a_second_attempt_or_create_a_partial_result(self):
        self.branch()
        first, second = self.attempt(), self.attempt()
        self.m.settle(first, b"first", True, "shared-usage", 5)
        before = self.counts()
        with self.assertRaises(memory.Conflict):
            self.m.settle(second, b"not persisted", True, "shared-usage", 5)
        self.assertEqual(self.counts(), before)

    def test_restore_prevents_aba_by_increasing_revision(self):
        initial = self.branch()
        old, intervening = self.attempt(), self.attempt()
        self.m.settle(intervening, b"intervening state", True, "usage-1", 2)
        self.m.restore_branch("main", initial["head"])
        restored = self.m.branch_state("main")
        self.assertEqual(restored["head"], initial["head"])
        self.assertEqual(restored["revision"], 2)
        result = self.m.settle(old, b"old result", True, "usage-2", 3)
        self.assertEqual(result["disposition"], "STALE")
        self.assertEqual(self.m.branch_state("main"), restored)
        self.assertEqual(self.m.usage_total(), 5)

    def test_validator_rejection_records_cost_without_promoting(self):
        baseline = self.branch()
        attempt = self.attempt()
        result = self.m.settle(attempt, b"invalid result", False, "usage-1", 13)
        self.assertEqual((result["disposition"], result["reason"]), ("REJECTED", "validator_rejected"))
        self.assertEqual(self.m.branch_state("main"), baseline)
        self.assertEqual(self.m.usage_total(), 13)

    def test_malformed_settlement_is_rejected_before_any_writes(self):
        self.branch()
        attempt = self.attempt()
        before = self.counts()
        cases = ((b"result", True, "usage", -1), (b"result", True, "usage", True),
                 (b"result", True, "usage", float("nan")), (b"result", True, "usage", 2**63),
                 (b"result", 1, "usage", 1), ("not bytes", True, "usage", 1),
                 (b"result", True, "", 1))
        for args in cases:
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.m.settle(attempt, *args)
            self.assertEqual(self.counts(), before)
        with self.assertRaises(KeyError):
            self.m.settle("unknown-attempt", b"result", True, "usage", 1)
        self.assertEqual(self.counts(), before)

    def test_reopen_preserves_evidence_snapshot_context_and_settlement(self):
        evidence_id = self.evidence(b"persistent bytes")
        self.branch()
        attempt = self.attempt([evidence_id], b"persisted context")
        result = self.m.settle(attempt, b"result", True, "usage-1", 19)
        snapshot = self.m.attempt_info(attempt)["snapshot_id"]
        self.m.close()
        self.m = memory.Memory(self.path)
        self.addCleanup(self.m.close)
        self.assertEqual(self.m.get_evidence("alice", "task", evidence_id), b"persistent bytes")
        self.assertEqual(self.m.snapshot_info(snapshot)["evidence_ids"], [evidence_id])
        self.assertEqual(self.m.attempt_info(attempt)["input_evidence_ids"], [evidence_id])
        self.assertEqual(self.m.settle(attempt, b"result", True, "usage-1", 19), result)
        self.assertEqual(self.m.branch_state("main")["head"], result["result_ref"])
        self.assertEqual(self.m.usage_total(), 19)
        self.assertEqual(len(self.m.admission_history("alice", "task")), 1)

    def test_revoke_then_readmit_still_invalidates_prior_view_epoch(self):
        evidence_id = self.evidence()
        self.branch()
        attempt = self.attempt([evidence_id])
        self.m.revoke("alice", "task", evidence_id)
        self.m.admit("alice", "task", evidence_id)
        result = self.m.settle(attempt, b"result", True, "usage-1", 1)
        self.assertEqual(self.m.view_epoch("alice", "task"), 3)
        self.assertEqual(result["disposition"], "STALE")


if __name__ == "__main__":
    unittest.main()
