# Benchmarking selective revision

This benchmark asks how much wall time it takes to create a fresh SQLite database, publish a catalogue of evidence-support queries, and keep their answers current through a sequence of edits. Each arm receives the same catalogue and ordered edits, produces the same changed verdicts, and persists the answers.

## Reproduce

Run the release executable on a local filesystem. A C compiler is required for bundled SQLite. Set `TMPDIR` to choose the filesystem. Each measured arm gets a fresh file.

```sh
cargo build --release --example revision_bench
target/release/examples/revision_bench > samples.csv
python3 scripts/summarize_revision_bench.py samples.csv
```

To run a single cell, supply the scenario, number of rules, number of edits, and number of paired repetitions. Even repetition counts balance the two execution orders.

```sh
target/release/examples/revision_bench sparse 128 32 6
```

## What is compared

| | Indexed | Full scan |
| --- | --- | --- |
| Implementation | Production `Store`, using admission, consumer discovery, revision and current reads | Benchmark-only SQLite implementation, single writer |
| Rule storage | Persisted clauses, missing-premise counts, reverse dependency index | Persisted definitions and an in-memory copy of the catalogue |
| Edit | Update affected clauses, visit dependent branches, publish changed answers | Load current memberships once and evaluate the whole catalogue in memory |
| Durability | Per-occurrence seed operations and per-branch publication/revision transactions with the production attempt ledger | Batches seeding, registration, and each edit with its changed verdicts. Stores admission and verdict history |
| Shared conditions | SQLite file on the same filesystem, synchronous FULL, default rollback journal, identical input and answer interface | Same |

The scan models a caller that can give up independent attempt accounting and per-branch publication. The ratio includes those transaction and history costs. This is an end-to-end implementation comparison. It cannot attribute a difference solely to the reverse index.

Each fixture has two three-premise alternatives per query. All evidence starts admitted. With an even catalogue size, sparse edits toggle a local observation used by two queries and one verdict changes. Dense edits toggle a shared observation used by every query and half change. Even-numbered edits readmit the previously withdrawn observation. Seed 20261113 fixes the order of local keys. The two edit patterns share one catalogue for each size.

Each timing includes open/schema construction, evidence seeding, registration, all edits, and database close. The CSV reports microseconds by phase. The total additionally includes close. File size is the main database after close, excluding Rust heap memory. Fixture generation, compilation, oracle evaluation, printing, and deletion are outside the timings. Every timed arm returns the complete sequence of changed verdicts, checked after the timer stops. Before timing each cell, a separate file-backed replay verifies every persisted branch verdict after registration and after every edit against a full evaluator.

The standard matrix uses six paired repetitions, alternating arm order. It covers 128 rules with 1 and 32 edits at both fanouts, plus 1,024 rules with 128 sparse edits. Timings describe repeated runs of these fixed, synthetic traces on a single runner. They do not estimate fleet-wide performance or a general break-even point.

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

The next experiment is a batched production path that preserves the existing head, provenance, and accounting contracts. Measure registration, evidence admission, and multi-branch publication separately on the same traces. Transaction granularity is a plausible contributor; the present comparison does not isolate its effect from the different storage and history contracts.

Environment: GitHub-hosted Linux x86-64, AMD EPYC 7763, ext4, runner image `ubuntu26 20261111.196.1`, Rust 1.99.1 and bundled SQLite 3.46.0. Measurements came from benchmark commit `1caea4f`. [CI run](https://github.com/k2jac9/kairotensor/actions/runs/43472340750) · [Raw paired samples](benchmarks/revision-2026-11-13.csv).
