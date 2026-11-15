# Decision bundle v1

`kairotensor::decision_bundle` is a pure, bounded export codec and replay API.
It cannot authenticate the producer, grant current access, import into a Store,
reproduce general historical timing/publication, or dispatch a backend. SHA-256
establishes internal consistency only. C2 owns transactional access checks and
payload redaction; C3 owns CLI/process-effect observation.

## Public API and limits

```rust
parse_decision_bundle(bytes: &[u8]) -> Result<DecisionBundleV1, BundleError>
validate_recorded_bundle(bundle: &DecisionBundleV1) -> ReplayReport
reevaluate_bundle(bundle: &DecisionBundleV1, validator: &str) -> ReplayReport
DecisionBundleV1::seal(&mut self) -> Result<(), BundleError>
DecisionBundleV1::to_json(&self) -> Result<Vec<u8>, BundleError>
ByteEnvelope::from_bytes(bytes: &[u8]) -> Result<ByteEnvelope, BundleError>
ByteEnvelope::decode(&self) -> Result<Vec<u8>, BundleError>
```

All types are re-exported from `decision_bundle`; exact field definitions are
in `src/decision_bundle/types.rs`. Every wire struct denies unknown and duplicate
fields. Every `Option` field must be present, using JSON `null` for absence.
Arrays preserve their order. Identity values are positive signed 64-bit integers;
positions, epochs, revisions, charges and holds are nonnegative signed 64-bit
integers. Arithmetic is checked. Floating-point identities are invalid.

Limits: 16 MiB complete JSON, 4 MiB decoded bytes per envelope, 128 attempt
records, JSON nesting depth 64. The incident codec additionally bounds its
context/request/reply to 1 MiB, with at most 1,024 incident input occurrences.
Raw parsing checks size, UTF-8 and depth before deserializing. Directly constructed
public values check envelope lengths and use a bounded counting serializer
before cloning or constructing a JSON value. Envelope bytes are decoded only
after the length and lowercase-hex checks. There is no unbounded JSON `Value`
field in the wire schema.

Errors are payload-free `InvalidSchema`, `InvalidEncoding`, `LimitExceeded`,
`InconsistentLedger`, or `Store(rusqlite::Error)` for C2's distinct database errors.
Pure parsing and replay never open a database. Missing input/dependency records
are representable for diagnostic completeness reporting; duplicate definitions,
invalid identities, contradictory accounting and invalid digests refuse parsing.

## Exact outer encoding and digests

Encode recursively sorted object keys as compact UTF-8 JSON, with comma and
colon separators, no trailing newline, no Unicode normalization, and no ASCII
escaping of non-ASCII characters. Array order is unchanged. Rust's default
`serde_json::Map` ordering implements this encoding. An independent Python
encoder uses `json.dumps(value, sort_keys=True, ensure_ascii=False,
separators=(',', ':')).encode('utf-8')`.

`integrity` is `{ "version": "decision-bundle-integrity/1", "sha256": ... }`.
Its digest hashes the canonical complete object after removing **only** the
`integrity.sha256` key. Keep `integrity.version`, root selection, all scope
metadata, and every ordered record in that hash. `seal` checks a bounded
candidate before changing this digest; it is not authorization.

`source_bundle_sha256` hashes the canonical **complete** representation, including
`integrity.sha256`. Pretty printing or transport key order may therefore change
a file's digest without changing `source_bundle_sha256`. Record transport file
SHA-256 separately when retaining an artifact.

Every original byte field is `{ "hex": "lowercase hex", "byte_len": integer,
"sha256": "64 lowercase hex characters" }`. Embedded bytes are never re-encoded.
In particular, runtime/timing JSON, source bytes, payloads, requests and responses
retain their original bytes, whitespace, key order and non-UTF-8 values. Replaying
a recognized contract may parse copies, but never replaces the stored envelopes.

## Exact record map for C2

Top-level required fields:

| Field | Wire type and ledger meaning |
| --- | --- |
| `schema_version` | Integer `1` |
| `root_attempt` | Selected attempt ID |
| `actor`, `scope`, `export_purpose` | Nonempty strings for the authorized export |
| `snapshot` | `{root_branch, head: nullable ID, revision, current_epoch}` from the retained root snapshot |
| `attempts` | Ordered `AttemptRecord` array; no duplicate IDs |
| `evidence` | Ordered `EvidenceRecord` array; one definition per global occurrence ID |
| `integrity` | Version and outer digest described above |

Every `AttemptRecord` has these fields:

