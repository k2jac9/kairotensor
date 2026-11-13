"""Offline policy and trace-integrity regressions for the preregistered pilot."""
import json
from pathlib import Path
import tempfile
import unittest

from summarize_token_caps import CAPS, DEADLINE_MS, FAMILIES, LIMIT, PINNED_MODEL, load_traces, summarize


def scoring_cases():
    cases = {}
    for family, split in FAMILIES.items():
        for label in ("green", "red"):
            case = f"{family}-{label}"
            cases[case] = {}
            for cap in CAPS:
                # 32 qualifies on 2/4; 48 on 3/4; 64 and 96 on 4/4.
                success = (split == "heldout" or cap >= 64 or
                           (cap == 48 and case != "integration-red") or
                           (cap == 32 and family == "unit"))
                cases[case][cap] = dict(split=split, cap=cap, preflight_input_tokens=150,
                                        success=success, valid=success, correct=success, dispatched=True,
                                        hit_cap=False, ended_eos=True, input_tokens=150, output_tokens=20,
                                        charged_tokens=170)
    return cases


def calibrated_row(cases):
    return next(line for line in summarize(cases).splitlines() if line.startswith("| Calibrated |"))


class TokenCapSummaryTests(unittest.TestCase):
    def test_smallest_cap_at_target_and_heldout_labels_cannot_change_selection(self):
        cases = scoring_cases()
        self.assertIn("| 48 × 6 | 6 | 0 | 6 |", calibrated_row(cases))
        for grid in cases.values():
            for r in grid.values():
                if r["split"] == "heldout":
                    r["success"] = r["valid"] = r["correct"] = r["cap"] == 96
        self.assertIn("| 48 × 6 | 6 | 0 | 0 |", calibrated_row(cases))

    def test_abstain_even_when_all_heldout_outcomes_would_work(self):
        cases = scoring_cases()
        for grid in cases.values():
            for r in grid.values():
                if r["split"] == "calibration":
                    r["success"] = r["valid"] = r["correct"] = False
        self.assertIn("| none | 0 | 6 | 0 | 0 | n/a |", calibrated_row(cases))

    def test_unreconciled_ledger_rejected_before_scoring(self):
        envelope = dict(model=PINNED_MODEL[0], revision=PINNED_MODEL[1], generation={"max_new_tokens": 32})
        row = dict(case_id="unit-green", family="unit", split="calibration", cap=32, trial_order=0,
                   preflight_input_tokens=150, input_tokens=150, output_tokens=20, charged_tokens=170,
                   reserved_tokens=182, requested_reservation_tokens=182, receipt_us=500,
                   dispatched=True, ended_eos=True, hit_cap=False, valid=True, correct=True,
                   on_time=True, success=True, budget_limit=LIMIT, deadline_ms=DEADLINE_MS,
                   disposition="accepted", spent_after=170, outstanding_after=0, request_utf8=json.dumps(envelope))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "unit.json")
            for mutation in ({"spent_after": 169}, {"outstanding_after": 1}):
                with self.subTest(mutation=mutation):
                    path.write_text(json.dumps(dict(family="unit", mode="live", error=None,
                                         budget_limit=LIMIT, deadline_ms=DEADLINE_MS,
                                         model=PINNED_MODEL[0], revision=PINNED_MODEL[1], samples=[row | mutation])))
                    with self.assertRaisesRegex(ValueError, "final ledger state"):
                        load_traces([path])


if __name__ == "__main__":
    unittest.main()
