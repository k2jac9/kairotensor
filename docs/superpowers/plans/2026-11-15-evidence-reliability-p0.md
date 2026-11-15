# KairoTensor Eligibility and Atomic Correction Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` or
> `superpowers:subagent-driven-development` to execute task by task. Every checkbox
> below is future work. The owner requested review and planning first; this
> document does not claim implementation or authorize a live model run.

**Goal:** Explain current eligibility and apply several admission corrections
atomically without weakening read, lineage or accounting guarantees.

**Architecture:** Extend the existing Rust/SQLite Store. One snapshot evaluator
serves explanation, current read and resume; one Immediate writer transaction
applies a bounded correction and publishes only final changed support verdicts.

**Tech Stack:** Rust 2021, existing rusqlite `=0.32.1` with bundled SQLite,
serde/serde_json and existing fixture tests. No new service, model, hash library
or persisted schema is needed for this milestone.

**Spec:** `docs/superpowers/specs/2026-11-15-evidence-reliability-review.md`,
especially sections 2-4, plus the owner's pasted design sections 3.2 and 5.
Baseline: `20fab84a6baa525452191ecf80eeee339f0b5ac7`.

## Global Constraints

- Rust and the existing local SQLite database remain authoritative.
- Preserve occurrence identity, ordered inputs, lineage and historical receipts.
- Preserve every incurred charge and unresolved reservation; no new model calls.
- Missing witnesses are not evidence of a proposition's negation.
- Opaque/model heads keep coarse invalidation. Explicit reuse remains separate.
- Authenticate actor/scope and diagnostic access in the embedding runtime.
- No schema migration, evidence payload disclosure or unrelated dependency upgrade.
- Preserve PR #18 and PR #19 and the published-source experiment archives.
- Reconcile the exact base, allowed files and runtime before dispatching a worker.
- Keep tests and contracts unchanged to clear failures; retain failed receipts.
- Use workstation storage admission and bounded Cargo output. During the recorded
  exception, use `kairotensor-cargo`; afterward use `workstation-cargo` and obtain
  a fresh passing admission. The exception is not a permanent build entitlement.

## Review Focus

1. A revoke/read or head-replacement race must return one coherent snapshot;
   exercise the actual gate-to-payload path, not only independent SQL queries.
2. A two-record witness replacement must not create transient unsupported heads
   or intermediate charges, including consumers shared by several edits.
3. A late failure must roll back memberships, indexes, heads, lineage and charges.
4. Empty/no-op stale requests must conflict; no-op requests must not repair an
   already-stale consumer or leak missing-versus-withheld payload information.
5. Historical Accepted and explicit model reuse must never become an implicit
   current-read authorization. Resumes must use the same evaluator.

## Preflight and interfaces

Recheck remote main and both draft PRs. If source differs, update this plan's
mapping through the technical owner before execution. Preserve other worktrees.
Native worktree tools did not expose the Projects placement required by owner
policy; this review used `.worktrees/reliability-review` with a local Git exclude.

Task 1 qualifies dependency identity for Tasks 2-3. Task 2 produces the shared
eligibility types and evaluator; Task 3 changes writer helpers without changing
that evaluator's snapshot semantics. Neither task consumes the future semantic
bridge, bundle or policy registry.

Run commands from the selected worktree. The `cargo` commands below describe the
test invocation; on this workstation prefix them through the admitted launcher,
for example `kairotensor-cargo --budget-gib 2 test --locked --test eligibility`.
Do not start a build following a refused admission.

### Task 1: Qualify the reproducible baseline

**Files:** create `Cargo.lock`, `docs/runtime-adoption.md`; modify
`.github/workflows/check.yml` only to enforce the accepted lock in existing Cargo
invocations. No production Rust files or manifest version upgrades.

**Inputs:** current source, existing manifest constraints, exact Rust executable
and the retained verification lock under the review artifact directory.
**Produces:** a reviewed lockfile, locked CI commands and a source/runtime/lock
verification receipt. This is a reproducibility change, not proof of a complete
runtime compatibility matrix.

- [ ] Confirm `git ls-files Cargo.lock` and preserve the baseline's missing-lock
  failure. On the unchanged baseline, `cargo test --locked --all-targets` must
  refuse to create a lock. If a newer source already has a lock, inspect its
  identity instead of reproducing an obsolete missing-file failure.
- [ ] Resolve under existing manifest constraints, inspect the full dependency
  diff and licenses, record the chosen lock SHA-256 and exact Rust identity.
  Verify upstream freshness before any upgrade decision. Preserve historical
  pins; a proposed version migration is a separate reviewed change.
- [ ] Run `cargo test --locked --all-targets` and the existing Python reference
  and script suites against that resolution. Expected: all tests pass; each
  environment-dependent skip is identified. Preserve the exact lock and logs.