| Field | Wire type / source |
| --- | --- |
| `id`, `branch` | `attempts.id`, `attempts.branch_id` |
| `actor`, `scope` | The attempt branch's own actor/scope; root must equal top-level identity |
| `captured_epoch`, `captured_revision` | `attempts.view_epoch`, `attempts.branch_revision` |
| `parent`, `inherited_from` | Nullable attempt IDs; inherited_from must equal parent if present |
| `candidates` | Ordered `{position, evidence_id}` records from `attempt_candidates.requested_id` |
| `inputs` | Ordered `{position, evidence_id, role}` records from `attempt_inputs` |
| `dispatch_request` | Nullable envelope from **actual** `dispatches.request` |
| `delivery` | Tagged state described below |
| `validator` | Nullable `{version, provenance}` |
| `immutable_runtime` | Nullable exact-byte envelope of retained `runtime_json` |
| `immutable_manifest` | Nullable exact B manifest envelope; if reconstructed, only export after its SHA-256 matches the captured runtime fingerprint |
| `timing_configuration` | Nullable exact-byte envelope of `recovery_timing_bindings.configuration`; no fabricated completion observations |
| `accounting` | Per-attempt object described below |
| `reuse_origin` | Nullable `model_reuses.origin_attempt` |
| `model_contract` | Nullable `model_attempts.contract`, preserved verbatim |
| `deterministic` | Nullable `{version, output: ByteEnvelope}` from computation/output binding |
| `support` | Nullable `{version, rule_id, verdict, clauses}`; see formula below |
| `recovery` | Nullable `{recovery_id, ordinal, state, predecessor_id, request}`; `request` is a nullable envelope of `recovery_attempts.request`, distinct from dispatch |

Candidate positions and input positions are separate ordered sequences. The
legacy capture API compacts admitted inputs after filtering candidates, and
other exact-input attempts may have no candidate rows. Never join them by
position globally. Repeated occurrence IDs at distinct positions are preserved.
Positions must strictly increase within each sequence. Only recognized complete
contracts enforce their additional candidate/input parity requirements.

Each `EvidenceRecord` is `{occurrence_id, source: ByteEnvelope,
payload: ByteEnvelope, authorized_scopes: [{actor, scope}, ...]}`. The scope list
is nonempty and has unique pairs. Every consuming attempt must have its own
actor/scope in the list. Shared occurrence bytes are defined once even when
multiple scopes consume them. These assertions are checked for consistency;
they do not authenticate the supplied identity or prove actual admission.

Roles use `context`, `policy`, `report`, `evidence`, or `unknown`. The recognized
`report-verdict/v1` contract uses `report`; a recognized B manifest uses `context`
then ordered `report` entries; `support/v1` uses `evidence`. Otherwise use
`unknown`. No policy-role derivation exists in C1. Roles never justify omitting
an input or independently authorize access.

`support` retains the immutable formula, not mutable witness counters. `clauses`
is an ordered array of `{clause_id, premises: [{evidence_id, actor, scope}]}`;
clause IDs and premise occurrence IDs within a clause must be unique. The
formula's `rule_id` is a reference, not an output dependency. A requested premise
may be absent; do not invent an evidence record for it. Support computation is
not a C1 replay validator. The recorded verdict remains historical metadata.

Only `inherited_from`, `reuse_origin` and the bridge retry predecessor identify
required prior output records. An unconsumed `parent` is lineage only. Explicit
inherited/reuse cycles or absent outputs yield `Incomplete` and
`MissingDependency`, with bounded memoized graph traversal. Included reuse outputs
must match their dispatched accepted origin, contract, candidates and inputs.

## Delivery, provenance and accounting

Delivery is either:

```json
{"state":"unresolved"}
```

or:

```json
{"state":"recorded","raw":{"hex":"","byte_len":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},"disposition":"accepted","usage":0,"rejection":null}
```

Dispositions: `accepted`, `stale_view`, `stale_branch`, `invalid_output`.
A recorded ledger delivery has nonnegative measured `usage`. The Rust type is
nullable so a caller cannot accidentally default unknown usage to zero; null on
a recorded delivery refuses. An unresolved state has no usage fact. Its charge
is zero **recorded charges**, not a claim of zero model usage.

Only invalid-output deliveries have rejection metadata:
`{validator, reason, applicability}`. Applicability is `current`, `stale_view`
or `stale_branch`. The reason is preserved, never rendered into retry feedback.
Validator provenance values: `declared_contract`, `recorded_rejection`,
`deterministic_binding`, `unknown`. Bind deterministic provenance to its exact
computation version; recorded rejection provenance must match its rejection
record. A bridge declaration does not prove that validator ran historically.

`accounting` fields are `{unit, quote, granted, budget_reference, charge,
hold_bound}`. Quote is null or `{input_tokens, output_cap}`; quote/granted/budget
reference must all be present or all null. `output_cap > 0`; the checked sum is
the reservation bound. A known delivery's charge equals usage and its hold is
zero; reserved usage must lie between input quote and full bound. A granted
unresolved reservation keeps its full hold except a cancelled unsent recovery.
Denied reservations have no hold. Recovery state, request, dispatch, reservation
and delivery facts must be compatible. Unknown recovery phases can be retained
but cannot select a renderer.

