#!/usr/bin/env python3
"""Score the preregistered token-cap experiment from its five complete live traces."""
import argparse
import json
import sys
from collections import Counter, defaultdict
from pathlib import Path

CAPS = (32, 48, 64, 96)
FAMILIES = {
    "unit": "calibration", "integration": "calibration",
    "package": "heldout", "migration": "heldout", "compatibility": "heldout",
}
LIMIT = 1024
DEADLINE_MS = 60_000
MIN_CALIBRATION_SUCCESSES = 3  # 75% of four cases; fixed before any holdout generation.


def load_traces(paths):
    """Validate the paired design, token ledger, and score labels before summarizing."""
    cases = defaultdict(dict)
    seen = set()
    model = None
    for path in paths:
        with path.open(encoding="utf-8") as stream:
            data = json.load(stream)
        family = data["family"]
        if family not in FAMILIES or family in seen:
            raise ValueError(f"{path}: unknown or duplicate family {family!r}")
        seen.add(family)
        if data["mode"] != "live" or data.get("error") is not None:
            raise ValueError(f"{path}: expected a completed live evaluation")
        if data["budget_limit"] != LIMIT or data["deadline_ms"] != DEADLINE_MS:
            raise ValueError(f"{path}: budget or deadline differs from the preregistration")
        identity = data["model"], data["revision"]
        if not all(isinstance(x, str) and x for x in identity) or (model and identity != model):
            raise ValueError(f"{path}: incomplete or inconsistent model identity")
        model = identity
        expected_cases = {f"{family}-green", f"{family}-red"}
        for row in data["samples"]:
            case, cap = row["case_id"], row["cap"]
            context = f"{path}: {case} at cap {cap}"
            if row["family"] != family or row["split"] != FAMILIES[family] or case not in expected_cases:
                raise ValueError(f"{context}: wrong family, case or split")
            if cap not in CAPS or cap in cases[case]:
                raise ValueError(f"{context}: invalid or duplicate cap")
            for field in ("cap", "trial_order", "preflight_input_tokens", "input_tokens",
                          "output_tokens", "charged_tokens", "reserved_tokens", "receipt_us"):
                if type(row[field]) is not int or row[field] < 0:
                    raise ValueError(f"{context}: invalid {field}")
            for field in ("dispatched", "ended_eos", "hit_cap", "valid", "correct", "on_time", "success"):
                if type(row[field]) is not bool:
                    raise ValueError(f"{context}: invalid {field}")
            prompt, spent, generated = row["preflight_input_tokens"], row["charged_tokens"], row["output_tokens"]
            requested = prompt + cap
            if prompt == 0 or row["requested_reservation_tokens"] != requested:
                raise ValueError(f"{context}: reservation does not match preflight")
            if row["dispatched"]:
                if row["input_tokens"] != prompt or not (0 <= generated <= cap):
                    raise ValueError(f"{context}: actual input or output count differs from preflight contract")
                if row["reserved_tokens"] != requested or row["hit_cap"] != (generated == cap):
                    raise ValueError(f"{context}: inconsistent cap or reservation")
            elif (row["input_tokens"] or generated or row["reserved_tokens"] or row["hit_cap"]
                  or row["ended_eos"] or row["valid"] or row["correct"]):
                raise ValueError(f"{context}: denied row contains execution measurements")
            if spent != row["input_tokens"] + generated or not (spent <= row["reserved_tokens"] <= LIMIT):
                raise ValueError(f"{context}: unreconciled token accounting")
            if row["spent_after"] != spent or row["outstanding_after"] != 0 or row["budget_limit"] != LIMIT:
                raise ValueError(f"{context}: final ledger state does not match the row")
            if row["dispatched"] and row["disposition"] != ("accepted" if row["valid"] and row["correct"] else "invalid_output"):
                raise ValueError(f"{context}: terminal disposition conflicts with verdict")
            if row["deadline_ms"] != DEADLINE_MS or row["on_time"] != (row["receipt_us"] <= DEADLINE_MS * 1000):
                raise ValueError(f"{context}: inconsistent caller deadline")
            if row["correct"] and not row["valid"]:
                raise ValueError(f"{context}: correct judgment has an invalid schema")
            if row["success"] != (row["dispatched"] and row["valid"] and row["correct"] and row["on_time"]):
                raise ValueError(f"{context}: inconsistent completion score")

            # Within a paired case only the output cap may change in the frozen request.
            envelope = json.loads(row["request_utf8"])
            if envelope["generation"].pop("max_new_tokens") != cap or (envelope["model"], envelope["revision"]) != model:
                raise ValueError(f"{context}: request conflicts with experiment metadata")
            if cases[case]:
                first = next(iter(cases[case].values()))
                if envelope != first["envelope"] or prompt != first["preflight_input_tokens"]:
                    raise ValueError(f"{context}: paired trials must use identical prompt and decoding settings")
            cases[case][cap] = dict(row, envelope=envelope)
        for case in expected_cases:
            if set(cases[case]) != set(CAPS) or {r["trial_order"] for r in cases[case].values()} != set(range(4)):
                raise ValueError(f"{path}: incomplete cap grid/order for {case}")
    if seen != set(FAMILIES):
        raise ValueError(f"missing family traces: {', '.join(sorted(set(FAMILIES) - seen))}")
    return cases