- [ ] Add `--locked` to existing CI Cargo build/test/run commands. Record baseline,
  latest-stable, upcoming and previous-stable lane status with exact known
  identities and explicit unexecuted/unavailable gaps in `docs/runtime-adoption.md`.
  Do not claim missing lanes passed or choose a floating preview as a pin.
- [ ] Review and commit `build: freeze the tested dependency resolution`.

**Done:** the feature implementation has a reproducible dependency baseline;
unresolved runtime-policy adoption remains visible and separately owned.

### Task 2: Explain the authoritative current-read gate

**Files:** create `src/eligibility.rs`, `tests/eligibility.rs`,
`docs/current-eligibility.md`; modify `src/attempts.rs`, `src/support.rs`,
`src/lib.rs`, `README.md`. A test-only race seam may live in these modules;
it must not add a public hook or a persisted format.

**Public interface:**

```rust
Store::explain_current(&mut self, actor: &str, scope: &str, branch: &str)
    -> rusqlite::Result<CurrentExplanation>

pub enum EligibilityState { Empty, Ready, NeedsReview }
pub enum EligibilityProof { CoarseEpoch, SupportV1 }
pub enum EligibilityReason { NoHead, CoarseViewChanged, SupportVerdictChanged }
pub struct CurrentExplanation {
    pub state: EligibilityState,
    pub head_id: Option<i64>,
    pub branch_revision: Option<i64>,
    pub captured_epoch: Option<i64>,
    pub current_epoch: i64,
    pub proof_kind: Option<EligibilityProof>,
    pub reason: Option<EligibilityReason>,
}
```

**Internal interface:** a payload-free evaluator over `&rusqlite::Connection`,
the optional current `Branch` and captured current epoch; return the same
`CurrentExplanation`. Its caller retains the transaction. Extend support
inspection to distinguish `not a support head`, `same verdict` and `changed
verdict`; retain an existing boolean adapter where needed by unchanged callers.

| Snapshot | Explanation |
| --- | --- |
| No branch / no head | Empty, NoHead, no proof or captured epoch; branch_revision is None only when no branch exists |
| Captured epoch equals current epoch | Ready, CoarseEpoch, no reason |
| Epoch differs, current support verdict equals published verdict | Ready, SupportV1, no reason |
| Epoch differs, support verdict differs | NeedsReview, SupportV1, SupportVerdictChanged |
| Epoch differs, opaque/model head | NeedsReview, CoarseEpoch, CoarseViewChanged |

Malformed stored state remains an error, not Ready. Ready means eligible under
the Store contract, not true. No payload, request bytes, occurrence payloads or
free-form validator reasons appear in the explanation or its debug output.

- [ ] Add `explanation_matches_current_gate`,
  `historical_acceptance_does_not_grant_current_access`,
  `model_reuse_requires_explicit_resolution`, and
  `explanation_contains_no_payload_or_validation_text`. Exercise every table row,
  support in both result directions, empty branches and other actor/scope pairs.
  Compare separate public calls only while no writer can intervene.
- [ ] Run `cargo test --locked --test eligibility`. Expected initially: failure
  because the new interface is missing; preserve the actual error.
- [ ] Extract the evaluator and use it in `read_current`, `explain_current` and
  the existing `begin_resume` path. Keep eligibility and accepted-output loading
  in the same transaction; do not add a public authorization cache. Explanation
  checks accepted-delivery existence without fetching output bytes.
- [ ] Add controlled WAL/barrier tests
  `revocation_snapshot_never_exposes_payload` and
  `head_replacement_keeps_metadata_and_output_in_one_snapshot` in the actual
  read path. Pause after gate evaluation, commit through another connection,
  then resume output loading. The pending read may return its older coherent
  snapshot; the next read must observe withdrawal/replacement. No sleep-based
  race timing. At the payload boundary, after the concurrent writer commits,
  the test-only seam must assert `!connection.is_autocommit()` and reread the
  scope epoch plus branch head/revision through that same connection. They must
  equal the gate's captured values. This directly proves snapshot retention;
  immutable historical output alone cannot detect a transaction split. Verify
  that a split-transaction mutation fails this snapshot assertion, and that a
  head-reselection mutation fails the metadata/output consistency assertion.
  Keep mutations in isolated verification copies; restore source afterward.
- [ ] Verify unchanged historical disposition, admission events, branch revisions
  and charged/reserved usage after explanations. Verify `begin_resume` refuses
  the same stale heads and preserves its existing error behavior.
- [ ] Run `cargo test --locked --lib --test eligibility --test dispatch --test
  model_reuse --test selective_revision --test deterministic_verdict`.
  Expected: all named tests and the internal race tests pass.
- [ ] Document reason codes and snapshot boundaries, perform focused review,
  then commit `feat: explain current eligibility through the shared read gate`.

**Done:** inspect why a head is unavailable without serving its bytes, changing
admission, granting a new send or rewriting its historical acceptance.

