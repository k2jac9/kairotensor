# Evaluated policy activation implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`; execute sequentially, preserving failing evidence.

**Goal:** Register exact known policies, evaluate comparable complete records and
activate or roll back a version without bypassing evidence or accounting gates.
**Architecture:** Immutable registry/evaluation rows and scoped revision CAS in
the same SQLite writer transaction as policy admission changes and support
publications. Python produces offline attestations; Rust validates and decides.
**Tech Stack:** existing Rust/rusqlite/serde/SHA-256 and Python fixtures.
**Spec:** [continuation specification](../specs/2026-11-15-reliability-continuation.md),
section D. Prerequisites: A-C reviewed. Only this stage introduces a migration.

## Global Constraints

- Known policies only: always_stop, affordable_retry, evidence_gated_retry.
- Scope/purpose and authenticated management authority come from trusted runtime.
- No inferred scientific result from failure, no unknown-cost efficiency claim.
- Compare a contemporaneous paired baseline under one frozen comparison manifest.
- Activation atomically changes policy admissions; never changes old send claims,
  outstanding holds, historical dispositions or original deadlines.
- Rollback advances history and needs compatibility, not superiority.
- No Rocket integration, live model call, new service or live-database migration.
- Next schema version is determined from integrated source; do not assume v10.

## Review Focus

1. A B-beats-A evaluation must not activate B when C is the active baseline.
2. Hash, settings, split, case coverage, unit or scorer mismatches refuse.
3. An invalid policy or inconsistent policy admission cannot activate.
4. Concurrent bootstrap/promotion/rollback has one committed winner and no partial
   admission, support, pointer or event state.
5. Changing policy invalidates dependent opaque work but preserves claims/holds.

### Task 1: Strict candidates and complete comparison records

**Files:** create `src/policy_registry.rs`, `src/policy_registry/types.rs`,
`src/policy_registry/evaluation.rs`, `tests/policy_evaluation.rs`,
`scripts/policy_evaluation_v2.py`, `scripts/test_policy_evaluation_v2.py`,
`docs/policy-evaluation-v2.md`; modify `src/lib.rs`,
`scripts/recovery_policy_eval.py` only to expose an explicit v2 conversion mode,
and `scripts/test_recovery_policy_eval.py` for preservation of v1 behavior.

**Interfaces:**

```rust
pub fn validate_policy_artifact(bytes: &[u8]) -> rusqlite::Result<PolicyArtifactV1>;
pub fn evaluate_policy_comparison(value: &PolicyEvaluation)
    -> rusqlite::Result<SelectionDecision>;
```

Artifact exact fields: schema_version=1, policy_name (known enum), purpose,
compatibility_contract_sha256. Unknown fields, empty purpose, duplicate keys,
unknown policies, invalid lowercase digests or artifacts over 64 KiB refuse.
No user-supplied executable configuration.

`PolicyCandidate`: version_id:String, actor:String, scope:String, purpose:String,
artifact:Vec<u8>, sha256:String. Store creates its evidence occurrence.
`PolicyEvaluation`: evaluation_id, actor/scope/purpose, baseline_id/candidate_id,
baseline_sha256/candidate_sha256, experiment_manifest:Vec<u8>,
experiment_sha256, evaluator_version=`policy-selection/1`, evaluator_sha256,
provenance (synthetic_registry_mechanics or observed_policy_replay), baseline_rows,
candidate_rows. Provenance is included in all digests and persisted with the
evaluation. Synthetic evaluations are eligible only for purpose
`fixture/recovery-policy`; nonfixture activation refuses them.
Manifest format is `policy-comparison/1` with all
identities specified in section D. Evidence/labels/request opportunities are
explicit digested records, not URLs loaded during activation.

Rows contain case_id, outcome enum {Accepted, Refused, Failed, Unknown},
correct:Option<bool>, eligible:Option<bool>, answerable:bool,
completed_ms:Option<u64>, charged:Option<u64>, reserved_bound:u64,
false_refusal:bool, unsafe_acceptance:bool, deadline_miss:bool. Validate the
redundant flags against outcome, frozen labels, eligibility and timing. Require
all ordered scheduled cases exactly once per side. Cap 10,000 cases/16 MiB.
Manifest accounting unit is model_tokens or fixture_bytes, identical on both
sides; logical_support and unknown refuse policy comparison.
Observed rows additionally contain ordered attempt references with opportunity ID,
request/evidence-manifest/response digests, execution/disposition, measured input
and output or unknown, retained bound, start/completion offsets, validator/scorer
identities and label digest. Check against frozen opportunities and recompute row
accounting/outcome from those observations. Missing or incompatible references
make observed replay incomplete/incomparable. This is consistency checking of
attestations; it does not prove the host executed them. Synthetic mechanics rows
declare simulated counters and are never reported as feasible named-policy replay.

