# Store-backed incident fixture

`examples/incident_store_bridge.rs` composes the existing timed Store recovery
APIs with the pure incident protocol. It makes zero model or provider calls.
The canned backend counts UTF-8 bytes as fixture tokens; its timing, CPU identity,
and calibration are synthetic, not measured performance evidence.

Run the example and focused gates with the admitted project Cargo wrapper:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  /home/neo/.local/bin/kairotensor-cargo --budget-gib 2 test --locked \
  --example incident_store_bridge --test timed_recovery --test recovery_runtime
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  /home/neo/.local/bin/kairotensor-cargo --budget-gib 2 run --locked \
  --example incident_store_bridge
```

The example test's independent initial-request oracle invokes `python3` and
reads only frozen cases and prompt bytes. It compares the complete request bytes
with Python's sorted, compact, non-ASCII-escaped v1 JSON encoding. Retry bytes
have a separate exact comparison.

## Contract and capture

The example-local `IncidentContract` has private fields. Its constructor verifies
the embedded freeze against SHA-256
`f71732c193ff889f24709a11855b59820a657c21c2b43ec721cddfbff1ede2c8`, then checks
the embedded cases, prompt and contract against the pinned manifest digests.
It derives the complete required report set from the selected frozen case.
It never loads `labels.json`; the freeze's label digest is integrity metadata,
not label contents. Family, split and eligibility flags are not context fields.

The caller supplies a context occurrence, its exact canonical context bytes,
ordered source mappings, explicit runtime identity and output cap. Each mapping
is keyed by workload version, case ID and external report ID. The constructor
rejects incomplete, duplicate, cross-case, reordered or aliased mappings and
contexts that differ from the pinned case. A context occurrence cannot also be a
report occurrence. Context is always candidate position zero.

`incident-store-manifest/1` is sorted-key compact UTF-8 JSON containing:

- Schema and manifest versions; workload, frozen-workload, prompt and validator identities.
- Context occurrence ID and exact-byte SHA-256.
- Ordered complete source mappings with occurrence IDs and payload SHA-256 values.
- Output cap and the complete runtime identity except `prompt_fingerprint`.
- Initial renderer `incident-request/1`, retry renderer `incident-request-retry/1`,
  and fixed feedback `incident-routing-protocol/1:invalid_output`.

The runtime fingerprint is derived from those exact manifest bytes. Excluding
the fingerprint itself avoids a self-referential hash. Hashes bind identities;
they do not grant admission or prove truth, execution or authority. There are at
most 1,024 input occurrences, including context. The pure protocol bounds
context, rendered request and reply validation to 1 MiB and JSON depth to 64.

For replay, the exact object shape below is normative; angle-bracket strings
describe values, not literal manifest contents. Serialize recursively with
lexicographically sorted object keys, compact `,` and `:` separators, UTF-8,
no ASCII escaping of Unicode, and no trailing newline. Arrays retain input
order. Numeric fields are JSON integers, never floats or strings. Digests are
64 lowercase hexadecimal characters over exact bytes.

```json
{
  "context": {"occurrence_id": 1, "sha256": "<captured context bytes SHA-256>"},
  "freeze_sha256": "<captured context freeze_sha256>",
  "initial_renderer": "incident-request/1",
  "output_cap": 512,
  "prompt_sha256": "<captured context prompt_sha256>",
  "retry_feedback": "incident-routing-protocol/1:invalid_output",
  "retry_renderer": "incident-request-retry/1",
  "runtime": {
    "cpu_fingerprint": "<runtime cpu_fingerprint>",
    "decoding": "<runtime decoding>",
    "dtype": "<runtime dtype>",
    "model": "<runtime model>",
    "model_revision": "<runtime model_revision>",
    "readiness": "warm",
    "runtime_version": "<runtime runtime_version>",
    "threads": 1,
    "tokenizer_revision": "<runtime tokenizer_revision>"
  },
  "schema_version": 1,
  "sources": [
    {
      "case_id": "<captured context case_id>",
      "external_id": "<captured context evidence_ids[position-1]>",
      "occurrence_id": 2,
      "payload_sha256": "<captured report bytes SHA-256>",
      "workload_version": "incident-routing/1"
    }
  ],
  "validator_version": "incident-routing-protocol/1",
  "version": "incident-store-manifest/1",
  "workload_version": "incident-routing/1"
}
```

Use the actual captured occurrence IDs, not the illustrative `1` and `2`.
There is one `sources` object for every input after context, with no omissions.
Its case ID and external IDs derive from the captured context, verified against
the pinned case; its occurrence IDs and hashes derive from ordered captured
inputs. Context SHA-256 hashes the raw stored context bytes, not a re-encoding.
Report SHA-256 hashes the raw report text bytes, not a JSON string or report
wrapper. Freeze and prompt digests come from the verified captured context.
Workload/validator/renderer versions and feedback are the fixed literals shown.
The runtime values come from `RecoveryTiming.runtime`, with only
`prompt_fingerprint` removed, and output cap comes from the bound calibration
cap. Readiness is the actual lowercase `warm` or `cold` value; all other runtime
fields are preserved exactly. This fixture uses cap 512 and threads 1, but these
are bound supplied values, not manifest constants. Reconstructed manifest
SHA-256 must equal the timing runtime's original `prompt_fingerprint`.

Initial attempt ordinal 0 has no retry reason and uses `IncidentPhase::Initial`.
Retry ordinal 1 is available only after invalid output, has a retry reason and
uses `IncidentPhase::ProtocolRetry`. Both phases share the exact same manifest.
Request rendering retains the captured system prompt and ordered reports; the
retry adds `validation_error` with the fixed feedback to the user payload before
its compact canonical encoding. No stored free-text reason enters the request.

`prepare_incident_attempt` checks the complete candidate order and immutable
timing runtime/cap/manifest binding before calling `begin_recovery_attempt`.
Only `RecoveryStart::New` provides a preparation opportunity. The timed Store
path captures every required admitted occurrence atomically. Unknown and
unadmitted occurrences return the same payload-free `UnavailableEvidence`;
other Store corruption and I/O errors remain `BridgeError::Store`.

After capture, preparation checks every captured ID, decodes the captured
context, hashes the captured context and every report, reconstructs the whole
manifest, and compares it with the original immutable timing fingerprint.
Only then does it render captured report text. It does not render caller caches.
An error cancels only the newly allocated unsent attempt. Cancellation refusal
or SQL failure preserves both the original error and typed cancellation failure
in `CancellationFailed`; claimed holds remain conservative.

## Reservation, dispatch and retry

Preparation does not reserve budget or invoke a backend. The example separately
calls `reserve_recovery_attempt`, then `run_timed_recovery` with deterministic
clocks and a counted canned closure. Reply format validation uses only context
decoded from the capture. Usage reports include both request and reply bytes.

An admission correction before reservation prevents a hold. A correction after
reservation and before claim prevents dispatch. A correction through another
Store connection inside the backend records stale publication and charges full
measured usage. Post-claim expiry invokes no backend and preserves the hold;
backend uncertainty or settlement failure likewise preserves it. No repeat run
can claim twice. Duplicate settlement does not charge twice.

Reopening a reserved or sent recovery returns `ExistingRecovery`, never another
prepared request. The restart test retains its original in-process preparation
while reopening the Store; this example does not add a preparation reconstruction
or resend API for crashed callers. Finished, denied and exhausted recoveries
return `TerminalRecovery`.

After invalid output, the existing Store permits at most one retry. The bridge
selects the retry renderer solely from the new attempt's `retry_reason` presence
and discards the reason text. The retry adds only the fixed `validation_error`
field. The original timing deadline and manifest remain unchanged. Different
source occurrences or runtime/cap identities cannot silently replace that
recovery's contract.

Tests deliberately use a protocol-valid but semantically wrong canned answer.
Store acceptance here means current evidence and valid format, not correctness
against semantic labels. This work supplies no efficacy, live inference,
production calibration, remote exactly-once delivery, or all-case export claim.
All-case exports and CI integration belong to the separately scoped next task.
