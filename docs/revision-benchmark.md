# Benchmarking selective revision

This benchmark asks how much wall time it takes to create a fresh SQLite database, publish a catalogue of evidence-support queries, and keep their answers current through a sequence of edits. Each arm receives the same catalogue and ordered edits, produces the same changed verdicts, and persists the answers.

## Reproduce

Run the release executable on a local filesystem. A C compiler is required for bundled SQLite. Set `TMPDIR` to choose the filesystem. Each measured arm gets a fresh file.

```sh
cargo build --release --example revision_bench
target/release/examples/revision_bench > samples.csv
python3 scripts/summarize_revision_bench.py samples.csv
```

To run a single cell, supply the scenario, number of rules, number of edits, and number of repetitions. Multiples of six run every ordering of the three implementations equally often.

```sh
target/release/examples/revision_bench sparse 128 32 6
```

## What is compared

| | Per-branch indexed | Atomic indexed batch | Full scan |
| --- | --- | --- | --- |
| Implementation | Production `Store`, explicit consumer discovery and revision | Production `Store::set_admitted_and_revise` | Single-writer benchmark baseline |
| Setup | Record occurrences, seed admissions, register indexed rules and initial heads | Same setup as the per-branch arm | Batch seed and registration. Persist definitions and cache the catalogue in memory |
| Edit | Commit membership, then visit and revise dependent branches in separate transactions | Commit membership, clause updates, and every changed branch head together | Load admissions once, evaluate the catalogue in memory, and commit the edit and changed verdicts together |
| Publication history | Per-branch attempt, parent, binding, delivery and usage | Same per-branch history and charges | Admission and verdict history |
| Shared conditions | File-backed SQLite on the same filesystem, synchronous FULL, default rollback journal, identical fixtures and expected answers | Same | Same |

The indexed comparison holds setup and publication semantics fixed. It measures the combined effect of batching, consumer lookup and SQL execution. The full scan implements a lighter history contract and batches setup too. End-to-end timing includes these differences.

Each fixture has two three-premise alternatives per query. All evidence starts admitted. With an even catalogue size, sparse edits toggle a local observation used by two queries and one verdict changes. Dense edits toggle a shared observation used by every query and half change. Even-numbered edits readmit the previously withdrawn observation. Seed 20261113 fixes the order of local keys. The two edit patterns share one catalogue for each size.

Each timing includes open/schema construction, evidence seeding, registration, all edits, and database close. The CSV reports microseconds by phase. The total additionally includes close. File size is the main database after close, excluding Rust heap memory. Fixture generation, compilation, oracle evaluation, printing, and deletion are outside the timings. Every timed arm returns the complete sequence of changed verdicts, checked after the timer stops. Before timing each cell, a separate file-backed replay verifies every persisted branch verdict after registration and after every edit against a full evaluator.

The standard matrix uses six matched repetitions, one per permutation of the three arms. It covers 128 rules with 1 and 32 edits at both fanouts, plus 1,024 rules with 128 sparse edits. Timings describe repeated runs of these fixed, synthetic traces on a single runner. They do not estimate fleet-wide performance or a general break-even point.

## First measured run

On 13 November 2026 the batched full scan finished sooner in all 30 paired trials. Every timed change stream matched the full evaluator. Both separate verification replays for each cell matched the persisted verdicts after every edit.

I = indexed and S = full scan. Times and file sizes are medians. Ratios are computed within each pair. Values above 1 favor indexed.

| Scenario | Rules | Edits | Pairs | Setup ms (I / S) | Edit ms (I / S) | Total ms (I / S) | S / I total, median [min, max] | Database KiB (I / S) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| dense | 128 | 1 | 6 | 281.272 / 3.406 | 70.355 / 0.897 | 354.380 / 4.335 | 0.012 [0.011, 0.013] | 140.000 / 32.000 |
| dense | 128 | 32 | 6 | 272.948 / 3.498 | 2239.867 / 31.340 | 2510.717 / 35.611 | 0.014 [0.013, 0.015] | 280.000 / 56.000 |
| sparse | 128 | 1 | 6 | 317.850 / 3.982 | 2.233 / 0.994 | 320.139 / 4.960 | 0.016 [0.013, 0.018] | 132.000 / 32.000 |
| sparse | 128 | 32 | 6 | 282.759 / 3.301 | 68.299 / 27.589 | 351.341 / 31.197 | 0.087 [0.083, 0.093] | 140.000 / 32.000 |
| sparse | 1024 | 128 | 6 | 2701.729 / 8.479 | 283.374 / 120.273 | 2985.164 / 128.958 | 0.043 [0.039, 0.046] | 708.000 / 136.000 |