`SelectionDecision` is Eligible { validated_counters } or Refused(SelectionRefusal),
with typed reasons Incomplete, Incomparable, NoImprovement, IncreasedFalseRefusal,
IncreasedUnsafeAcceptance, IncreasedCost, IncreasedDeadlineMisses. Structural
malformation is an error. Unknown data remains stored unevaluated only where
validly represented; it never yields Eligible.

- [ ] Add failing tests for every identity/case/unit mismatch, malformed known
  artifact, incomplete/unknown result, numeric overflow, fabricated counters and
  deadline boundary (completion equal to deadline is on time).
- [ ] Implement strict parse and recomputation of counters from rows. Require
  success_candidate > success_baseline and all four nonincrease conditions.
- [ ] Implement explicit Python v2 attestations with the same strict validation.
  Legacy v1 rows lacking answerability, timing or unit identity return an explicit
  incomplete conversion unless the caller supplies a matching frozen comparison
  artifact. Do not infer these fields from a successful protocol parse.
- [ ] Preserve v1 evaluation output. Add cross-language golden positive/negative
  v2 fixtures; label the positive case `synthetic_registry_mechanics`.
  Test that synthetic evaluation cannot activate a nonfixture purpose.
  Existing always_stop paired replay may correctly return NoImprovement; never
  select a worse baseline or change the selection rule to make it pass.
- [ ] Run `cargo test --locked --test policy_evaluation`, both Python policy
  suites and locked all-targets; review and commit
  `feat: validate complete comparable policy evaluation records`.

**Done:** selection is a deterministic check of exact supplied attestations,
not an assertion that current empirical policies improve performance.

### Task 2: Registry migration and immutable registration

**Files:** create `src/policy_registry.sql`, `tests/policy_registry.rs`;
modify `src/policy_registry.rs`, `src/policy_registry/types.rs`, `src/lib.rs`,
`tests/dispatch.rs`, `tests/deterministic_verdict.rs`, `tests/timed_recovery.rs`,
and `README.md`. The three legacy test edits are limited to the exact final
schema-version expectation; preserve their fixtures and every data/accounting/FK
assertion. README changes only its current migration-version sentence.

Tables: policy_versions (version_id PK, actor, scope, purpose, evidence_id UNIQUE
FK evidence, contract SHA, artifact SHA); policy_evaluations (evaluation_id PK,
actor/scope/purpose, baseline_id/candidate_id FKs, exact manifest and row bytes,
digests/evaluator identity); policy_heads (actor/scope/purpose composite PK,
version_id FK, revision positive i64, baseline_manifest_sha256);
policy_events (same scoped key plus revision UNIQUE, action, previous/target
version, evaluation ID nullable, manifest SHA, nonempty reason).
Version/evaluation/event rows reject update/delete with triggers. Head revisions
are monotonic and changed only by the activation API. No invented backfill.

```rust
Store::register_policy_candidate(&mut self, candidate: &PolicyCandidate)
    -> rusqlite::Result<()>;
Store::record_policy_evaluation(&mut self, evaluation: &PolicyEvaluation)
    -> rusqlite::Result<()>;
```

Registration checks exact artifact/digest and matching scope/purpose, creates
one fresh unadmitted evidence occurrence and version row atomically. Identical
repeat of an existing version is a no-op; conflicting bytes or metadata error.
Evaluation insertion binds existing exact candidate versions and immutable
manifest/rows. Identical duplicate is a no-op; conflicting duplicate refuses.
Baseline-only evaluation uses baseline_id == candidate_id and identical rows;
it establishes comparison evidence, not superiority. Incomplete structurally
valid evaluations may be stored but are never promotion eligible.

- [ ] Write failing migration, duplicate-registration, wrong-scope, artifact/hash
  and immutable-row tests. Determine the next free schema version.
- [ ] Add migration within Store's existing Immediate initialization transaction.
  Test a populated v9 copy containing accepted/stale work and an unresolved hold:
  IDs, output, usage, view epochs and old receipts survive unchanged after reopen.
- [ ] Inject failure after some DDL but before commit using a test-only migration
  hook; reopen and verify the old schema/rows remain intact. Reject unknown future
  versions. Keep original backup bytes; test recovery from that fixture backup.
- [ ] Update the three existing migration tests to assert the newly allocated
  exact final version, and update the README migration sentence. Do not replace
  version equality with a permissive range or weaken old invariants.
- [ ] Implement registration. Run `cargo test --locked --lib --test policy_registry
  --test recovery --test timed_recovery --test dispatch --test deterministic_verdict`;
  the lib run must execute the test-only interrupted-migration hook. Review and commit
  `feat: persist immutable scoped policy versions and evaluations`.

**Done:** registry data is durable; nothing is active and no sends are authorized.

### Task 3: Atomic bootstrap, promotion and rollback

