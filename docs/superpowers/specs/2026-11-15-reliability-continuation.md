# KairoTensor continuation: executable design decisions

Date: 15 November 2026. Source inspected: main
`20fab84a6baa525452191ecf80eeee339f0b5ac7`; preceding P0 plan commit `8d4cce8`.
All new names below are proposed interfaces until their implementation commits.
The owner authorized autonomous execution.

## Evidence and known limits

The Store already owns admission history, ordered candidate/input occurrences,
branch publication, timed reservations/claims and settlement. Keep that authority.
The existing incident renderer and scorer deliberately trust declared fixture
eligibility. The new bridge must obtain bytes from complete Store capture.

Independent source review established these constraints:

- Accepted/stale legacy deliveries have NULL validator identity by schema. A
  later caller cannot invent the validator used for them.
- Delivery receipt time and the delivery-time head/view are not recorded for all
  attempts. A claim timestamp is not a completion timestamp.
- A budget can include attempts from several actors/scopes. Scoped diagnostics
  cannot automatically disclose its global balances.
- Logical support charges, fixture byte counts and measured model tokens are
  different units. `charged_usage()` alone cannot qualify a policy comparison.
- Immutable historical output may still exist after withdrawal. Availability
  on disk and historical Accepted do not grant current export permission.

## B: incident preparation

Use one context occurrence plus the declared report occurrences in their exact
order. A context payload of version `incident-store-context/1` contains workload
and prompt identities, validator `incident-routing-protocol/1`, case ID, question,
ordered owner IDs, ordered external report IDs, system prompt, and the frozen
workload digest. No labels, family/split annotations or eligibility flags.

Source mappings are keyed by `(workload_version, case_id, external_id)`; IDs like
`e01` are not globally unique. A trusted pinned workload loader determines the
complete required ID set. A caller cannot shrink that set by supplying a smaller
contract. Require a distinct Store occurrence per mapping; identical payloads
may still be separate occurrences. Context is input position zero.

The manifest is exact JSON bytes of version `incident-store-manifest/1`, containing
the context occurrence ID and SHA-256, ordered full source mappings and payload
digests, workload/prompt/validator identifiers, output cap, and runtime identity
excluding its derived `prompt_fingerprint`. The latter is set to the manifest
SHA-256 in the immutable `RecoveryTiming` binding. The manifest also names initial
renderer `incident-request/1`, retry renderer `incident-request-retry/1` and fixed
feedback `incident-routing-protocol/1:invalid_output`. Retry ordinal 1 adds only
`validation_error` with that fixed string to the user payload; ordinal 0 retains
exact Python v1 bytes. Both alternatives are bound before the recovery starts,
so retry does not change the immutable timing contract. This exclusion avoids a
self-referential hash. Model/tokenizer/runtime identities and decoding remain
explicit even for the deterministic byte-token fixture.

Prepare compares that frozen timing binding with its reconstructed manifest,
then reconstructs the complete manifest from every `RecoveryStart::New` captured
payload and compares every digest and the final manifest hash again before
rendering. No request byte
comes from a caller's payload cache. Errors after new capture cancel only that
new provably-unsent attempt through the existing cancellation API. Existing,
sent, denied, terminal or otherwise ambiguous operations never produce another
send opportunity. No recovery state machine or implicit retry is added.

Canonical fixture JSON matches Python `ensure_ascii=False`, sorted object keys,
compact separators and UTF-8 exactly. Arrays preserve order. Unknown/duplicate
keys, floats/nonfinite values where integers are required, invalid UTF-8 and
depth above 64 refuse. Bound context/request/reply at 1 MiB each and manifests
at 1,024 input occurrences, including context. These are new resource limits.

Put the pure renderer/protocol validator in `src/incident_protocol.rs` so replay
can reuse it without invoking a Store or tool. It has no admission or send API.
Protocol validation checks format and supplied IDs, not scientific correctness.
Retry feedback uses fixed versioned protocol reason codes and never gold labels.

