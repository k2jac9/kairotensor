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
