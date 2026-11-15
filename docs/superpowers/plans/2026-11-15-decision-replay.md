# Authorized decision export and pure replay implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`; execute tasks sequentially with tests and review.

**Goal:** Export an authorized consistent decision snapshot and reconstruct or
validate what its recorded contracts actually support, without external effects.
**Architecture:** A Store read transaction builds an export; pure bounded parsing
and compiled validators consume it. No new transaction authority or migration.
**Tech Stack:** existing Rust/SQLite/serde and B's SHA-256/protocol primitives.
**Spec:** [continuation specification](../specs/2026-11-15-reliability-continuation.md),
section C. Prerequisites: A and B reviewed.

## Global Constraints

- Default current access and diagnostic historical access are distinct.
- Diagnostic approval does not override withdrawn evidence or restore bytes.
- A historical Accepted receipt is preserved, never used as an access grant.
- Authorize the exact dependency selection within the retained snapshot.
- Preserve arbitrary bytes, repeated input positions, lineage and uncertain holds.
- Do not export cross-scope/global budget totals or infer missing validators.
- Pure replay cannot invoke tools, a model, network, Store initialization or
  migrations. No Rocket integration; use normal storage admission.
- Resource bounds apply before cloning public constructed values and before
  materializing oversized SQLite payload/metadata fields, not only JSON parsing.

## Review Focus

1. Revocation/settlement/head changes must not splice different export snapshots.
2. Denied/redacted output must contain no payload-derived values or existence leak.
3. Unknown validator or missing lineage must not become reproducible success.
4. Changed bytes/accounting and duplicate occurrence loss must be detected.
5. A changed validator cannot rewrite historical disposition or release a hold.

### Task 1: Bounded bundle schema and pure integrity validation

**Files:** create `src/decision_bundle.rs`, `src/decision_bundle/types.rs`,
`src/decision_bundle/replay.rs`, `tests/decision_bundle.rs`,
`docs/decision-bundle-v1.md`; modify `src/lib.rs`, `src/report_verdict.rs`
(expose the existing pure derive function crate-internally only).

**Public interfaces:**

```rust
pub fn parse_decision_bundle(bytes: &[u8]) -> Result<DecisionBundleV1, BundleError>;
pub fn validate_recorded_bundle(bundle: &DecisionBundleV1) -> ReplayReport;
pub fn reevaluate_bundle(bundle: &DecisionBundleV1, validator: &str) -> ReplayReport;
```

`BundleError`: InvalidSchema, InvalidEncoding, LimitExceeded, InconsistentLedger,
Store(rusqlite::Error). Strict versioned structs deny unknown/duplicate fields.
Required top-level fields: schema_version=1, root_attempt, actor, scope,
export_purpose, snapshot (root branch/head/revision/current epoch), attempts,
evidence, and integrity manifest. Each byte field is `{hex, byte_len, sha256}`.
Attempt records contain ID, branch, captured epoch/revision, parent/inherited_from,
ordered candidates and inputs, optional dispatch request, explicit delivery state,
optional validator binding/provenance, optional immutable runtime/manifest,
per-attempt accounting, reuse origin, and support/deterministic contract if bound.
Each input entry preserves position/evidence_id and role (Context, Policy, Report,
Evidence or Unknown). Derive roles only from a recognized immutable contract;
unknown legacy roles remain Unknown. A role is descriptive metadata, never an
independent authorization or justification for omitting an input.
Recovery attempts also export recovery ID, ordinal, state and predecessor ID;
the recognized bridge contract maps ordinal0/1 to Initial/ProtocolRetry and binds
the fixed feedback string. Unrecognized phase/version is RendererUnavailable.
Evidence records contain occurrence ID, source bytes, exact payload bytes and
authorized scope reference. Numeric identities are positive checked i64s; repeated
positions are preserved but duplicate keys/record definitions refuse.

Delivery state is Recorded { raw, disposition, usage, optional rejection metadata }
or Unresolved. Accounting contains unit, optional quote/granted/budget reference,
charge and hold bound; unknown measured usage stays null. Never invent times.
The integrity manifest binds the versioned ordered record serialization and
root selection; include a digest excluding its own digest field. Preserve all
original raw byte envelopes; reserialization of embedded bytes is forbidden.
Define that outer serialization as versioned compact UTF-8 JSON with recursively
sorted object keys, unchanged array order, no Unicode normalization and no ASCII
escaping of non-ASCII characters. `source_bundle_sha256` hashes this canonical
complete representation, including the integrity field, rather than claiming to
hash an arbitrary original transport file. File-byte digests remain separate
artifact receipts. Document this encoding and share exact golden bytes with C3.

`ReplayReport` has independent consistency, reconstruction and validation
results; recorded_disposition, validator_provenance, missing_requirements,
charged/held units and source_bundle_sha256. Result statuses are Matched,
Mismatch, Incomplete or NotReproducible; missing requirements include
ValidatorUnavailable, RendererUnavailable, MissingInput, MissingDelivery,
MissingDependency, UnknownAccountingUnit and HistoricalTimingUnavailable.
Reevaluation additionally names the new validator and never changes the bundle.

- [ ] Write failing tests for changed bytes/digests, non-UTF8 round-trip, identical
  bytes/distinct IDs, repeated input positions, unknown schema, oversized/deep
  JSON, inconsistent quote/usage/holds and unresolved delivery with retained hold.
- [ ] Implement strict parsing and consistency checks. A hash match establishes
  internal consistency only, not authenticated execution.
- [ ] Add pure allowlist entries for report-verdict/v1 and B's incident protocol.
  Replay compares deterministic output or exact reconstructed request as applicable.
  Missing legacy validator is NotReproducible. Unknown version never falls back.
