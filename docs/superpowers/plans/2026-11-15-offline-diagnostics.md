# Bounded offline decision diagnostics implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`. Execute Task1 after C; Task2 has an explicit
> consumer prerequisite and must not be auto-enabled by a synthetic fixture.

**Goal:** Plan bounded ablations and score recorded outcomes without execution;
prepare portable evidence only when a real second consumer requires it.
**Architecture:** Pure Python planning/scoring over authorized bundles, with an
optional future Rust envelope codec that grants no admission.
**Tech Stack:** Python standard library; optional Rust serde and B's SHA-256.
**Spec:** [continuation specification](../specs/2026-11-15-reliability-continuation.md),
section E. No Rocket integration or cross-repository build dependency.

## Global Constraints

- At most four evidence items; checked call bound before a plan is emitted.
- CLI plans or scores recorded outcomes only; no network, tool or model execution.
- Missing/failed trials stay in the denominator; unstable baselines are inconclusive.
- Influence is a diagnostic association, not causal attribution or independence.
- Never prune opaque dependencies or change evidence admission from a score.
- Preserve exact bytes, ordered inputs, IDs and all cost/hold information.

## Review Focus

1. Pair expansion must fit max_calls; 4 items × 3 trials means 33, not 15.
2. Missing/duplicated/mismatched trials must not become a complete report.
3. An unstable baseline or failed trial must not produce a ranked causal claim.
4. Tied associations remain tied and deterministic.
5. Imported receipts never grant current admission or timed send permission.

### Task 1: Bounded schedule and recorded-outcome scorer

**Files:** create `scripts/decision_ablation.py`,
`scripts/test_decision_ablation.py`, `docs/decision-ablation.md`;
modify `.github/workflows/check.yml` for tests only.

**Interfaces:**

```python
def plan_ablations(bundle, trials=3, include_pairs=False, max_calls=33,
                   *, selected_ids=None): ...
def score_ablations(plan, outcomes): ...
```

Accept only a complete bounded C bundle with all selected evidence available.
Initial public applicability is the direct root of the known initial incident/1
contract: no retry, inherited/reused output, policy-bearing or opaque contract.
Validate roles from the exact context/source manifest, runtime/timing binding,
recorded validator provenance and reconstructed request bytes; the existing
Python byte reader alone is insufficient. Preserve full source comparison
identities and refuse unsupported contracts. Ancestor computations are never
scheduled implicitly. These checks establish declared-contract consistency,
not full Rust ledger validation or renewed access/send authority.
`selected_ids=None` selects all distinct selectable Report/Evidence occurrences
only when there are at most four. Otherwise require an explicit selection.
Context/Policy/InheritedOutput and structural lineage do not count toward that
threshold and are never selected. Sort selected IDs by
original input order and reject unknown/duplicate selections; never silently
select a subset when the bundle contains more than four,
rather than silently truncating dependencies. Repeated occurrences at several
positions are one ablated item whose scheduled removal covers every occurrence
position; identical bytes under distinct IDs remain distinct items.
The known incident/1 contract requires unique occurrence IDs, so its public
planner refuses repeated IDs. Test repeated-position grouping in the internal
schedule helper only; do not invent a new public contract or relax incident/1.

Plan schema `decision-ablation-plan/1`: source bundle digest, ordered selected IDs,
trials (positive integer <=100), include_pairs boolean, max_calls integer <=10,000,
scheduled_calls, and rows {trial_id, repeat_index, removed_ids, variant_digest}.
The plan also binds scorer=`recorded-result-equality/1`, root runtime identity and
recorded validator identity. Only declared report/evidence inputs are selectable;
context, policy, inherited output and structural lineage are never removed. A
bundle with unknown input roles or missing comparison identity refuses planning.
`variant_digest` hashes versioned compact sorted-key JSON of source bundle digest,
ordered removed IDs and those three comparison identities, excluding repeat index;
trial_id uniquely combines the variant digest and repeat index.
Schedule baseline, ordered singles, lexicographic pairs, each for all r repetitions.
Omission describes a hypothetical diagnostic request; it does not mutate or
re-authorize the original bundle. The plan emits no executable backend command.

