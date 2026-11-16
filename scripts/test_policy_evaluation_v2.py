"""Strict offline comparison contract; no model calls."""

import importlib.util
import unittest


class PolicyEvaluationTests(unittest.TestCase):
    def test_complete_comparison_api_exists(self):
        self.assertIsNotNone(
            importlib.util.find_spec("policy_evaluation_v2"),
            "complete v2 comparison validator is missing",
        )


class ArtifactTests(unittest.TestCase):
    def test_artifact_refuses_unknown_duplicate_and_injected_configuration(self):
        from policy_evaluation_v2 import validate_policy_artifact

        good = (
            b'{"schema_version":1,"policy_name":"always_stop","purpose":"fixture/recovery-policy","compatibility_contract_sha256":"'
            + b"a" * 64
            + b'"}'
        )
        self.assertEqual(validate_policy_artifact(good)["policy_name"], "always_stop")
        for bad in (
            good.replace(b"always_stop", b"evil"),
            good.replace(b"fixture/recovery-policy", b" "),
            good.replace(b"{", b'{"purpose":"duplicate",'),
            good.replace(b"{", b'{"script":"exec",'),
            good.replace(b"a" * 64, b"A" * 64),
            good.replace(b'"schema_version":1', b'"schema_version":true'),
            b" " * 65537,
        ):
            with self.subTest(bad=bad[:80]), self.assertRaises(ValueError):
                validate_policy_artifact(bad)


import copy
import hashlib
import json


def pack(value):
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()


def record(raw):
    return {"bytes": list(raw), "sha256": hashlib.sha256(raw).hexdigest()}


CONTRACT = b"policy-selection/1\ncomplete-paired-frozen-attestations;success=strict-increase;false-refusal,unsafe-acceptance,charged-plus-reserved,deadline-miss=nonincrease;synthetic=fixture/recovery-policy-only\n"
CONTRACT_SHA = "df97ae41b2648bef15a11bf1950883597b057df1940fe43347d485fedf8590b3"


def fixture():
    policies = []
    for name in ("always_stop", "affordable_retry", "evidence_gated_retry"):
        artifact = {
            "schema_version": 1,
            "policy_name": name,
            "purpose": "fixture/recovery-policy",
            "compatibility_contract_sha256": "a" * 64,
        }
        policies.append({"policy_name": name, "artifact": record(pack(artifact))})
    label = record(
        pack(
            {
                "answerable": True,
                "admissible_support": True,
                "normalized_format": "opaque-bytes/1",
                "scorer_sha256": "1" * 64,
                "expected_normalized": list(b"yes"),
            }
        )
    )
    opportunities = []
    for policy in policies:
        name = policy["policy_name"]
        ops = []
        for n in range(1 if name == "always_stop" else 2):
            ops.append(
                {
                    "opportunity_id": f"{name}/{n}",
                    "request": record(f"request{n}".encode()),
                    "evidence_manifest": record(b"evidence"),
                    "admitted": True,
                    "consistent": True,
                    "input_bound": 4,
                    "output_cap": 6,
                    "start_min_ms": 0,
                    "start_max_ms": 99,
                }
            )
        opportunities.append({"policy_name": name, "opportunities": ops})
    case = {
        "case_id": "c1",
        "family_id": "family",
        "split_id": "heldout",
        "initial_evidence": record(b"evidence"),
        "label": label,
        "budget": 30,
        "deadline_ms": 100,
        "timing_start_ms": 0,
        "timing_end_ms": 200,
        "policy_opportunities": opportunities,
    }
    manifest = {
        "format": "policy-comparison/1",
        "actor": "actor",
        "scope": "scope",
        "purpose": "fixture/recovery-policy",
        "provenance": "synthetic_registry_mechanics",
        "accounting_unit": "fixture_bytes",
        "workload_sha256": "a" * 64,
        "prompt_sha256": "b" * 64,
        "runtime_sha256": "c" * 64,
        "model_sha256": "d" * 64,
        "tokenizer_sha256": "e" * 64,
        "validator_sha256": "f" * 64,
        "scorer_sha256": "1" * 64,
        "compatibility_contract_sha256": "a" * 64,
        "policies": policies,
        "cases": [case],
    }
    row = {
        "case_id": "c1",
        "outcome": "Refused",
        "correct": False,
        "eligible": True,
        "answerable": True,
        "completed_ms": 100,
        "charged": 4,
        "reserved_bound": 0,
        "false_refusal": True,
        "unsafe_acceptance": False,
        "deadline_miss": False,
        "simulated": True,
        "attempts": [],
    }
    candidate = copy.deepcopy(row)
    candidate.update(outcome="Accepted", correct=True, false_refusal=False)
    raw = pack(manifest)
    return {
        "evaluation_id": "e1",
        "actor": "actor",
        "scope": "scope",
        "purpose": "fixture/recovery-policy",
        "baseline_id": "v1",
        "candidate_id": "v2",
        "baseline_sha256": policies[0]["artifact"]["sha256"],
        "candidate_sha256": policies[1]["artifact"]["sha256"],
        "experiment_manifest": list(raw),
        "experiment_sha256": hashlib.sha256(raw).hexdigest(),
        "evaluator_version": "policy-selection/1",
        "evaluator_sha256": CONTRACT_SHA,
        "provenance": "synthetic_registry_mechanics",
        "baseline_rows": [row],
        "candidate_rows": [candidate],
    }


def manifest_edit(e, change):
    m = json.loads(bytes(e["experiment_manifest"]))
    change(m)
    raw = pack(m)
    e.update(
        experiment_manifest=list(raw), experiment_sha256=hashlib.sha256(raw).hexdigest()
    )