The reviewed hash dependency candidate is `sha2 = "=0.11.1"`, observed in the
[RustCrypto crate documentation](https://docs.rs/sha2/0.11.1/sha2/) on this date.
Check registry checksum, license, compiler compatibility and freshness at task
intake; commit the exact resolved lock. No hand-written SHA-256 or unrelated
dependency update. A discovered incompatibility is a documented successor
decision, not permission to change existing accepted pins silently.

## C: authorized decision bundles and replay

`DecisionBundleV1` is an export, never a transaction/import authority. Use exact
byte strings encoded as lowercase hexadecimal plus byte length and SHA-256;
never decode and re-encode arbitrary request/output/evidence bytes as UTF-8.
Limit decoded payloads to 4 MiB each, bundle JSON to 16 MiB, dependency closure
to 128 attempts and JSON depth to 64. Preserve repeated positions and occurrence
IDs even when content hashes match. Reject overflow before allocation.

A read transaction collects the root and required data dependencies: inherited
output, model-reuse origin and support rule inputs/formula. Parent lineage that
was not consumed is a reference, not a request to export all ancestor bytes.
Mutable current witness counters are not historical proof. Cycles, missing
dependencies and limits produce redaction/incomplete results, never substitutes.

CurrentRead requires the requested attempt to be the current eligible head and
every exported evidence input to be currently admitted in its own actor/scope.
Dependency outputs require the same current-read check. DiagnosticReplay permits
historical outputs only after a trusted synchronous authorizer accepts the exact
root/dependency selection and export purpose within that retained snapshot.
It does not override withdrawn inputs or restore redacted bytes. The callback
must not reenter/mutate this Store or perform network access; no reusable grant
is returned. A later revocation does not claw back an already exported file.

Denied/missing/withdrawn exports share `Redacted { requested_attempt, reason:
Unavailable }`, with no source strings, hashes, counts, payloads, requests,
validator messages or undisclosed lineage. SQL corruption/I/O is a separate
error; public errors carry no private values. Default diagnostic authorizer denies.
The caller's actor/scope identity remains the embedding runtime's responsibility.

Export per-attempt charges, reservation quote and retained hold. Omit global
budget balances and cross-scope contributions. Preserve budget ID as an opaque
local reference only where the root is authorized; do not claim that a partial
accounting view equals the whole budget. Units are `logical_support`,
`fixture_bytes`, `model_tokens` or `unknown`; infer a unit only from a recognized
immutable computation/runtime binding, never from numeric size.

Replay reports four separate facts: ledger consistency, request reconstruction,
validator evaluation, and the recorded historical disposition. It cannot
recompute general historical publication/timeliness without missing observations.
For `report-verdict/v1`, recompute the bound deterministic output. For the bridge,
use the captured declared protocol contract and timed manifest binding. Mark
that validator provenance `declared_contract`, not proof it ran historically.
Recognized invalid-output receipts retain `recorded_rejection` provenance.
Legacy unbound accepted responses return `ValidatorUnavailable` for validation.

Use a compiled allowlist of pure versioned validators/renderers. Unknown versions
are `NotReproducible`, not dynamic code loading. A changed validator yields a
new linked evaluation; the original disposition remains immutable. Missing raw
delivery with a retained hold is `Incomplete`, with unknown usage and the hold
preserved. Pure replay never opens `Store::open`, resolves URLs, loads models,
executes commands or invokes adapters.

## D: policy registry and evaluation

Registry key: `(actor, scope, purpose)`. Candidates are immutable exact artifacts
for `always_stop`, `affordable_retry` or `evidence_gated_retry`; schema version 1,
no programmable expressions or parameters. Each version owns a distinct evidence
occurrence. Registration does not admit it. The trusted runtime authenticates
actor/scope and policy-management access before calling Store; artifacts and
evaluation rows cannot authenticate their issuer.

An immutable comparison manifest identifies ordered case/family/split IDs,
workload/prompt, allowed exact per-policy request opportunities, initial evidence,
runtime/model/tokenizer, protocol validator, semantic scorer/labels, cap, budget,
original deadline, accounting unit and timing boundaries. Policy choices select
among those predeclared opportunities; changed requests cannot inherit outcomes.

Evaluation provenance is serialized and digested: `synthetic_registry_mechanics`
or `observed_policy_replay`. Synthetic records can activate only under purpose
`fixture/recovery-policy`; all other purposes refuse that provenance. Observed
replay rows reference the exact frozen request opportunities and attempt records;
case totals alone cannot establish those opportunities were executed. The Store
still treats external observations as attestations, not authenticated model proof.
Evaluation rows cover every scheduled case for both baseline and candidate.
Each row records outcome class, correctness, publication eligibility, completion
time or explicit unknown, false refusal, unsafe acceptance, charged units and
reserved bounds. Count success only when correct, eligible, accepted and completed
at or before the original deadline. A false refusal is refusal on a frozen case
labelled answerable with admissible support. Unsafe acceptance is any accepted
incorrect or ineligible result. Missing/unknown rows or unmeasured usage prevent
promotion, even when a conservative hold bound is present. All sums are checked
nonnegative integers; rows, not caller totals, determine counters.

Promotion requires strictly more successful cases and no increase in false
refusals, unsafe acceptances, charged-plus-reserved units or deadline misses.
It compares identical manifests, unit, case set and label/scorer versions. A
separately recorded baseline-only evaluation must match the paired baseline
rows exactly. Current v1 replay may yield no improvement: always_stop plus equal
initial outcomes and an added paid retry cannot meet the no-cost-increase rule.
Do not weaken the rule or alter old measurements. Positive activation tests use
explicit synthetic row attestations to test registry mechanics; they are not
empirical evidence that any current policy is superior.

Bootstrap succeeds only without a head, binds the baseline comparison manifest
and admits its policy occurrence. Promote requires expected revision plus active
baseline identity/digest equality inside the writer transaction. Rollback needs
prior activation, exact compatible contract, current authority and a nonempty
reason, but no superiority evaluation. Revision monotonically increases.

Every transition atomically withdraws the previous policy occurrence (if any),
admits the target, publishes final changed support verdicts through A's shared
transaction helper, updates the pointer and appends an event. Require the current
head occurrence admitted and an inactive target unadmitted; inconsistent external
membership changes refuse activation instead of silently repairing authority.
New operations capture the selected policy occurrence. Existing opaque/timed
work sees the changed scope epoch through ordinary gates; post-claim work may
still execute but cannot publish stale results. Never rewrite claims or holds.

Only D needs a new database schema. Allocate the next free version at integration
(v10 only if still free). Migrate under the existing Immediate initialization
transaction, preserve v9 IDs and usage, and reject future versions. Back up and
test using fixture/copy databases; do not migrate an owner's live database here.

## E: offline diagnostics and optional portability

Plan ablations for at most four declared evidence items. With r trials and pairs,
the schedule has r * (1 + n + n*(n-1)/2) rows; n=4,r=3 means 33. Bounds apply before
execution; the CLI only plans or scores recorded outcomes. Require every scheduled
row, keep failed/unknown trials and call counts, and require all baseline trials
to agree before computing associations. Ties stay tied; no causal or independence
claim and no pruning of opaque dependency manifests.

Portable envelopes remain conditional on an actual second consumer. Their parser
and digest checker cannot grant admission; a receiver must authenticate owner
mapping and recheck full reference, scope/purpose, context and current authority.
No cross-store atomicity, implicit TTL or live ablation runner.