Outcome schema `decision-ablation-outcomes/1`: plan digest and one row per trial
ID with state (recorded, failed, unknown), response digest, validated semantic
result or null, charged units or unknown, reserved bound and unit. Rows also bind
the plan's scorer/runtime/validator identities. The fixed scorer compares
producer-supplied normalized result values; it is not a scientific-truth validator.
Treat normalized results as recorded attestations. Exact duplicate
trial IDs and unknown IDs refuse; missing trials make report Incomplete. Different
scorer/runtime/validator identities make comparison Incomparable.

Before scoring, independently regenerate the complete bounded plan schedule,
including every baseline/single/enabled-pair repeat, variant digest and trial ID.
Require exact rows and scheduled count within max_calls even if a caller rehashed
a mutated plan. Reject removed baseline repetitions/pairs, duplicate repeat
indices, incorrect variant digests or false counts. This verifies plan consistency;
without the source bundle the scorer does not reauthenticate source applicability.

Score only if every baseline trial is recorded and all baseline semantic results
agree. Each variant records changed/unchanged/failed/unknown counts out of all r
scheduled repeats; incomplete variant trials yield no influence rank. A complete
variant's association score is changed/r. Preserve equal scores as ties, sorting
display by original selected order. Pair scores are associations, not additive
causal effects. Report total scheduled/recorded calls, charged and held units;
unknown measurements remain unknown plus bounds, never zero.

- [ ] Write failing tests for n=4,r=3,pairs=True =>33, max_calls=32 refusal,
  invalid bounds/types/overflow, more-than-four without selection, identical bytes
  with distinct IDs, repeated positions and redacted bundle refusal.
- [ ] Implement deterministic planning. Add no backend imports or run mode.
- [ ] Write failing unstable-baseline, missing/unknown/failed trial, extra trial,
  changed plan digest, identity mismatch, tied score and unknown-cost tests.
- [ ] Implement scoring and `plan`/`score` CLI. Monkeypatch process/network hooks
  to raise in tests; both commands must remain pure file input/output.
- [ ] Run `python3 -m unittest discover -s scripts -p 'test_decision_ablation.py'`
  plus existing script suites; demonstrate plan and score using explicit recorded
  fixtures. Independent review and commit `feat: plan and score bounded offline ablations`.

**Done:** offline diagnostics are usable independently of an external consumer.

### Task 2: Conditional portable envelope

**Entry gate:** name an actual second consumer, its owner identity mapping and
scope/purpose needs in a new intake receipt. The current roadmap names none and
explicitly excludes Rocket. Until this condition is met, record Deferred and
continue other work; do not invent a mock consumer as justification.

**Allowed files after the gate:** create `src/evidence_envelope.rs`,
`tests/evidence_envelope.rs`, `examples/export_evidence.rs`,
`docs/evidence-envelope.md`; modify `src/lib.rs`. No other repository edits.

```rust
pub fn parse_envelope(bytes: &[u8]) -> Result<EvidenceEnvelopeV1, EnvelopeError>;
pub fn verify_payload(reference: &EvidenceReferenceV1, payload: &[u8])
    -> Result<(), EnvelopeError>;
```

Reference: producer, kind, id, revision, lowercase SHA-256, media_type.
Envelope: version1, full reference, exact context manifest version/bytes/digest,
typed lineage of full references; payload bytes may be separately carried.
Receipt: full reference, authority, actor, scope, purpose, decision
(eligible/ineligible/unknown), authority_revision, view_epoch, context digest;
optional expiry must specify clock domain and trusted sample semantics. Without
known clock continuity an imported receipt stays unknown. Parse <=16 MiB,
<=1,024 lineage entries, depth<=64; reject unknown/duplicate fields and cycles.
EnvelopeError: InvalidSchema, InvalidReference, InvalidHash, HashMismatch,
LimitExceeded. Parsing/hash verification have no Store mutation capability.

- [ ] Write failing hash/reference mismatch, identical payload/distinct ID,
  wrong scope/purpose, stale receipt, unsupported version and binary-byte tests.
- [ ] Implement strict parsing/export, preserve original manifest bytes and
  require receiver authentication/current owner eligibility before any future
  admission. No import-admit API is added by this task.
- [ ] Run `cargo test --locked --test evidence_envelope`, the export example and
  integration suites. Exchange exact fixture bytes with the named consumer's
  owner; record what was actually verified. Review and focused commit.

**Done:** a deliberately optional contract serves a real documented consumer;
its receipts remain assertions, not cross-store authority.
