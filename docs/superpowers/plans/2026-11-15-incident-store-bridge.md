# Store-backed incident fixture implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`. Execute tasks sequentially; checkboxes are future
> work. The owner authorized autonomous implementation and documented rulings.

**Goal:** Demonstrate real Store eligibility at incident capture, claim and
publication using deterministic adapters and the frozen v1 renderer.
**Architecture:** A pure protocol module and an example-local adapter reuse the
existing timed recovery state machine. Capture one context occurrence followed
by every required report. No separate admission database.
**Tech Stack:** existing Rust/SQLite/serde, qualified `sha2 = "=0.11.1"`, Python
fixture parity. No provider/model/service dependency.
**Spec:** [continuation specification](../specs/2026-11-15-reliability-continuation.md),
section B. Prerequisite: P0 A complete and reviewed.

## Global Constraints

- Preserve occurrence identity, candidate/input order, lineage and all holds.
- Exact manifest bytes bind the context, mappings and runtime; hashes are not
  proof of execution, truth or authority.
- All required IDs come from the pinned workload definition. Caller flags and
  caller-supplied payload replacements cannot authorize preparation.
- Fixture byte accounting is labelled explicitly; no live model calls.
- Do not change `scripts/incident_routing.py` or the
  frozen `workloads/incident-routing-v1/` files.
- Use admitted Cargo output and retain failed checks; honor the autonomous map.

## Review Focus

1. Missing mappings, duplicated IDs or aliases must fail before reservation.
2. A fixture's current flag must not admit an unadmitted Store occurrence.
3. Correction before claim must refuse; correction after claim must refuse
   stale publication and preserve incurred usage/holds.
4. Protocol-valid wrong answers remain protocol-valid; labels never enter retry.
5. Restart, expiry or duplicate delivery must not create a second invocation.

### Task 1: Pure protocol and exact-byte manifest primitives

**Files:** create `src/incident_protocol.rs`, `src/evidence_digest.rs`,
`tests/incident_protocol.rs`; modify `src/lib.rs`, `Cargo.toml`, `Cargo.lock`.
No Store/schema/state-machine changes in this task.

**Interfaces:**

```rust
pub fn sha256_hex(bytes: &[u8]) -> String; // crate-private evidence_digest
pub fn render_incident_request(context: &IncidentContextV1,
    reports: &[IncidentReport], phase: IncidentPhase)
    -> Result<Vec<u8>, IncidentProtocolError>;
pub fn validate_incident_output(raw: &[u8], context: &IncidentContextV1)
    -> Result<IncidentOutput, IncidentProtocolError>;
```

`IncidentContextV1` has schema_version=1, version=`incident-store-context/1`,
workload_version, freeze_sha256, prompt_sha256, validator_version, case_id,
question, owner_ids, evidence_ids, system_prompt. Strings/arrays are nonempty;
owner/evidence IDs are unique. `IncidentReport` has external_id and text;
its order and IDs must exactly match evidence_ids. `IncidentOutput` has decision,
owner (optional), reason and evidence_ids, matching frozen v1 rules. Error enum:
TooLarge, InvalidEncoding, InvalidJson, UnknownVersion, InvalidContext,
InvalidOutput. Error text never embeds rejected bytes.
`IncidentPhase` is Initial or ProtocolRetry. Initial renderer `incident-request/1`
matches Python bytes. ProtocolRetry renderer `incident-request-retry/1` adds
`validation_error: "incident-routing-protocol/1:invalid_output"` to the inner user
payload and nothing else. It never accepts raw prior response or validator text.

- [ ] Write tests for byte-exact rendering, Unicode/escaping, duplicate keys,
  unknown fields, empty/duplicate IDs, depth 64/65, 1 MiB limit, and well-formed
  semantically wrong output. Assert wrong answers can pass protocol validation.
- [ ] Run `cargo test --locked --test incident_protocol`; retain missing-API
  failure. Inspect SHA-256 dependency checksum/license and compatibility; update
  only its required lock resolution and record the dependency diff.
- [ ] Implement the pure types/codec with strict typed deserialization and the
  existing frozen Python JSON rules; SHA-256 delegates to RustCrypto.
- [ ] Run the target suite and locked all-target regression. Commit
  `feat: add pure versioned incident protocol and exact-byte digests`.

**Done:** a pure library can render and validate v1 without a Store, label or
backend. No application can obtain send permission from these functions.

### Task 2: Complete timed capture and deterministic runtime integration

**Files:** create `examples/incident_store_bridge.rs`,
`examples/incident_store_bridge/adapter.rs`,
`examples/incident_store_bridge/tests.rs`, `docs/incident-store-bridge.md`.
Read existing recovery APIs; no new core recovery APIs or migration.

**Interfaces (example-local):**

