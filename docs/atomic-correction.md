# Atomic admission corrections

`Store::revise_admissions_if_current(actor, scope, expected_epoch, edits)` applies
one correction against a known actor/scope view. An `AdmissionEdit` names an
existing occurrence and its requested admission. A correction can withdraw old
observations and admit replacements without publishing intermediate verdicts.
The embedding runtime authenticates the actor, scope and diagnostic access.

## Request and conflict ordering

1. Validate shape: actor and scope must be nonempty, expected epoch nonnegative,
   and the batch must contain at most **1,024 edits**, each with a unique positive
   occurrence ID. Duplicate IDs are errors even when their requested values agree.
2. Acquire SQLite's **Immediate** writer transaction, then compare the view epoch.
   A valid stale request returns `BatchRevision::EpochConflict { current_epoch }`
   before occurrence existence is inspected. This includes empty batches,
   all-no-op batches and requests naming unknown positive IDs.
3. At a matching epoch, validate that every occurrence exists before the first
   write. An unknown occurrence is an error and leaves the database unchanged.
4. Apply effective edits in input order. Each effective membership transition
   advances the epoch by one and updates the support index in that transaction.
5. Union consumers of effective changes, evaluate their final support/v1 verdicts
   once, and publish changed branches in branch-name order. Every new attempt
   captures the **final** epoch. Commit all admissions, projections, attempts,
   lineage and charges together, or roll them all back on error.

A matching empty/all-no-op batch returns `Applied { epoch, changes: vec![] }`
without events, index writes, attempts or charges. No-op edits do not repair stale
consumers. A stale consumer may be reconciled if it also consumes an effective
edit in the batch. Consumers with unchanged final verdicts retain their heads and
charges even when a sequential correction would temporarily change the verdict.
Each changed verdict costs one unit, retains its rule identity and points to the
previous accepted head as its parent. Existing incurred charges and unresolved
reservations are preserved. Opaque/model heads retain their existing eligibility
rules; this operation does not certify or silently reuse them.

## Retry and concurrency

Two independent connections correcting the same nonempty effective view at the
same expected epoch serialize: one applies, the other observes an epoch conflict.
The epoch counts membership transitions, not batch calls. It advances even when
all final support verdicts remain unchanged.

After a lost response, **reread the current view and relevant heads** before
choosing another expected epoch. Reusing the original epoch is safe against
reapplying a successful effective correction, including after a later writer
reverses its memberships. A conflict is not proof that the original call succeeded;
another writer could have advanced the view. Do not automatically replace the
expected epoch and replay old intent. Empty or all-no-op calls do not advance the
epoch and can both succeed at the same epoch without writes.

The older `set_admitted_and_revise` remains an unconditional single-edit API.
Its immediate retry is a no-op, but a retry after an intervening reversal is a new
effective edit. The new API adds explicit optimistic concurrency without changing
that existing behavior. Neither API migrates the schema.

Run `cargo run --locked --example selective_revision` for a mixed withdrawal and
replacement. `tests/atomic_revision_batch.rs` covers error precedence, rollback
following real writes, independent writer contention, final-view comparison for
all 256 initial/final four-occurrence subsets, and reproducible seeded histories.