### Task 3: Correct memberships against an expected epoch

**Files:** modify `src/lib.rs`, `src/support.rs`, `examples/selective_revision.rs`;
create `tests/atomic_revision_batch.rs`, `docs/atomic-correction.md`.

**Public interface:**

```rust
pub struct AdmissionEdit { pub evidence: i64, pub admitted: bool }
pub enum BatchRevision {
    Applied { epoch: i64, changes: Vec<SupportRevision> },
    EpochConflict { current_epoch: i64 },
}
Store::revise_admissions_if_current(
    &mut self, actor: &str, scope: &str, expected_epoch: i64,
    edits: &[AdmissionEdit],
) -> rusqlite::Result<BatchRevision>
```

**Request contract:** nonempty actor/scope; expected_epoch >= 0; at most 1,024
edits; unique positive occurrence IDs. Validate this shape first. Acquire the
Immediate writer transaction and compare epoch before inspecting occurrence
existence. A structurally valid stale request returns EpochConflict, including
empty/no-op or unknown-occurrence requests. At a matching epoch, validate every
occurrence exists before the first mutation. This ordering is an explicit new
API decision; test it rather than leaving error precedence accidental.

**Writer contract:** use transaction-scoped existing admission/index helpers;
apply effective edits in input order; union only consumers of effective changes;
evaluate final verdicts once and publish changed branches in branch-name order.
Every new attempt captures the final epoch. Reuse existing `SupportRevision`.
An unchanged verdict retains its attempt and charge. Empty/all-no-op requests at
the expected epoch return Applied without events, index writes or charges.

- [ ] Add `replacement_has_no_intermediate_verdict`,
  `second_edit_failure_rolls_back_every_write`,
  `concurrent_correction_has_one_winner`, `duplicate_edit_is_rejected`,
  `stale_empty_and_noop_batches_conflict`, `noop_leaves_stale_consumers_alone`,
  `overlapping_consumers_publish_once_at_final_epoch`, and
  `request_shape_and_epoch_precedence_are_stable`.
- [ ] Run `cargo test --locked --test atomic_revision_batch`.
  Expected initially: missing interface failure, retained verbatim.
- [ ] Extract transaction-scoped helpers from the existing single-edit path.
  Implement one transaction, not a loop of public mutating methods. Do not
  change the old method's no-op or duplicate-retry semantics.
- [ ] Extend the rollback test with a trigger that aborts the second effective
  admission, and a separate later-publication failure. A pre-validation failure
  alone does not prove rollback after writes. Compare complete persistent state
  before/after and after reopening, including charges and unresolved holds.
- [ ] Exercise two independent connections with explicit barriers and one
  expected epoch: exactly one Applied and one EpochConflict, no duplicate charge.
  A retry with the old epoch adds no events even after an intervening reversal.
- [ ] Exhaust all 16 initial and 16 final admission subsets of a four-occurrence
  rule fixture. Compare final verdicts to a fresh evaluator/Store; assert
  lineage and charges separately because a fresh store has different history.
  Add reproducible seeded edit sequences and report their seed on failure.
- [ ] Mutation checks: bypass epoch comparison; publish after each edit;
  remove consumer deduplication. Each corresponding invariant test must fail.
  Mutations stay in isolated verification copies and never alter the tests or
  acceptance contract; retain failure receipts and restore the candidate.
- [ ] Run `cargo test --locked --test atomic_revision_batch --test
  atomic_revision --test bulk_setup --test selective_revision --test eligibility`.
  Expected: all pass, including unchanged-head and one-charge assertions.
- [ ] Document the 1,024-edit bound, conflict/error ordering, lost-response
  reread requirement and final-view semantics; update the example; focused
  review and commit `feat: atomically revise admissions at an expected epoch`.

**Done:** one mixed withdraw/admit correction is atomic, reference-equivalent
and independently useful with the explanation API. No migration is needed.

## Integration and independent review

- [ ] Run `cargo test --locked --all-targets`,
  `python3 -m unittest discover -s reference -p test_memory.py`,
  `python3 reference/belief_oracle.py`, and
  `python3 -m unittest discover -s scripts -p 'test_*.py'`.
  Report environment-dependent skips separately; execute binary-dependent checks
  with the built examples when required by the existing CI workflow.
- [ ] Obtain an independent review of the full source range and the five Review
  Focus conditions. Fix important findings with a reproduced failing test and
  a passing targeted/integration run. Preserve every finding and disposition.
- [ ] Record exact source, Rust identity, lock digest, commands, test counts,
  mutation failures and resource accounting. Do not modify historical benchmark
  receipts or claim new model/benchmark performance.
- [ ] Prepare the branch for owner review. Merge, publication, hosted spending
  and the separately authorized Adaption smoke are not part of this milestone.

Next: write the Store-backed incident bridge plan using section 3 of the review.
Bundle replay, policy activation and portability remain later, separate plans.
