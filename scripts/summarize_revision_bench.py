#!/usr/bin/env python3
"""Summarize paired Kairotensor revision benchmark CSV rows as Markdown."""

import argparse
import csv
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from statistics import median


HEADER = (
    "scenario,rules,edits,trial,first,arm,open_us,seed_us,register_us,"
    "edit_us,total_us,db_bytes,changes"
).split(",")
ARMS = ("indexed", "scan")


@dataclass(frozen=True)
class Sample:
    first: str
    setup_us: int
    edit_us: int
    total_us: int
    db_bytes: int
    changes: int


def read_cells(stream):
    """Pair each trial before calculating any aggregate."""
    reader = csv.reader((line for line in stream if not line.startswith("#")), strict=True)
    if next(reader, None) != HEADER:
        raise ValueError("unexpected CSV header; expected " + ",".join(HEADER))
    trials = defaultdict(dict)
    for fields in reader:
        if not fields:
            continue
        if len(fields) != len(HEADER):
            raise ValueError(f"CSV row {reader.line_num}: expected {len(HEADER)} fields")
        row = dict(zip(HEADER, fields))
        scenario, arm, first = row["scenario"], row["arm"], row["first"]
        if scenario not in ("sparse", "dense") or arm not in ARMS or first not in ARMS:
            raise ValueError(f"CSV row {reader.line_num}: unknown scenario, arm, or first arm")
        numbers = {}
        for name in ("rules", "edits", "trial", *HEADER[6:]):
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
        trials[key][arm] = Sample(first, setup, edits, total, numbers["db_bytes"], numbers["changes"])
    if not trials:
        raise ValueError("no benchmark samples")
    cells = defaultdict(list)
    for key, pair in trials.items():
        if set(pair) != set(ARMS):
            raise ValueError(f"incomplete pair for {key}")
        indexed, scan = pair["indexed"], pair["scan"]
        if indexed.first != scan.first or indexed.changes != scan.changes:
            raise ValueError(f"first arm or change count differs within pair {key}")
        cells[key[:3]].append((indexed, scan))
    return cells


def format_summary(cells):
    header = (
        "| Scenario | Rules | Edits | Pairs | Setup ms (I / S) | Edit ms (I / S) | "
        "Total ms (I / S) | S / I total, median [min, max] | Database KiB (I / S) |"
    )
    lines = [
        "I = indexed and S = full scan. Times and file sizes are medians. "
        "Ratios are computed within each pair. Values above 1 favor indexed.",
        "",
        header,
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for (scenario, size, edits), pairs in sorted(cells.items()):
        indexed, scan = zip(*pairs)
        metrics = []
        for name, scale in (("setup_us", 1000), ("edit_us", 1000),
                            ("total_us", 1000), ("db_bytes", 1024)):
            values = [
                median(getattr(sample, name) for sample in arm) / scale
                for arm in (indexed, scan)
            ]
            metrics.append(" / ".join(f"{value:.3f}" for value in values))
        setup, edit, total, storage = metrics
        ratios = [s.total_us / i.total_us for i, s in pairs]
        lines.append(
            f"| {scenario} | {size} | {edits} | {len(pairs)} | {setup} | {edit} | {total} | "
            f"{median(ratios):.3f} [{min(ratios):.3f}, {max(ratios):.3f}] | {storage} |"
        )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", nargs="?", help="CSV file; omit or use - for stdin")
    args = parser.parse_args()
    try:
        if args.file in (None, "-"):
            cells = read_cells(sys.stdin)
        else:
            with Path(args.file).open(encoding="utf-8", newline="") as stream:
                cells = read_cells(stream)
        print(format_summary(cells))
    except (ValueError, csv.Error, OSError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