def summarize(cases):
    calibration = [grid for grid in cases.values() if grid[CAPS[0]]["split"] == "calibration"]
    calibration_counts = {cap: sum(grid[cap]["success"] for grid in calibration) for cap in CAPS}
    eligible = [cap for cap in CAPS if calibration_counts[cap] >= MIN_CALIBRATION_SUCCESSES]

    heldout = [grid for grid in cases.values() if grid[CAPS[0]]["split"] == "heldout"]
    lines = [
        "## Calibration", "",
        "Caps qualify with at least 3 correct, valid, on-time completions among 4 calibration cases.",
        "", "| Output cap | Successes / 4 | Qualifies |", "| ---: | ---: | --- |",
    ]
    for cap in CAPS:
        lines.append(f"| {cap} | {calibration_counts[cap]} | {'yes' if cap in eligible else 'no'} |")

    lines += [
        "", "## Held-out cap sweep", "",
        "Each row covers the same six cases. Token totals include unsuccessful and late calls. "
        "Reaching the cap can coincide with EOS; a length stop counts a cap hit without EOS.",
        "", "| Cap | Dispatched | Schema valid | Correct | Correct, valid, on time / 6 | Cap hits | Length stops | Charged tokens |",
        "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for cap in CAPS:
        group = [grid[cap] for grid in heldout]
        counts = [sum(r[key] for r in group) for key in ("dispatched", "valid", "correct", "success", "hit_cap")]
        stops = sum(r["hit_cap"] and not r["ended_eos"] for r in group)
        lines.append("| " + " | ".join(map(str, [cap, *counts, stops, sum(r["charged_tokens"] for r in group)])) + " |")

    lines += [
        "", "## Matched policy comparison", "",
        "The simulated selector uses only calibration outcomes, the held-out prompt's exact preflight count "
        "and the 1,024-token allowance. Each selected outcome comes from that case's measured generation "
        "at the selected cap. Abstentions remain in the six-case denominator.",
        "", "| Policy | Cap selections | Dispatched | Abstained | Correct, valid, on time / 6 | Charged tokens | Successes / 1,000 tokens |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for name, candidates in [("Calibrated", eligible), ("Fixed 96", [96])]:
        chosen = []
        for grid in heldout:
            prompt = grid[CAPS[0]]["preflight_input_tokens"]
            cap = next((c for c in candidates if prompt + c <= LIMIT), None)
            if cap is not None:
                chosen.append(grid[cap])
        tokens = sum(row["charged_tokens"] for row in chosen)
        success = sum(row["success"] for row in chosen)
        dispatches = sum(row["dispatched"] for row in chosen)
        selections = Counter(row["cap"] for row in chosen)
        label = ", ".join(f"{cap} × {count}" for cap, count in sorted(selections.items())) or "none"
        rate = f"{1000 * success / tokens:.2f}" if tokens else "n/a"
        lines.append(f"| {name} | {label} | {dispatches} | {len(heldout)-dispatches} | {success} | {tokens} | {rate} |")

    samples = [row for grid in cases.values() for row in grid.values()]
    calls = sum(row["dispatched"] for row in samples)
    prompt_tokens = sum(row["input_tokens"] for row in samples)
    generated_tokens = sum(row["output_tokens"] for row in samples)
    tokens = prompt_tokens + generated_tokens
    lines += ["", f"The complete sweep used {calls} model calls and {tokens:,} tokens across 40 measured cells "
              f"({prompt_tokens:,} input and {generated_tokens:,} output). "
              "The archive replay is accounted separately. These are paired descriptive results on three "
              "held-out fixture families; six cases cannot establish a production success probability."]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("traces", type=Path, nargs="+", help="one JSON trace per preregistered family")
    args = parser.parse_args()
    try:
        print(summarize(load_traces(args.traces)))
    except (KeyError, TypeError, ValueError, OSError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