class ComparisonTests(unittest.TestCase):
    def evaluate(self, e):
        from policy_evaluation_v2 import evaluate_policy_comparison

        return evaluate_policy_comparison(e)

    def test_positive_synthetic_and_equal_deadline(self):
        result = self.evaluate(fixture())
        self.assertEqual(
            result,
            {
                "decision": "Eligible",
                "validated_counters": {
                    "baseline": {
                        "cases": 1,
                        "success": 0,
                        "false_refusal": 1,
                        "unsafe_acceptance": 0,
                        "cost": 4,
                        "deadline_misses": 0,
                    },
                    "candidate": {
                        "cases": 1,
                        "success": 1,
                        "false_refusal": 0,
                        "unsafe_acceptance": 0,
                        "cost": 4,
                        "deadline_misses": 0,
                    },
                },
            },
        )

    def test_identity_mismatches_are_incomparable(self):
        for key, value in [
            ("actor", "other"),
            ("scope", "other"),
            ("purpose", "other"),
            ("experiment_sha256", "0" * 64),
            ("baseline_sha256", "0" * 64),
            ("candidate_sha256", "0" * 64),
            ("evaluator_version", "new"),
            ("evaluator_sha256", "0" * 64),
            ("provenance", "observed_policy_replay"),
        ]:
            e = fixture()
            e[key] = value
            with self.subTest(key=key):
                self.assertEqual(
                    self.evaluate(e), {"decision": "Refused", "reason": "Incomparable"}
                )
        for key, value in [
            ("accounting_unit", "logical_support"),
            ("accounting_unit", "unknown"),
        ]:
            e = fixture()
            manifest_edit(e, lambda m, k=key, v=value: m.update({k: v}))
            self.assertEqual(self.evaluate(e)["reason"], "Incomparable")

    def test_missing_unknown_and_reordered_cases_refuse(self):
        for side in ("baseline_rows", "candidate_rows"):
            e = fixture()
            e[side] = []
            self.assertEqual(self.evaluate(e)["reason"], "Incomplete")
            for key in ("correct", "eligible", "completed_ms", "charged"):
                e = fixture()
                e[side][0][key] = None
                self.assertEqual(self.evaluate(e)["reason"], "Incomplete")
            e = fixture()
            e[side][0].update(outcome="Unknown", false_refusal=False)
            self.assertEqual(self.evaluate(e)["reason"], "Incomplete")
            e = fixture()
            e[side][0]["case_id"] = "other"
            self.assertEqual(self.evaluate(e)["reason"], "Incomparable")

    def test_fabricated_flags_labels_and_counters_error(self):
        for key in (
            "false_refusal",
            "unsafe_acceptance",
            "deadline_miss",
            "answerable",
            "eligible",
        ):
            e = fixture()
            e["candidate_rows"][0][key] = not e["candidate_rows"][0][key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.evaluate(e)
        for value in (-1, True, 1.5, 1 << 64):
            e = fixture()
            e["candidate_rows"][0]["charged"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.evaluate(e)
        e = fixture()
        e["candidate_rows"][0].update(charged=(1 << 64) - 1, reserved_bound=1)
        with self.assertRaises(ValueError):
            self.evaluate(e)
        e = fixture()
        e["candidate_rows"][0]["invented_total"] = 100
        with self.assertRaises(ValueError):
            self.evaluate(e)

    def test_no_improvement_cost_and_deadline_rules(self):
        e = fixture()
        e["candidate_rows"] = copy.deepcopy(e["baseline_rows"])
        self.assertEqual(self.evaluate(e)["reason"], "NoImprovement")
        e = fixture()
        e["candidate_rows"][0]["charged"] = 5
        self.assertEqual(self.evaluate(e)["reason"], "IncreasedCost")
        e = fixture()
        e["candidate_rows"][0].update(completed_ms=101, deadline_miss=True)
        self.assertEqual(self.evaluate(e)["reason"], "NoImprovement")

    def test_synthetic_nonfixture_refuses_even_consistent_identities(self):
        e = fixture()
        e["purpose"] = "production"

        def edit(m):
            m["purpose"] = "production"
            for p in m["policies"]:
                a = json.loads(bytes(p["artifact"]["bytes"]))
                a["purpose"] = "production"
                p["artifact"] = record(pack(a))
            e.update(
                baseline_sha256=m["policies"][0]["artifact"]["sha256"],
                candidate_sha256=m["policies"][1]["artifact"]["sha256"],
            )

        manifest_edit(e, edit)
        self.assertEqual(self.evaluate(e)["reason"], "Incomparable")

    def test_parse_required_duplicate_and_unknown_fields(self):
        from policy_evaluation_v2 import parse_policy_evaluation

        e = fixture()
        self.assertEqual(parse_policy_evaluation(pack(e)), e)
        for raw in (
            pack(e).replace(b"{", b'{"actor":"duplicate",', 1),
            pack(e).replace(b"{", b'{"extra":0,', 1),
            b" " * ((16 << 20) + 1),
        ):
            with self.assertRaises(ValueError):
                parse_policy_evaluation(raw)
        del e["candidate_rows"][0]["charged"]
        with self.assertRaises(ValueError):
            parse_policy_evaluation(pack(e))

    def test_golden_contract_and_provenance_digest(self):
        from policy_evaluation_v2 import SELECTION_CONTRACT, evaluation_sha256

        self.assertEqual(SELECTION_CONTRACT, CONTRACT)
        self.assertEqual(hashlib.sha256(CONTRACT).hexdigest(), CONTRACT_SHA)
        e = fixture()
        before = evaluation_sha256(e)
        e["provenance"] = "observed_policy_replay"
        self.assertNotEqual(before, evaluation_sha256(e))


def observed_fixture():
    e = fixture()
    e["provenance"] = "observed_policy_replay"
    manifest_edit(
        e,
        lambda m: m.update(
            provenance="observed_policy_replay", accounting_unit="model_tokens"
        ),
    )
    m = json.loads(bytes(e["experiment_manifest"]))
    c = m["cases"][0]
    for side, index in [("baseline_rows", 0), ("candidate_rows", 1)]:
        row = e[side][0]
        o = c["policy_opportunities"][index]["opportunities"][0]
        response = record(b"yes")
        a = {
            "execution_id": "shared-execution",
            "opportunity_id": o["opportunity_id"],
            "request_sha256": o["request"]["sha256"],
            "evidence_manifest_sha256": o["evidence_manifest"]["sha256"],
            "execution": "Executed",
            "disposition": "Accepted",
            "response": response,
            "measured_input": 2,
            "measured_output": 2,
            "retained_bound": 0,
            "start_ms": 0,
            "completed_ms": 100,
            "validator_sha256": m["validator_sha256"],
            "scorer_sha256": m["scorer_sha256"],
            "label_sha256": c["label"]["sha256"],
            "scoring": {
                "response_sha256": response["sha256"],
                "scorer_sha256": m["scorer_sha256"],
                "label_sha256": c["label"]["sha256"],
                "normalized": list(b"yes"),
            },
        }
        row.update(
            outcome="Accepted",
            correct=True,
            false_refusal=False,
            simulated=False,
            attempts=[a],
        )
    return e


class ObservedTests(unittest.TestCase):
    evaluate = ComparisonTests.evaluate

    def test_same_observed_initial_has_no_improvement(self):
        self.assertEqual(self.evaluate(observed_fixture())["reason"], "NoImprovement")

    def test_reference_mismatches_and_missing_scoring_refuse(self):
        for key in (
            "opportunity_id",
            "request_sha256",
            "evidence_manifest_sha256",
            "validator_sha256",
            "scorer_sha256",
            "label_sha256",
        ):
            e = observed_fixture()
            e["candidate_rows"][0]["attempts"][0][key] = "0" * 64
            with self.subTest(key=key):
                self.assertEqual(self.evaluate(e)["reason"], "Incomparable")
        for key in ("response_sha256", "scorer_sha256", "label_sha256"):
            e = observed_fixture()
            e["candidate_rows"][0]["attempts"][0]["scoring"][key] = "0" * 64
            with self.subTest(key=key):
                self.assertEqual(self.evaluate(e)["reason"], "Incomparable")
        e = observed_fixture()
        e["candidate_rows"][0]["attempts"][0]["scoring"] = None
        self.assertEqual(self.evaluate(e)["reason"], "Incomplete")
        e = observed_fixture()
        e["candidate_rows"][0]["attempts"] = []
        self.assertEqual(self.evaluate(e)["reason"], "Incomplete")

    def test_false_correctness_usage_and_timing_cannot_fabricate_success(self):
        for key, value in [
            ("correct", False),
            ("charged", 3),
            ("completed_ms", 99),
            ("reserved_bound", 1),
        ]:
            e = observed_fixture()
            e["candidate_rows"][0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.evaluate(e)
        e = observed_fixture()
        a = e["candidate_rows"][0]["attempts"][0]
        a["execution_id"] = "independent"
        a["scoring"]["normalized"] = list(b"no")
        with self.assertRaises(ValueError):
            self.evaluate(e)
        e = observed_fixture()
        a = e["candidate_rows"][0]["attempts"][0]
        a["measured_input"] = 5
        a["execution_id"] = "independent"
        with self.assertRaises(ValueError):
            self.evaluate(e)
        e = observed_fixture()
        a = e["candidate_rows"][0]["attempts"][0]
        a["start_ms"] = 101
        a["execution_id"] = "independent"
        with self.assertRaises(ValueError):
            self.evaluate(e)

    def test_shared_execution_cannot_contradict_but_equal_requests_may_be_independent(
        self,
    ):
        e = observed_fixture()
        r = e["candidate_rows"][0]
        a = r["attempts"][0]
        a["scoring"]["normalized"] = list(b"no")
        r.update(correct=False, unsafe_acceptance=True)
        self.assertEqual(self.evaluate(e)["reason"], "Incomparable")
        a["execution_id"] = "independent"
        self.assertEqual(self.evaluate(e)["reason"], "NoImprovement")

    def test_unknown_row_fields_remain_incomplete_with_known_observations(self):
        for key in ("correct", "eligible", "charged", "completed_ms"):
            e = observed_fixture()
            e["candidate_rows"][0][key] = None
            with self.subTest(key=key):
                self.assertEqual(self.evaluate(e)["reason"], "Incomplete")

    def test_unknown_usage_retains_quote_and_never_qualifies(self):
        e = observed_fixture()
        r = e["candidate_rows"][0]
        a = r["attempts"][0]
        a.update(
            execution_id="uncertain",
            disposition="Uncertain",
            measured_input=None,
            measured_output=None,
            retained_bound=10,
            response=None,
            scoring=None,
            completed_ms=None,
        )
        r.update(
            outcome="Unknown",
            correct=None,
            charged=None,
            reserved_bound=10,
            completed_ms=None,
        )
        self.assertEqual(self.evaluate(e)["reason"], "Incomplete")
        a["retained_bound"] = 9
        with self.assertRaises(ValueError):
            self.evaluate(e)

    def test_only_invalid_output_unlocks_retry_and_quotes_gate_before_outcome(self):
        e = observed_fixture()
        r = e["candidate_rows"][0]
        first = r["attempts"][0]
        first.update(execution_id="first", disposition="InvalidOutput", completed_ms=20)
        m = json.loads(bytes(e["experiment_manifest"]))
        o = m["cases"][0]["policy_opportunities"][1]["opportunities"][1]
        second = copy.deepcopy(first)
        second.update(
            execution_id="second",
            opportunity_id=o["opportunity_id"],
            request_sha256=o["request"]["sha256"],
            start_ms=20,
            completed_ms=100,
            disposition="Accepted",
        )
        r.update(charged=8, attempts=[first, second])
        self.assertEqual(self.evaluate(e)["reason"], "NoImprovement")
        for disposition in ("Accepted", "Stale", "Failed", "Uncertain"):
            bad = copy.deepcopy(e)
            bad["candidate_rows"][0]["attempts"][0]["disposition"] = disposition
            with self.subTest(disposition=disposition), self.assertRaises(ValueError):
                self.evaluate(bad)
        bad = copy.deepcopy(e)
        manifest_edit(bad, lambda m: m["cases"][0].update(budget=13))
        with self.assertRaises(ValueError):
            self.evaluate(bad)
        bad = copy.deepcopy(e)
        bad["candidate_rows"][0]["attempts"] = bad["candidate_rows"][0]["attempts"][:1]
        self.assertEqual(self.evaluate(bad)["reason"], "Incomplete")

    def test_baseline_projection_binds_exact_frozen_universe(self):
        from policy_evaluation_v2 import paired_baseline_matches

        e = fixture()
        b = copy.deepcopy(e)
        b["evaluation_id"] = "baseline-only-receipt"
        self.assertTrue(paired_baseline_matches(b, e))
        for key in ("family_id", "split_id", "case_id"):
            b = copy.deepcopy(e)
            manifest_edit(b, lambda m, k=key: m["cases"][0].update({k: "other"}))
            self.assertFalse(paired_baseline_matches(b, e))
        b = copy.deepcopy(e)
        b["baseline_rows"][0]["charged"] = 3
        self.assertFalse(paired_baseline_matches(b, e))


def legacy_comparison():
    from recovery_policy_eval import evaluate, synthetic_fixture

    old = synthetic_fixture()
    out = evaluate(old)
    e = fixture()
    m = json.loads(bytes(e["experiment_manifest"]))
    template = copy.deepcopy(m["cases"][0])
    m["cases"] = []
    m["accounting_unit"] = "model_tokens"
    m["workload_sha256"] = hashlib.sha256(pack(old)).hexdigest()
    for case in old["cases"]:
        c = copy.deepcopy(template)
        c.update(
            case_id=case["case_id"],
            family_id=case["family"],
            split_id=case["split"],
            budget=case["budget_tokens"],
            deadline_ms=case["deadline_ms"],
        )
        c["initial_evidence"] = record(case["initial_context"]["evidence_id"].encode())
        for p in c["policy_opportunities"]:
            for i, o in enumerate(p["opportunities"]):
                ctx = (
                    case["initial_context"]
                    if i == 0
                    else case["retries"][p["policy_name"]]["context"]
                )
                o.update(
                    request=record(ctx["request"].encode()),
                    evidence_manifest=record(ctx["evidence_id"].encode()),
                    admitted=ctx["admitted"],
                    consistent=ctx["consistent"],
                    input_bound=ctx["input_tokens"],
                    output_cap=ctx["output_cap"],
                )
        m["cases"].append(c)
    e["baseline_rows"] = []
    e["candidate_rows"] = []
    for side, policy in [
        ("baseline_rows", "always_stop"),
        ("candidate_rows", "affordable_retry"),
    ]:
        for row in out["cases"]:
            if row["policy"] != policy:
                continue
            e[side].append(
                {
                    "case_id": row["case_id"],
                    "outcome": "Refused" if row["abstained"] else "Accepted",
                    "correct": row["correct"],
                    "eligible": True,
                    "answerable": True,
                    "completed_ms": row["elapsed_ms"],
                    "charged": row["charged_tokens"],
                    "reserved_bound": row["reserved_tokens"],
                    "false_refusal": row["abstained"],
                    "unsafe_acceptance": False,
                    "deadline_miss": not row["on_time"],
                    "simulated": True,
                    "attempts": [],
                }
            )
    raw = pack(m)
    e.update(
        experiment_manifest=list(raw), experiment_sha256=hashlib.sha256(raw).hexdigest()
    )
    return old, e


class ConversionTests(unittest.TestCase):
    def test_legacy_without_complete_supplied_facts_is_explicitly_incomplete(self):
        from policy_evaluation_v2 import convert_legacy
        from recovery_policy_eval import synthetic_fixture

        for version in (1, 2):
            old = synthetic_fixture()
            old["schema_version"] = version
            result = convert_legacy(old)
            self.assertEqual(
                (result["format"], result["status"]),
                ("policy-evaluation-conversion/2", "Incomplete"),
            )
            self.assertNotIn("evaluation", result)

    def test_matching_frozen_comparison_preserves_no_improvement_and_old_rows(self):
        from policy_evaluation_v2 import convert_legacy

        old, e = legacy_comparison()
        before = copy.deepcopy(old)
        result = convert_legacy(old, e)
        self.assertEqual(result["status"], "Complete")
        self.assertEqual(result["selection"]["reason"], "IncreasedCost")
        self.assertEqual(old, before)
        bad = copy.deepcopy(old)
        bad["provenance"] = "different source"
        self.assertEqual(convert_legacy(bad, e)["status"], "Incomplete")
        bad = copy.deepcopy(e)
        bad["candidate_rows"][0]["charged"] = 6
        self.assertEqual(convert_legacy(old, bad)["status"], "Incomplete")


class AdditionalBoundaries(unittest.TestCase):
    evaluate = ComparisonTests.evaluate

    def test_each_nonincrease_gate_independently(self):
        for flag, reason in [
            ("false_refusal", "IncreasedFalseRefusal"),
            ("unsafe_acceptance", "IncreasedUnsafeAcceptance"),
            ("deadline_miss", "IncreasedDeadlineMisses"),
        ]:
            e = fixture()

            def multiple(m):
                c = m["cases"][0]
                m["cases"] = [copy.deepcopy(c) for _ in range(3)]
                for i, c in enumerate(m["cases"]):
                    c["case_id"] = str(i)

            manifest_edit(e, multiple)
            for side in ("baseline_rows", "candidate_rows"):
                row = e[side][0]
                e[side] = [copy.deepcopy(row) for _ in range(3)]
                for i, r in enumerate(e[side]):
                    r.update(
                        case_id=str(i),
                        outcome="Failed",
                        correct=False,
                        false_refusal=False,
                    )
            for i in (0, 1):
                e["candidate_rows"][i].update(outcome="Accepted", correct=True)
            r = e["candidate_rows"][2]
            if flag == "false_refusal":
                r.update(outcome="Refused", false_refusal=True)
            if flag == "unsafe_acceptance":
                r.update(outcome="Accepted", unsafe_acceptance=True)
            if flag == "deadline_miss":
                r.update(completed_ms=101, deadline_miss=True)
            with self.subTest(flag=flag):
                self.assertEqual(self.evaluate(e)["reason"], reason)

    def test_aggregate_overflow_is_error(self):
        e = fixture()

        def multiple(m):
            m["cases"] = [copy.deepcopy(m["cases"][0]) for _ in range(2)]
            for i, c in enumerate(m["cases"]):
                c.update(case_id=str(i), budget=(1 << 64) - 1)

        manifest_edit(e, multiple)
        for side in ("baseline_rows", "candidate_rows"):
            e[side] = [copy.deepcopy(e[side][0]) for _ in range(2)]
            for i, r in enumerate(e[side]):
                r.update(case_id=str(i), charged=1 << 63)
        with self.assertRaises(ValueError):
            self.evaluate(e)

    def test_family_split_leakage_and_duplicate_case_are_invalid(self):
        for duplicate in (True, False):
            e = fixture()

            def edit(m, duplicate=duplicate):
                c = copy.deepcopy(m["cases"][0])
                c["split_id"] = "calibration"
                if not duplicate:
                    c["case_id"] = "c2"
                m["cases"].append(c)

            manifest_edit(e, edit)
            with self.assertRaises(ValueError):
                self.evaluate(e)


class StrictUnknownTests(unittest.TestCase):
    evaluate = ComparisonTests.evaluate

    def test_unknown_does_not_hide_known_false_refusal_flags(self):
        e = fixture()
        r = e["baseline_rows"][0]
        r["charged"] = None
        r["false_refusal"] = False
        with self.assertRaises(ValueError):
            self.evaluate(e)

    def test_same_execution_cannot_count_for_two_scheduled_cases(self):
        e = observed_fixture()

        def duplicate(m):
            c = copy.deepcopy(m["cases"][0])
            c["case_id"] = "c2"
            m["cases"].append(c)

        manifest_edit(e, duplicate)
        for side in ("baseline_rows", "candidate_rows"):
            r = copy.deepcopy(e[side][0])
            r["case_id"] = "c2"
            e[side].append(r)
        self.assertEqual(self.evaluate(e)["reason"], "Incomparable")

    def test_unknown_first_case_cannot_hide_fabricated_later_row(self):
        e = fixture()

        def duplicate(m):
            c = copy.deepcopy(m["cases"][0])
            c["case_id"] = "c2"
            m["cases"].append(c)

        manifest_edit(e, duplicate)
        for side in ("baseline_rows", "candidate_rows"):
            r = copy.deepcopy(e[side][0])
            r["case_id"] = "c2"
            e[side].append(r)
        e["baseline_rows"][0]["charged"] = None
        e["baseline_rows"][1]["false_refusal"] = False
        with self.assertRaises(ValueError):
            self.evaluate(e)


class EncodingTests(unittest.TestCase):
    def test_canonical_limit_counts_json_escapes_exactly(self):
        from unittest.mock import patch

        from policy_evaluation_v2 import canonical

        expected = b'{"text":"\\n"}'
        with patch("policy_evaluation_v2.MAX_BYTES", len(expected)):
            self.assertEqual(canonical({"text": "\n"}), expected)
        with (
            patch("policy_evaluation_v2.MAX_BYTES", len(expected) - 1),
            self.assertRaises(ValueError),
        ):
            canonical({"text": "\n"})

    def test_deep_json_and_lone_surrogates_are_rejected(self):
        from policy_evaluation_v2 import loads, validate_policy_artifact

        with self.assertRaises(ValueError):
            loads(b"[" * 65 + b"0" + b"]" * 65)
        raw = (
            b'{"schema_version":1,"policy_name":"always_stop","purpose":"\\ud800","compatibility_contract_sha256":"'
            + b"a" * 64
            + b'"}'
        )
        with self.assertRaises(ValueError):
            validate_policy_artifact(raw)


def golden_cases():
    cases = [
        ("synthetic-positive", fixture(), "Eligible"),
        ("observed-equal", observed_fixture(), "NoImprovement"),
    ]

    def changed(name, change, want, observed=False):
        e = observed_fixture() if observed else fixture()
        change(e)
        cases.append((name, e, want))

    for key in (
        "actor",
        "scope",
        "purpose",
        "evaluator_version",
        "evaluator_sha256",
        "baseline_sha256",
        "candidate_sha256",
        "experiment_sha256",
    ):
        changed(key, lambda e, k=key: e.update({k: "0" * 64}), "Incomparable")
    for key in ("correct", "eligible", "charged", "completed_ms"):
        changed(
            "unknown-" + key,
            lambda e, k=key: e["candidate_rows"][0].update({k: None}),
            "Incomplete",
        )
    for key in ("correct", "eligible", "charged", "completed_ms"):
        changed(
            "observed-unknown-" + key,
            lambda e, k=key: e["candidate_rows"][0].update({k: None}),
            "Incomplete",
            True,
        )
    for key in (
        "false_refusal",
        "unsafe_acceptance",
        "deadline_miss",
        "answerable",
        "eligible",
    ):
        changed(
            "fabricated-" + key,
            lambda e, k=key: e["candidate_rows"][0].update(
                {k: not e["candidate_rows"][0][k]}
            ),
            "Error",
        )
    for unit in ("logical_support", "unknown"):
        changed(
            "unit-" + unit,
            lambda e, u=unit: manifest_edit(e, lambda m: m.update(accounting_unit=u)),
            "Incomparable",
        )
    changed(
        "over-u64", lambda e: e["candidate_rows"][0].update(charged=1 << 64), "Error"
    )
    changed(
        "boolean-count", lambda e: e["candidate_rows"][0].update(charged=True), "Error"
    )
    changed(
        "overflow",
        lambda e: e["candidate_rows"][0].update(
            charged=(1 << 64) - 1, reserved_bound=1
        ),
        "Error",
    )
    changed("missing-cases", lambda e: e.update(candidate_rows=[]), "Incomplete")
    changed(
        "wrong-case",
        lambda e: e["candidate_rows"][0].update(case_id="wrong"),
        "Incomparable",
    )
    changed("missing-field", lambda e: e["candidate_rows"][0].pop("charged"), "Error")
    changed("extra-field", lambda e: e["candidate_rows"][0].update(total=0), "Error")
    changed("cost", lambda e: e["candidate_rows"][0].update(charged=5), "IncreasedCost")
    changed(
        "late",
        lambda e: e["candidate_rows"][0].update(completed_ms=101, deadline_miss=True),
        "NoImprovement",
    )
    for key in (
        "request_sha256",
        "opportunity_id",
        "evidence_manifest_sha256",
        "validator_sha256",
        "scorer_sha256",
        "label_sha256",
    ):
        changed(
            "observed-" + key,
            lambda e, k=key: e["candidate_rows"][0]["attempts"][0].update(
                {k: "0" * 64}
            ),
            "Incomparable",
            True,
        )
    changed(
        "missing-scoring",
        lambda e: e["candidate_rows"][0]["attempts"][0].update(scoring=None),
        "Incomplete",
        True,
    )
    changed(
        "observed-cost",
        lambda e: e["candidate_rows"][0].update(charged=3),
        "Error",
        True,
    )
    changed(
        "unknown-hidden-flag",
        lambda e: e["baseline_rows"][0].update(charged=None, false_refusal=False),
        "Error",
    )
    for kind, reason in [
        ("false_refusal", "IncreasedFalseRefusal"),
        ("unsafe_acceptance", "IncreasedUnsafeAcceptance"),
        ("deadline_miss", "IncreasedDeadlineMisses"),
    ]:
        e = fixture()

        def multiple(m):
            m["cases"] = [copy.deepcopy(m["cases"][0]) for _ in range(3)]
            for i, c in enumerate(m["cases"]):
                c["case_id"] = str(i)

        manifest_edit(e, multiple)
        for side in ("baseline_rows", "candidate_rows"):
            e[side] = [copy.deepcopy(e[side][0]) for _ in range(3)]
            for i, r in enumerate(e[side]):
                r.update(
                    case_id=str(i), outcome="Failed", correct=False, false_refusal=False
                )
        for i in (0, 1):
            e["candidate_rows"][i].update(outcome="Accepted", correct=True)
        r = e["candidate_rows"][2]
        if kind == "false_refusal":
            r.update(outcome="Refused", false_refusal=True)
        if kind == "unsafe_acceptance":
            r.update(outcome="Accepted", unsafe_acceptance=True)
        if kind == "deadline_miss":
            r.update(completed_ms=101, deadline_miss=True)
        cases.append((kind, e, reason))
    return cases


def baseline_only_fixture(observed=False):
    e = observed_fixture() if observed else fixture()
    e.update(
        candidate_id=e["baseline_id"],
        candidate_sha256=e["baseline_sha256"],
        candidate_rows=copy.deepcopy(e["baseline_rows"]),
    )
    return e


def incomplete_observation_fixture():
    e = observed_fixture()
    r = e["candidate_rows"][0]
    a = r["attempts"][0]
    a.update(execution_id="unknown-measurement", measured_input=None, retained_bound=10)
    r.update(
        outcome="Unknown",
        correct=None,
        charged=None,
        reserved_bound=10,
        completed_ms=None,
    )
    return e


def legacy_observed_comparison():
    old, e = legacy_comparison()
    old["mode"] = "recorded_policy_replay"
    e["provenance"] = "observed_policy_replay"
    manifest_edit(
        e,
        lambda m: m.update(
            provenance="observed_policy_replay",
            workload_sha256=hashlib.sha256(pack(old)).hexdigest(),
        ),
    )
    m = json.loads(bytes(e["experiment_manifest"]))
    for side, pi in [("baseline_rows", 0), ("candidate_rows", 1)]:
        for row, c, oldc in zip(e[side], m["cases"], old["cases"]):
            count = (
                2 if pi and oldc["initial"]["disposition"] == "invalid_output" else 1
            )
            attempts = []
            end = 0
            for i in range(count):
                cell = (
                    oldc["retries"]["affordable_retry"]
                    if i
                    else {
                        "context": oldc["initial_context"],
                        "attempt": oldc["initial"],
                    }
                )
                ctx, prior = cell["context"], cell["attempt"]
                op = c["policy_opportunities"][pi]["opportunities"][i]
                start = end + ctx["elapsed_ms"]
                end = start + prior["elapsed_ms"]
                response = record(b"yes" if prior["correct"] else b"no")
                attempts.append(
                    {
                        "execution_id": c["case_id"] + "/" + str(i),
                        "opportunity_id": op["opportunity_id"],
                        "request_sha256": op["request"]["sha256"],
                        "evidence_manifest_sha256": op["evidence_manifest"]["sha256"],
                        "execution": "Executed",
                        "disposition": "Accepted"
                        if prior["disposition"] == "accepted"
                        else "InvalidOutput",
                        "response": response,
                        "measured_input": ctx["input_tokens"],
                        "measured_output": prior["charged_tokens"]
                        - ctx["input_tokens"],
                        "retained_bound": 0,
                        "start_ms": start,
                        "completed_ms": end,
                        "validator_sha256": m["validator_sha256"],
                        "scorer_sha256": m["scorer_sha256"],
                        "label_sha256": c["label"]["sha256"],
                        "scoring": {
                            "response_sha256": response["sha256"],
                            "scorer_sha256": m["scorer_sha256"],
                            "label_sha256": c["label"]["sha256"],
                            "normalized": list(b"yes" if prior["correct"] else b"no"),
                        },
                    }
                )
            row.update(simulated=False, attempts=attempts)
    return old, e


def fix_round_one_goldens():
    cases = []
    for observed in (False, True):
        e = baseline_only_fixture(observed)
        cases.append(("baseline-only-" + str(observed), e, "NoImprovement"))
        e = baseline_only_fixture(observed)
        for side in ("baseline_rows", "candidate_rows"):
            e[side][0]["charged"] = None
        cases.append(("baseline-unknown-" + str(observed), e, "Incomplete"))
    e = baseline_only_fixture()
    for side in ("baseline_rows", "candidate_rows"):
        e[side][0]["false_refusal"] = False
    cases.append(("baseline-invalid-identical", e, "Error"))
    e = baseline_only_fixture()
    e["candidate_sha256"] = "0" * 64
    cases.append(("baseline-conflicting-digest", e, "Incomparable"))
    e = baseline_only_fixture()
    e["candidate_rows"][0]["charged"] = 3
    cases.append(("baseline-conflicting-rows", e, "Incomparable"))
    e = incomplete_observation_fixture()
    cases.append(("unknown-usage-valid", e, "Incomplete"))
    for end in (201,):
        e = incomplete_observation_fixture()
        e["candidate_rows"][0]["attempts"][0]["completed_ms"] = end
        cases.append(("unknown-usage-invalid-completion", e, "Error"))
    e = incomplete_observation_fixture()
    a = e["candidate_rows"][0]["attempts"][0]
    a.update(start_ms=20, completed_ms=19)
    cases.append(("unknown-usage-completion-before-start", e, "Error"))
    for key in ("response_sha256", "scorer_sha256", "label_sha256"):
        e = incomplete_observation_fixture()
        e["candidate_rows"][0]["attempts"][0]["scoring"][key] = "0" * 64
        cases.append(("unknown-usage-scoring-" + key, e, "Incomparable"))
    e = incomplete_observation_fixture()
    a = e["candidate_rows"][0]["attempts"][0]
    a["execution_id"] = "shared-execution"
    a["response"] = record(b"different raw response")
    a["scoring"].update(response_sha256=a["response"]["sha256"], normalized=list(b"no"))
    cases.append(("unknown-usage-conflicting-execution", e, "Incomparable"))
    e = incomplete_observation_fixture()
    a = e["candidate_rows"][0]["attempts"][0]
    a.update(
        execution="Unknown",
        disposition="Uncertain",
        measured_output=None,
        response=None,
        scoring=None,
        completed_ms=201,
    )
    cases.append(("unknown-execution-invalid-completion", e, "Error"))
    for reverse in (False, True):
        e = incomplete_observation_fixture()
        e["candidate_rows"][0]["attempts"][0]["execution_id"] = "shared-execution"
        if reverse:
            for left, right in [
                ("baseline_id", "candidate_id"),
                ("baseline_sha256", "candidate_sha256"),
                ("baseline_rows", "candidate_rows"),
            ]:
                e[left], e[right] = e[right], e[left]
        cases.append(
            ("compatible-partial-order-" + str(reverse), copy.deepcopy(e), "Incomplete")
        )
        partial = e["baseline_rows" if reverse else "candidate_rows"][0]["attempts"][0]
        partial["response"] = record(b"different response")
        partial["scoring"].update(
            response_sha256=partial["response"]["sha256"], normalized=list(b"no")
        )
        cases.append(("conflicting-partial-order-" + str(reverse), e, "Incomparable"))
    e = incomplete_observation_fixture()
    a = e["candidate_rows"][0]["attempts"][0]
    a.update(execution_id="shared-execution", start_ms=1)
    cases.append(("partial-fixed-start-conflict", e, "Incomparable"))
    e = incomplete_observation_fixture()
    a = e["candidate_rows"][0]["attempts"][0]
    a.update(execution_id="shared-execution", disposition="Stale")
    e["candidate_rows"][0]["eligible"] = False
    cases.append(("partial-known-terminal-conflict", e, "Incomparable"))
    for reverse in (False, True):
        for conflict in (False, True):
            e = observed_fixture()
            e["baseline_rows"][0]["attempts"][0]["response"] = None
            e["baseline_rows"][0]["correct"] = None
            e["candidate_rows"][0]["attempts"][0]["scoring"] = None
            e["candidate_rows"][0]["correct"] = None
            if conflict:
                e["candidate_rows"][0]["attempts"][0]["response"] = record(b"other")
            if reverse:
                for left, right in [
                    ("baseline_id", "candidate_id"),
                    ("baseline_sha256", "candidate_sha256"),
                    ("baseline_rows", "candidate_rows"),
                ]:
                    e[left], e[right] = e[right], e[left]
            cases.append(
                (
                    "complementary-partials-" + str(reverse) + "-" + str(conflict),
                    e,
                    "Incomparable" if conflict else "Incomplete",
                )
            )
    for changed_bound in ("input_bound", "output_cap"):
        _, e = legacy_observed_comparison()
        m = json.loads(bytes(e["experiment_manifest"]))
        e.update(
            baseline_id="v3",
            baseline_sha256=m["policies"][2]["artifact"]["sha256"],
            baseline_rows=copy.deepcopy(e["candidate_rows"]),
        )
        for row, c in zip(e["baseline_rows"], m["cases"]):
            for a, o in zip(
                row["attempts"], c["policy_opportunities"][2]["opportunities"]
            ):
                a["opportunity_id"] = o["opportunity_id"]
        manifest_edit(
            e,
            lambda m, k=changed_bound: m["cases"][1]["policy_opportunities"][2][
                "opportunities"
            ][1].update({k: 6 if k == "input_bound" else 9}),
        )
        cases.append(("shared-execution-" + changed_bound, e, "Incomparable"))
    e = incomplete_observation_fixture()
    e["candidate_rows"][0]["eligible"] = False
    cases.append(("unknown-usage-known-eligibility", e, "Error"))
    _, e = legacy_observed_comparison()
    a = e["candidate_rows"][1]["attempts"][0]
    a.update(
        execution_id="independent-terminal", disposition="Accepted", completed_ms=None
    )
    cases.append(("unknown-completion-terminal-cannot-retry", e, "Error"))
    _, e = legacy_observed_comparison()
    e["candidate_rows"][1]["attempts"][0]["completed_ms"] = None
    e["candidate_rows"][1].update(eligible=False, unsafe_acceptance=True)
    manifest_edit(
        e,
        lambda m: m["cases"][1]["policy_opportunities"][1]["opportunities"][1].update(
            admitted=False
        ),
    )
    cases.append(("unknown-completion-cannot-hide-next-denied-gate", e, "Error"))
    for key, value in [("charged", 1), ("completed_ms", 5)]:
        _, e = legacy_observed_comparison()
        r = e["candidate_rows"][1]
        r["attempts"] = r["attempts"][:1]
        r[key] = value
        cases.append(("missing-retry-below-known-prefix-" + key, e, "Error"))
    return cases


class FixRoundOneTests(unittest.TestCase):
    evaluate = ComparisonTests.evaluate

    def test_baseline_only_and_incomplete_known_fact_regressions(self):
        for name, e, want in fix_round_one_goldens():
            with self.subTest(name=name):
                try:
                    result = self.evaluate(e)
                    actual = result.get("reason", result["decision"])
                except ValueError:
                    actual = "Error"
                self.assertEqual(actual, want)

    def test_observed_conversion_preserves_known_per_attempt_facts(self):
        from policy_evaluation_v2 import convert_legacy

        old, e = legacy_observed_comparison()
        self.assertEqual(convert_legacy(old, e)["status"], "Complete")
        changes = [
            ("start", lambda a: a.update(start_ms=3)),
            (
                "input-output-split",
                lambda a: a.update(measured_input=3, measured_output=4),
            ),
            ("disposition", lambda a: a.update(disposition="Stale")),
            ("correctness", lambda a: a["scoring"].update(normalized=list(b"no"))),
        ]
        for name, change in changes:
            bad = copy.deepcopy(e)
            for side in ("baseline_rows", "candidate_rows"):
                change(bad[side][0]["attempts"][0])
            with self.subTest(name=name):
                self.assertEqual(convert_legacy(old, bad)["status"], "Incomplete")
        bad = copy.deepcopy(e)
        row = bad["candidate_rows"][1]
        row["attempts"][0]["completed_ms"] = 5
        row["attempts"][1]["start_ms"] = 6
        self.assertEqual(convert_legacy(old, bad)["status"], "Incomplete")


class LegacyNonexecutionTests(unittest.TestCase):
    def test_nonexecuted_attempt_preserves_preparation_and_zero_usage(self):
        from policy_evaluation_v2 import convert_legacy

        old, e = legacy_observed_comparison()
        old["cases"][0]["initial_context"]["admitted"] = False

        def edit(m):
            m["workload_sha256"] = hashlib.sha256(pack(old)).hexdigest()
            for p in m["cases"][0]["policy_opportunities"]:
                p["opportunities"][0]["admitted"] = False

        manifest_edit(e, edit)
        for side in ("baseline_rows", "candidate_rows"):
            r = e[side][0]
            a = r["attempts"][0]
            a.update(
                execution="NotExecuted",
                disposition="Refused",
                response=None,
                scoring=None,
                measured_input=0,
                measured_output=0,
                completed_ms=1,
            )
            r.update(
                outcome="Refused",
                correct=False,
                eligible=False,
                charged=0,
                completed_ms=1,
                false_refusal=True,
            )
        self.assertEqual(convert_legacy(old, e)["status"], "Complete")
        for key, value in [
            ("start_ms", 0),
            ("measured_input", 1),
            ("execution", "Executed"),
        ]:
            bad = copy.deepcopy(e)
            bad["candidate_rows"][0]["attempts"][0][key] = value
            with self.subTest(key=key):
                self.assertEqual(convert_legacy(old, bad)["status"], "Incomplete")

    def test_same_total_charge_cannot_shift_usage_between_attempts(self):
        from policy_evaluation_v2 import convert_legacy

        old, e = legacy_observed_comparison()
        row = e["candidate_rows"][1]
        row["attempts"][0]["execution_id"] = "independent-initial"
        row["attempts"][0]["measured_output"] -= 1
        row["attempts"][1]["measured_output"] += 1
        self.assertEqual(convert_legacy(old, e)["status"], "Incomplete")


class GoldenTests(unittest.TestCase):
    def test_frozen_golden_bytes_and_decisions(self):
        from policy_evaluation_v2 import (
            evaluate_policy_comparison,
            evaluation_sha256,
            parse_policy_evaluation,
        )

        e = fixture()
        self.assertEqual(
            evaluation_sha256(e),
            "2b0ed900f956cd8fc2dd215e114a665d4799187dd6610209807fe557a1311a88",
        )
        self.assertEqual(
            e["experiment_sha256"],
            "1373dd5ff2bc8e1a06840f8feb31329b1a6666d434712db2a6fe4a3ef537c265",
        )
        for name, value, want in golden_cases():
            with self.subTest(name=name):
                try:
                    result = evaluate_policy_comparison(
                        parse_policy_evaluation(pack(value))
                    )
                    actual = result.get("reason", result["decision"])
                except ValueError:
                    actual = "Error"
                self.assertEqual(actual, want)


class CandidateTests(unittest.TestCase):
    def test_candidate_requires_exact_artifact_digest_purpose_and_identity(self):
        from policy_evaluation_v2 import validate_policy_candidate

        e = fixture()
        m = json.loads(bytes(e["experiment_manifest"]))
        a = m["policies"][0]["artifact"]
        c = {
            "version_id": "v1",
            "actor": e["actor"],
            "scope": e["scope"],
            "purpose": e["purpose"],
            "artifact": a["bytes"],
            "sha256": a["sha256"],
        }
        self.assertEqual(validate_policy_candidate(c)["policy_name"], "always_stop")
        for key, value in [
            ("actor", " "),
            ("version_id", ""),
            ("purpose", "other"),
            ("sha256", "0" * 64),
        ]:
            bad = copy.deepcopy(c)
            bad[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_policy_candidate(bad)


if __name__ == "__main__":
    unittest.main()