In the 1,024-rule sparse workload only two queries depended on each edit and one verdict changed. Median setup took 2.702 s indexed and 8.479 ms for the full scan. Even after setup, the 128 edits took 283.374 ms indexed and 120.273 ms for the scan. Dense edits made the difference larger: resolving 32 edits across 128 affected branches per edit took a median 2.240 s indexed and 31.340 ms for the scan, excluding setup.

These results motivated testing a batched production path that preserves the existing head, provenance, and accounting contracts.

Environment: GitHub-hosted Linux x86-64, AMD EPYC 7763, ext4, runner image `ubuntu26 20261111.196.1`, Rust 1.99.1 and bundled SQLite 3.46.0. Measurements came from benchmark commit `1caea4f`. [CI run](https://github.com/k2jac9/kairotensor/actions/runs/43472340750) · [Raw paired samples](benchmarks/revision-2026-11-13.csv).

## Atomic production path, 13 November 2026

The atomic production path reduced edit time in all 30 matched comparisons with the per-branch API in the run below. With 128 rules and 32 dense edits, the median within-trial edit speedup was 13.46× (range 13.24–13.56×). The median edit phase fell from 1,972.5 ms to 146.7 ms. At 1,024 rules and 128 sparse edits, the median within-trial speedup was 1.40× (range 1.34–1.68×).

I = per-branch indexed, B = atomic batch, S = full scan. Times and database sizes are medians. Ratios are calculated within each trial and reported as median [min, max]. Values above 1 favor batch. An edit ratio is unavailable for zero edits or a zero-microsecond edit time in either indexed arm.

| Scenario | Rules | Edits | Triplets | Setup ms (I / B / S) | Edit ms (I / B / S) | Total ms (I / B / S) | I / B edit | S / B total | Database KiB (I / B / S) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| dense | 128 | 1 | 6 | 226.668 / 231.425 / 3.033 | 62.100 / 4.580 / 0.725 | 293.450 / 236.067 / 3.769 | 13.676 [12.080, 15.952] | 0.016 [0.015, 0.097] | 140.000 / 140.000 / 32.000 |
| dense | 128 | 32 | 6 | 226.686 / 221.019 / 2.959 | 1972.508 / 146.650 / 24.859 | 2198.191 / 369.123 / 27.755 | 13.460 [13.244, 13.563] | 0.075 [0.073, 0.102] | 280.000 / 280.000 / 56.000 |
| sparse | 128 | 1 | 6 | 238.065 / 242.812 / 2.949 | 1.718 / 1.055 / 0.684 | 239.931 / 243.948 / 3.738 | 1.731 [1.382, 1.918] | 0.016 [0.012, 0.017] | 132.000 / 132.000 / 32.000 |
| sparse | 128 | 32 | 6 | 231.900 / 233.962 / 3.084 | 57.136 / 30.514 / 22.049 | 292.346 / 263.330 / 25.122 | 1.898 [1.673, 2.598] | 0.094 [0.087, 0.106] | 140.000 / 140.000 / 32.000 |
| sparse | 1024 | 128 | 6 | 2473.183 / 2477.150 / 8.067 | 258.505 / 189.174 / 101.647 | 2736.232 / 2660.274 / 109.905 | 1.397 [1.344, 1.682] | 0.041 [0.040, 0.046] | 708.000 / 708.000 / 136.000 |

The batched full scan still had the lowest end-to-end time in all 30 trials and returned the same verdicts. In the largest sparse cell, median setup for the atomic indexed path was 2,477 ms, followed by 189 ms for the edits. Its separate median seed and registration times were 706 ms and 1,773 ms. This points to bulk evidence admission and rule registration as the next optimization to measure. Both production arms used the same registration code and had the same final database size in each cell.

All comparisons above use measurements collected together on the same runner. An earlier pre-merge measurement on another runner also favored batch on median edit time in each of the five cells, with greater variation in the sparse cases. These are synthetic single-writer workloads. Simultaneous reader or writer throughput and tail latency require separate measurement.

Environment for the reported run: GitHub-hosted Linux x86-64, Intel Xeon Platinum 8370C, ext4, runner image `ubuntu26 20261111.196.1`, Rust 1.99.1, SQLite 3.46.0, benchmark commit `5d699c4`. [Measured CI run](https://github.com/k2jac9/kairotensor/actions/runs/43476626433) · [All 90 raw timings](benchmarks/atomic-revision-2026-11-13.csv) · [Earlier run](https://github.com/k2jac9/kairotensor/actions/runs/43476460487).