Units: `logical_support`, `fixture_bytes`, `model_tokens`, `unknown`. C1 recognizes
logical units only with `support/v1` or `report-verdict/v1`; fixture bytes require
the incident manifest fingerprint and immutable runtime identity
`canned-no-model`, `fixture/1`, `utf8-bytes/1`, `incident-canned/1`, `bytes`,
`canned-no-inference`. There is no allowlisted real-model token runtime in C1;
legacy and other runtime accounting stays `unknown`. Do not infer units from
sizes or arbitrary model contract strings. Budget references are opaque local
IDs, not global totals or authority. Report totals sum only exported attempts,
grouped by declared unit; `unknown` totals remain numerically recorded unknown
units and must not be interpreted as model-token or monetary totals.

## Replay semantics and pure boundary

Reports contain independent `consistency`, `reconstruction`, `validation`, root
`recorded_disposition`, root `validator_provenance`, `missing_requirements`,
`accounting`, `source_bundle_sha256`, `reevaluated_validator` and per-attempt
`attempts` results. Status serialization is `matched`, `mismatch`, `incomplete`,
`not_reproducible`. Aggregation prioritizes mismatch, then incomplete, then not
reproducible. An invalid directly constructed value yields consistency mismatch
without cloning its private metadata into the report.

Missing requirements serialize as `validator_unavailable`, `renderer_unavailable`,
`missing_input`, `missing_delivery`, `missing_dependency`,
`unknown_accounting_unit`, `historical_timing_unavailable`. Historical timing is
always marked unavailable; this API does not reconstruct general past publication
eligibility from a current snapshot.

`report-verdict/v1` invokes the existing crate-private pure report computation
and compares its exact output with the immutable output and recorded delivery.
Malformed report evidence yields mismatch, not an unavailable-validator claim.

The incident allowlist uses the exact manifest described in
`docs/incident-store-bridge.md`: complete canonical manifest bytes, runtime
fingerprint excluding its own fingerprint field, exact runtime and calibration
cap binding, context hash, ordered complete source mappings and raw report hashes.
Ordinal 0 with no predecessor selects `Initial`; ordinal 1 with the initial
invalid-output predecessor selects `ProtocolRetry`. Fixed feedback is always
`incident-routing-protocol/1:invalid_output`. Unknown version/phase returns
renderer unavailable. No legacy or unknown-version fallback exists.
Reconstruction compares an actual dispatch, or an unsent retained recovery
preparation when no dispatch exists. Neither proves it was sent.

Recorded validation compares actual format validity with accepted/stale versus
invalid-output disposition. Missing legacy validator bindings are not reproducible.
A changed-validator evaluation evaluates validity under that named allowlist
entry, carries the same source digest and historical disposition, and does not
mutate the bundle. It is not a revision of the historical receipt.

The pure API receives only bytes, bundle records and a validator name. It has no
Store or backend capability, closure, filesystem, process, network or dynamic
loader argument. The implementation calls pure codecs/hash functions and the
existing pure deterministic derive function. C3 must still observe real process
and network effects; no unused fake adapter test is offered as effect evidence.

## Frozen independent encoding example

The following is **825 UTF-8 bytes**, without a newline. It includes literal
`é` (U+00E9). Generated independently using Python's standard `json` and `hashlib`,
and frozen in `tests/decision_bundle.rs::GOLDEN` for C3's exact oracle.

- Integrity digest, excluding only its own key: `765fbaa97312a42a4e5da7022d6350efd2fb4811767bf763f327f6ddf0832093`.
- Complete canonical/source digest: `569b219c44295bb8e1a42358b1bbbd66c153ff379694dd55ef5decf229dbe471`.

```json
{"actor":"é","attempts":[{"accounting":{"budget_reference":null,"charge":0,"granted":null,"hold_bound":0,"quote":null,"unit":"unknown"},"actor":"é","branch":1,"candidates":[],"captured_epoch":0,"captured_revision":0,"delivery":{"state":"unresolved"},"deterministic":null,"dispatch_request":null,"id":1,"immutable_manifest":null,"immutable_runtime":null,"inherited_from":null,"inputs":[],"model_contract":null,"parent":null,"recovery":null,"reuse_origin":null,"scope":"s","support":null,"timing_configuration":null,"validator":null}],"evidence":[],"export_purpose":"golden","integrity":{"sha256":"765fbaa97312a42a4e5da7022d6350efd2fb4811767bf763f327f6ddf0832093","version":"decision-bundle-integrity/1"},"root_attempt":1,"schema_version":1,"scope":"s","snapshot":{"current_epoch":0,"head":null,"revision":0,"root_branch":1}}
```