**Files:** modify `src/policy_registry.rs`, `src/policy_registry/types.rs`,
`src/policy_registry/evaluation.rs`, `src/support.rs`, `src/lib.rs`,
`tests/policy_registry.rs`; create `docs/policy-promotion.md`. The evaluation-module
change is limited to extracting/exposing its existing strict manifest validator
for reuse by bootstrap; do not duplicate the validator or change selection rules.

```rust
Store::activate_policy(&mut self, request: &ActivationRequest)
    -> rusqlite::Result<ActivationResult>;
Store::active_policy(&self, actor: &str, scope: &str, purpose: &str)
    -> rusqlite::Result<Option<ActivePolicy>>;
```

Request contains actor/scope/purpose, expected_active_revision:Option<i64>, action:
Bootstrap { baseline_id, baseline_manifest:Vec<u8>, baseline_manifest_sha256,
reason }, Promote { candidate_id, evaluation_id }, or Rollback { target_version_id,
reason }. Baseline bytes are explicit here so bootstrap verifies their hash and
structure, not only an uncheckable supplied digest. The manifest does not assert
measured superiority. ActivePolicy returns version ID/digest, evidence ID,
compatibility contract, revision and baseline manifest digest, never send permission.

Result Activated { revision } or Refused(ActivationRefusal): RevisionConflict,
AlreadyInitialized, MissingHead, InvalidArtifact, ScopeMismatch, Incompatible,
BaselineMismatch, MissingBaselineEvaluation, EvaluationRefused(SelectionRefusal),
NeverActivated, AdmissionMismatch, NoChange. Invalid request syntax is an error.

Use one Immediate transaction. Check expected revision (None only bootstrap),
scope/purpose, candidate compatibility and current policy admission. Promotion
requires evaluation's baseline ID+digest match current head, its exact manifest
match the bootstrapped comparison manifest, and a baseline-only evaluation with
identical baseline rows/manifest/evaluator. Recompute selection from stored rows.
Rollback verifies a prior activation event and current artifact compatibility;
it does not require an evaluation. Refuse selecting an already active target.

Extract/reuse A's final-state transaction helper rather than calling a public
method that opens a nested transaction. Withdraw old/admit new, revise final
changed support heads, update pointer and append event in the same transaction.
New revision = old+1 (bootstrap1), checked for overflow. No edits to claims,
reservations or deliveries. Head/evaluation lookup is always scope-bound.

- [ ] Add failing empty-registry bootstrap, concurrent bootstrap, B-beats-A with
  C active, stale revision, missing baseline evaluation, incompatible rollback,
  rollback without superiority and inconsistent-admission tests.
- [ ] Implement distinct gates. Test simultaneous promote/promote and
  promote/rollback on two connections with barriers: one winner, one conflict.
- [ ] Inject an admission/support/event failure after writes; compare full state
  after rollback/reopen. Test a captured opaque/timed attempt refuses after the
  policy epoch change, while a previously claimed uncertain hold is unchanged.
- [ ] Mutation checks: omit active-baseline comparison, skip revision CAS,
  omit admission replacement, release an old hold. Each targeted test must fail.
- [ ] Run registry, recovery, timed recovery, atomic batch and eligibility tests;
  focused independent review and commit `feat: atomically activate evaluated policies`.

**Done:** activation has one atomic authority boundary and no independent send power.

### Task 4: Fixture integration and final regression

**Files:** modify `examples/recovery_policy_eval.rs`,
`scripts/recovery_policy_eval.py`, `scripts/test_recovery_policy_eval.py`,
`docs/policy-promotion.md`, `.github/workflows/check.yml`;
create `examples/policy_activation.rs`.

Keep existing example modes and archived v1 output unchanged. Add a fixture-only
registry path that bootstraps always_stop, records complete synthetic baseline
and paired evaluation, activates a qualifying named candidate and rolls back.
Demonstrate that an actual existing v1 comparison can refuse promotion. The new
`policy_activation` example uses a separately versioned typed-role input manifest:
one Policy occurrence followed by Report occurrences. Capture every occurrence,
validate the captured policy artifact against the selected active digest, and
render only Report payloads. Do not append policy bytes to the existing v1
collector, which parses every input as Report. Recheck active revision immediately
before preparation and rely on normal claim/settlement gates. Preserve the
original collector path and archived format byte-for-byte.

- [ ] Add failing integration tests for policy switch before claim, rollback
  revision advance, historical receipts and retained uncertain holds.
- [ ] Implement fixture CLI and explicit offline CI step. Outputs identify
  synthetic attestations and zero model calls; never label a policy empirically
  superior based on the registry example.
- [ ] Run `cargo test --locked --all-targets`, policy fixture example, all Python
  suites and explicit compiled-runner checks from existing CI. Retain artifacts.
- [ ] Independent review of the entire D range and commit
  `test: demonstrate policy activation and rollback without model calls`.