```rust
fn prepare_incident_attempt(store: &mut Store, recovery: i64,
    candidates: &[i64], contract: &IncidentContract)
    -> Result<IncidentPreparation, BridgeError>;
```

`IncidentContract` fields are private. Its constructor loads a case from embedded
frozen cases/prompt/contract bytes whose digests match the pinned manifest, then
accepts context occurrence, ordered source-to-occurrence mappings, runtime and
cap. It never reads labels. Reject a missing/unknown case, incomplete mapping,
duplicate/aliased occurrence or mismatched context. Runtime prompt_fingerprint
is derived from the exact manifest; other identities are supplied explicitly.

`IncidentPreparation` contains attempt:i64, request:Vec<u8>, captured_ids:Vec<i64>,
manifest_sha256:String and preparation:RecoveryPreparation. `BridgeError`:
UnavailableEvidence, ChangedContract, ExistingRecovery, TerminalRecovery,
InvalidManifest, Protocol(IncidentProtocolError), Store(rusqlite::Error).
Missing/unadmitted evidence share UnavailableEvidence with no payload details.

Candidate order is context then reports; require equality with the full pinned
mapping before calling `begin_recovery_attempt`. Read `recovery_timing` and
verify exact manifest/runtime/cap identity. For New, validate every captured ID
and context byte, recompute every captured report digest and the complete manifest,
and compare it with the immutable timing binding. Render only captured text and
return preparation; reservation
and invocation remain distinct existing API calls. Existing/Finished refuse.
On an error after New, cancel that newly allocated proven-unsent attempt; never
cancel an Existing attempt or modify a claimed hold. Preserve cancellation errors.

- [ ] Add failing `fixture_flags_cannot_admit_store_evidence`,
  `missing_required_input_prevents_claim`, `duplicate_and_cross_case_mapping_refuse`,
  `correction_after_prepare_refuses_send`, and
  `labels_never_enter_request_or_feedback`. Verify no reservation/dispatch for
  rejected mappings; corrupt captured context and a report payload independently,
  not merely caller flags. Full manifest mismatch must prevent reservation.
- [ ] Run `cargo test --locked --example incident_store_bridge`; preserve the
  initial missing-interface failure. Implement the preparation boundary above.
- [ ] Use `reserve_recovery_attempt` and `run_timed_recovery` with a counted canned
  closure and deterministic clocks. Cap/token counts are UTF-8 bytes in this
  fixture. Measured reply validation uses only the captured protocol context.
- [ ] Cover invalid output, reopen while reserved/sent, duplicate settlement,
  post-claim expiry, backend error and settlement error. A post-claim refusal
  invokes no adapter but retains the existing conservative hold. Repeat runs
  cannot claim twice. Correction inside the backend yields stale publication
  and full recorded usage; before claim it yields zero invocation.
- [ ] A retry after invalid output selects ProtocolRetry from the new recovery
  attempt's retry_reason presence, discarding its free text. Both renderer IDs and
  the fixed feedback string are already in the manifest. Preserve original
  deadline and at-most-one existing recovery retry; mismatched/new source mappings
  cannot silently replace the recovery's bound contract. Test byte-exact retry
  requests separately from initial Python parity.
- [ ] Run `cargo test --locked --example incident_store_bridge --test
  timed_recovery --test recovery_runtime` and `cargo run --locked --example
  incident_store_bridge`. Focused review and commit
  `feat: exercise incident preparation through Store timed gates`.

**Done:** the example proves admission and accounting behavior with zero model
calls. A protocol-valid semantic error does not become a correctness claim.

### Task 3: Cross-language parity and fixture-only CI

**Files:** modify `.github/workflows/check.yml`,
`docs/incident-routing-workload.md`, `docs/incident-store-bridge.md`,
`examples/incident_store_bridge.rs`, `examples/incident_store_bridge/tests.rs`;
create `scripts/check_incident_store_bridge.py`,
`scripts/test_incident_store_bridge.py`.

Expose example `--requests PATH`: write case ID/request/manifest digest records
for all 12 frozen cases, no model calls. Artifact metadata labels Store capture
and fixture mode. A Python checker loads the existing frozen workload and compares
every request byte and order with `incident_routing.prepare(..., split='all')`;
case/family/split IDs remain outside model request bytes.

- [ ] First make parity tests fail on one changed byte, omission and reordered
  report. Implement the checker and explicit output argument.
- [ ] Run the example into the artifact directory, run the checker on it and
  `python3 -m unittest discover -s scripts -p 'test_incident_store_bridge.py'`.
- [ ] Add only these offline fixture steps to CI. Run all locked Rust targets
  plus existing Python suites. Record exact source/runtime/lock and counts.
- [ ] Independent review and commit `test: verify Store incident byte parity in CI`.

**Done:** B's exact interfaces and artifacts are available to C. No frozen
workload revision or external provider qualification is claimed.
