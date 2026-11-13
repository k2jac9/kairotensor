#!/usr/bin/env python3
"""Summarize matched Kairotensor revision benchmark trials as Markdown."""

import argparse
import csv
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from statistics import median


PAIR_HEADER = (
    "scenario,rules,edits,trial,first,arm,open_us,seed_us,register_us,"
    "edit_us,total_us,db_bytes,changes"
).split(",")
TRIPLE_HEADER = ["order" if field == "first" else field for field in PAIR_HEADER]
PAIR_ARMS = ("indexed", "scan")
TRIPLE_ARMS = ("indexed", "batch", "scan")


@dataclass(frozen=True)
class Sample:
    order: tuple[str, ...]
    setup_us: int
    edit_us: int
    total_us: int
    db_bytes: int
    changes: int


def read_cells(stream):
    """Keep the reported arm order and validate each trial before aggregation."""
    reader = csv.reader((line for line in stream if not line.startswith("#")), strict=True)
    header = next(reader, None)
    if header == PAIR_HEADER:
        arms = PAIR_ARMS
    elif header == TRIPLE_HEADER:
        arms = TRIPLE_ARMS
    else:
        raise ValueError("unexpected CSV header; expected a paired 'first' or triplet 'order' header")
    trials = defaultdict(dict)
    for fields in reader:
        if not fields:
            continue
        if len(fields) != len(header):
            raise ValueError(f"CSV row {reader.line_num}: expected {len(header)} fields")
        row = dict(zip(header, fields))
        scenario, arm = row["scenario"], row["arm"]
        if scenario not in ("sparse", "dense") or arm not in arms:
            raise ValueError(f"CSV row {reader.line_num}: unknown scenario or arm")
        if arms == PAIR_ARMS:
            first = row["first"]
            if first not in arms:
                raise ValueError(f"CSV row {reader.line_num}: unknown first arm")
            order = (first, next(other for other in arms if other != first))
        else:
            order = tuple(row["order"].split("/"))
            if len(order) != len(arms) or set(order) != set(arms):
                raise ValueError(f"CSV row {reader.line_num}: order must be a permutation of {arms}")
        numbers = {}
        for name in ("rules", "edits", "trial", *header[6:]):
            value = row[name]
            if not value.isascii() or not value.isdecimal():
                raise ValueError(f"CSV row {reader.line_num}: {name} must be a nonnegative integer")
            numbers[name] = int(value)
        setup = numbers["open_us"] + numbers["seed_us"] + numbers["register_us"]
        edits, total = numbers["edit_us"], numbers["total_us"]
        if not numbers["rules"] or not total:
            raise ValueError(f"CSV row {reader.line_num}: rules and total_us must be positive")
        # Four independently rounded phase durations may differ by up to 2 us.
        if total + 2 < setup + edits:
            raise ValueError(f"CSV row {reader.line_num}: total_us is shorter than its phases")
        key = (scenario, numbers["rules"], numbers["edits"], numbers["trial"])
        if arm in trials[key]:
            raise ValueError(f"duplicate {arm} sample for {key}")
        trials[key][arm] = Sample(order, setup, edits, total, numbers["db_bytes"], numbers["changes"])
    if not trials:
        raise ValueError("no benchmark samples")
    cells = defaultdict(list)
    for key, trial in trials.items():
        if set(trial) != set(arms):
            raise ValueError(f"incomplete trial for {key}")
        samples = tuple(trial[arm] for arm in arms)
        if any(s.order != samples[0].order or s.changes != samples[0].changes for s in samples):
            raise ValueError(f"arm order or change count differs within trial {key}")
        if arms == TRIPLE_ARMS and tuple(trial) != samples[0].order:
            raise ValueError(f"observed row order differs from declared execution order for {key}")
        cells[key[:3]].append(samples)
    return arms, cells


def ratio_range(ratios):
    return f"{median(ratios):.3f} [{min(ratios):.3f}, {max(ratios):.3f}]"


def format_summary(arms, cells):
    if arms == PAIR_ARMS:
        description = (
            "I = indexed and S = full scan. Times and file sizes are medians. "
            "Ratios are computed within each pair. Values above 1 favor indexed."
        )
        names = "I / S"
        count = "Pairs"
        ratio_headers = ["S / I total, median [min, max]"]
    else:
        description = (
            "I = per-branch indexed, B = atomic batch, S = full scan. "
            "Times and database sizes are medians. Ratios are calculated within each trial "
            "and reported as median [min, max]. Values above 1 favor batch. "
            "An edit ratio is unavailable for zero edits or a zero-microsecond edit time in either indexed arm."
        )
        names = "I / B / S"
        count = "Triplets"
        ratio_headers = ["I / B edit", "S / B total"]
    columns = [
        "Scenario", "Rules", "Edits", count, f"Setup ms ({names})", f"Edit ms ({names})",
        f"Total ms ({names})", *ratio_headers, f"Database KiB ({names})",
    ]
    lines = [
        description, "", "| " + " | ".join(columns) + " |",
        "| --- |" + " ---: |" * (len(columns) - 1),
    ]
    for (scenario, size, edits), trials in sorted(cells.items()):
        metrics = []
        for name, scale in (("setup_us", 1000), ("edit_us", 1000),
                            ("total_us", 1000), ("db_bytes", 1024)):
            values = [
                median(getattr(sample, name) for sample in arm) / scale
                for arm in zip(*trials)
            ]
            metrics.append(" / ".join(f"{value:.3f}" for value in values))
        setup, edit, total, storage = metrics
        if arms == PAIR_ARMS:
            ratios = [ratio_range([s.total_us / i.total_us for i, s in trials])]
        else:
            edit_ratio = "n/a"
            if edits and all(i.edit_us and b.edit_us for i, b, _ in trials):
                edit_ratio = ratio_range([i.edit_us / b.edit_us for i, b, _ in trials])
            ratios = [edit_ratio, ratio_range([s.total_us / b.total_us for _, b, s in trials])]
        values = [scenario, size, edits, len(trials), setup, edit, total, *ratios, storage]
        lines.append("| " + " | ".join(map(str, values)) + " |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", nargs="?", help="CSV file; omit or use - for stdin")
    args = parser.parse_args()
    try:
        if args.file in (None, "-"):
            arms, cells = read_cells(sys.stdin)
        else:
            with Path(args.file).open(encoding="utf-8", newline="") as stream:
                arms, cells = read_cells(stream)
        print(format_summary(arms, cells))
    except (ValueError, csv.Error, OSError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
