# End-to-end timed recovery runner (offline only)

This runner connects evidence capture, request preparation, the production timed
runtime wrapper, validation and settlement into one auditable fixture. It uses
the existing offline Adapter without changing the archived adapter behavior.
It is not a live model evaluation and does not establish a production latency
profile or model-accuracy improvement.

## Run safely

Run `cargo run --example timed_recovery_runner -- --out /tmp/timed-run` with a new
output directory. The runner refuses an existing destination rather than
replacing earlier evidence. It rejects `--live` and invalid/duplicate arguments
before creating files or starting a backend. There is no automatic resume or
resend of an uncertain operation.

The fixed report is a contract fixture. Real arithmetic report verdicts should
use the deterministic Store route; routing this fixture through the Adapter tests
the recovery machinery, not whether a model is useful for arithmetic.

## Accounting and clocks

The existing offline Adapter has no tokenizer measurements or prepared tensor
handles. A small bridge counts UTF-8 bytes in the exact frozen request and output
as synthetic token units. These are not Qwen tokenizer counts and do not verify
the live worker's prepared-handle protocol. Its profile and timing allowance are
explicitly synthetic. Historical live calibration is not promoted into a valid
profile for this runner.

Evidence comes from the Store's admitted capture, not a caller-supplied answer or
reference label. The immutable request, synthetic quote and profile are used at
reservation and checked again through `run_timed_recovery`. A callback executes
only after its own single-use claim and final clock checks. Strict output
validation determines accepted versus invalid delivery; Store eligibility can
still reject stale work independently.

Synthetic operation-clock readings drive injected timing scenarios. Actual local
elapsed measurements are recorded separately and must not be presented as model
latency or mixed with the synthetic deadline. Completion timing is distinct from
admission, semantic correctness and ledger settlement. A missing completion-clock
reading does not make known measured usage disappear.

## Evidence and failures

The output includes SQLite accounting and immutable request/response provenance,
plus a flushed raw journal. Returned reply bytes are recorded before validation
and settlement where possible. Partial audit output, transport uncertainty and
measured invalid output must remain distinguishable. Filesystem or timing faults
must not fabricate zero usage, release a potentially-sent hold, or cause a resend.

The offline scenarios cover complete and unavailable evidence, changes before
claim, queue/commit delay, invalid output, callback uncertainty and settlement
failure. Restart and duplicate tests reopen the same durable state and verify
that claimed work is not invoked twice. Failed settlement retains measurements
for later reconciliation. These are fault-injection tests, not proof against every
OS crash or a guarantee of exactly-once external transport.

No paid inference is enabled. A later live integration needs a frozen task,
representative calibration for its exact request/runtime/readiness, an explicit
spending limit and separately verified raw results.

## Artifacts and scenarios

The exclusive directory contains `manifest.json`, `events.jsonl`, `store.sqlite`
and `summary.json`. An absent summary means collection did not complete. Summary
flags separately describe collection, audit completeness and whether accounting
is reconciled. A completed fault scenario can intentionally retain a hold.

Clock and adapter-entry events are buffered in memory. Clock events flush after
the runtime returns; the entry event flushes after Adapter dispatch returns. This
keeps journal I/O out of the lock and final send window. Sort journal events by their `sequence` field to recover observed
order; physical JSONL append order is not the execution order. Original local
elapsed snapshots are retained. This instrumentation still does not imply a hard
real-time live transport guarantee.

Use `--scenario` with `happy`, `missing`, `stale`, `queue-delay`, `commit-delay`,
`uncertain`, `malformed`, `settlement-failure`, `malformed-settlement-failure`,
`audit-failure`, or
`completion-clock-unavailable`. Use a fresh output directory for every scenario.
An injected lost reply is deliberately treated as unknown; an observed reply with
missing timing or journal coverage still retains its measured synthetic charge.

A failed settlement preserves `reconciliation_reply`: exact output bytes, both
synthetic usage counts, and the full valid/invalid decision with its original
validator and reason. Reconciliation must use those recorded facts, not rerun the
Adapter or reconstruct validation from a later version of the runner.