- [ ] Test changed validator as a separate report, missing inherited/reuse input,
  and original disposition preservation. The replay API takes no backend capability;
  inspect that boundary and observe actual subprocess effects in Task3. Do not add
  a disconnected panic-only adapter whose non-use proves nothing.
- [ ] Run `cargo test --locked --test decision_bundle` and locked all-targets;
  review and commit `feat: define bounded pure decision bundle replay`.

**Done:** parsing/replay is deterministic, effect-free and explicit about limits.

### Task 2: Snapshot-bound Store export and diagnostic authority

**Files:** create `src/decision_bundle/export.rs`; modify
`src/decision_bundle.rs`, `src/decision_bundle/types.rs`, `tests/decision_bundle.rs`,
`docs/decision-bundle-v1.md`. Use A's crate-internal evaluator; if its visibility
needs a change, `src/eligibility.rs` is allowed for that visibility only.

**Interfaces:**

```rust
pub struct BundleRequest {
    pub actor: String, pub scope: String, pub attempt: i64,
    pub purpose: BundlePurpose, // CurrentRead or DiagnosticReplay
}
pub trait DiagnosticAuthorizer {
    fn authorize(&self, request: &DiagnosticReadRequest) -> Result<bool, BundleError>;
}
pub enum BundleExport { Complete(DecisionBundleV1), Redacted(RedactedBundleV1) }
Store::export_decision_bundle(&mut self, request: &BundleRequest,
    authorizer: &impl DiagnosticAuthorizer) -> Result<BundleExport, BundleError>;
```

`DiagnosticReadRequest` includes authenticated actor/scope, root ID/purpose and
the full ordered dependency selection: every output attempt and input occurrence,
their own actor/scope, branch revisions, head IDs and current/captured epochs.
No boolean authorization receipt is persisted or reusable. `DenyDiagnostics`
implements the default false result. Redacted has only the caller-supplied
requested_attempt and generic Unavailable reason.

Keep one read transaction through metadata enumeration, authority, admission
checks and payload fetch. For CurrentRead apply A's gate to root and every required
output dependency; require every exported input admitted. For DiagnosticReplay,
authorize the exact selection, still require current input admission, then permit
historical output bytes. Omit global balances and unrelated budget members.
Copy bytes only after access checks; commit the read transaction before return.
Metadata/source/validator free text are sensitive too. Physical unavailability
or denied access never causes an archive lookup. SQL/I/O errors are not success.
Use held-snapshot length/type checks before copying payloads or metadata beyond
the bundle limits. Unavailable/denied/oversized undisclosed selections redact
generically. CurrentRead authenticates only the request actor/scope: reject
foreign-scope output dependency closures, rather than treating row identities as
caller authority. DiagnosticReplay may authorize the exact cross-scope selection,
but still requires every input's current admission under its owning scope.

- [ ] Add failing tests for default denial, authorized historical output with
  admitted inputs, revocation despite diagnostic approval, missing/withheld target
  equivalence, source/request/reason canaries and shared budgets across scopes.
- [ ] Implement bounded dependency closure without treating all parents as data
  dependencies. Export support formulas and recorded bindings, not a fabricated
  historical witness counter. Missing required dependency redacts the result.
- [ ] Add controlled concurrent withdrawal, settlement and head-change tests.
  Assert transaction liveness and unchanged selected rows at payload fetch;
  output-only comparisons do not prove a retained snapshot. Callback receives
  exact dependency IDs and cannot accidentally authorize a broader closure.
- [ ] Verify export leaves admissions, heads, usage and holds unchanged, including
  after reopen. Run the target suite and relevant P0/recovery tests; review and
  commit `feat: authorize decision exports within one Store snapshot`.

**Done:** every complete bundle is an authorized export of one coherent snapshot;
redaction is explicit and payload-free.

### Task 3: Separate-process replay and versioned Python reader

**Files:** create `examples/decision_replay.rs`, `docs/decision-replay.md`,
`scripts/decision_bundle_v1.py`, `scripts/test_decision_bundle_v1.py`;
modify `scripts/audit_recovery_bundle.py` only for an explicit `--decision-v1 FILE`
branch, preserving the legacy directory mode unchanged; modify
`.github/workflows/check.yml` for offline replay steps.

Example commands: `decision_replay export-fixture PATH` generates an authorized
fixture using Store; `decision_replay replay PATH` only parses and calls pure
replay, never Store::open. Python reader checks bounded hex bytes/digests and
prints integrity/availability facts; it must not claim to implement an unknown
Rust validator. Unknown modes refuse, no auto-detection that changes legacy mode.

- [ ] Add failing subprocess tests that compare export/replay results, binary
  preservation, redacted refusal, unknown validator and unchanged file hashes.
- [ ] Implement CLI and versioned reader; do not add a live/generate mode.
- [ ] Retain a separate-process replay process/network syscall trace with the
  installed strace where available; distinguish observed paths from a universal
  proof. No fake adapter or test-only dispatch capability is needed.
- [ ] Run `cargo run --locked --example decision_replay -- export-fixture PATH`,
  then `cargo run --locked --example decision_replay -- replay PATH` in a second
  process; run the new Python suite plus `test_audit_recovery_bundle.py`.
- [ ] Run integration suites, independent review and commit
  `test: exercise decision replay across process boundaries`.

**Done:** the second process reconstructs/validates supported recorded contracts,
or precisely reports why it cannot. Historical acceptance/timing stays reported,
not magically re-proven. No generic causal attribution is emitted.
