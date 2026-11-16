# Bounded offline decision ablations

`scripts/decision_ablation.py` plans hypothetical omissions and scores explicit
recorded outcome attestations. It performs file input/output only. It has no run
mode, backend imports, executable command output, process calls, model loading,
provider requests or network access. E2 portability remains Deferred; Rocket is
outside this work.

```sh
python3 scripts/decision_ablation.py plan complete-bundle.json > plan.json
python3 scripts/decision_ablation.py plan complete-bundle.json --trials 3 --include-pairs --max-calls 33 > pairs.json
python3 scripts/decision_ablation.py plan larger-bundle.json --selected-id 7 --selected-id 2 > selected.json
python3 scripts/decision_ablation.py score plan.json recorded-outcomes.json > report.json
```

The corresponding pure interfaces are `plan_ablations(bundle, trials=3,
include_pairs=False, max_calls=33, *, selected_ids=None)` and
`score_ablations(plan, outcomes)`. Refusals raise `ValueError`; the CLI prints a
reason on stderr and exits 1. JSON goes to stdout; callers choose output paths.
No new request bytes or send permission are produced.

## Supported source and its limits

Planning accepts a complete, integrity-checked `DecisionBundleV1` for one direct
root of the initial `incident-store-manifest/1` contract only. It requires ordinal
zero, no predecessor, no inherited/reused output, model contract, deterministic
contract or support/policy contract. An opaque parent lineage reference may remain;
it is never selected, expanded or recomputed. Retry and unsupported contracts refuse.

Checks include exact nested field sets and scalar types; positive i64 IDs;
unique evidence occurrences and scopes; matching root ID, branch, actor and scope;
every selected/input payload and the recorded actor/scope authorization entry;
compact ordered candidates/inputs; context at position zero; and report roles
derived independently from the known context and full ordered manifest source map.
Free-form evidence `source` bytes and fixture eligibility claims do not determine
roles. Context, policy and lineage cannot become selectable by changing a role
label and resealing the outer digest. Incident/1 report roles must be exactly
`report`; an `evidence` label cannot replace them.

The planner verifies exact manifest versions, canonical manifest bytes, context
payload digest, case/workload/external source mappings and report hashes,
context/manifest freeze/prompt/validator agreement, fixed initial/retry renderer
and feedback identifiers, full immutable runtime, timing runtime equality,
manifest runtime identity, manifest-derived prompt fingerprint, output cap,
typed calibration/timing and local request/state/accounting consistency.
It accepts `incident-routing-protocol/1` provenance `declared_contract` with a
valid manifest, or `recorded_rejection` with matching recorded invalid-output
rejection metadata. Other provenance or conflicts refuse. Source accounting
matches the narrow C contract: `unknown`, or exact canned-runtime `fixture_bytes`.

It reconstructs the original initial request from captured context and report
bytes, without loading workload labels, then requires equality with the captured
recovery request and also dispatch request when sent. An unsent reserved, denied
or cancelled prepared root may use its captured recovery request; a planned root
without request bytes is unavailable. Unknown or inconsistent states refuse.
These checks establish declared-contract consistency. They do not run a Rust
ledger/protocol validator, authenticate historical metadata, replay timing,
renew current access or authorize export/send. Scoring without the source bundle
checks plan consistency only; it cannot reauthenticate source applicability.

All public bundle/plan/outcome objects are counted before cloning/serialization:
exact compact UTF-8 size at most 16 MiB, depth at most 64, finite JSON types with
signed i64 integers and no floats, cycles, arbitrary Python objects or surrogates.
Files are read with the 16 MiB transport cap before parsing; duplicate keys refuse.
Byte envelopes retain their independent 4 MiB cap, lowercase hex, length and hash.
Context and reconstructed requests retain the separate 1 MiB incident cap.
At most 1,024 incident input occurrences including context are supported.

## Deterministic plan

`decision-ablation-plan/1` contains `source_bundle_digest` (SHA-256 of the full
canonical source bundle including its integrity record), `selected_ids`,
`selected_positions`, `trials`, `include_pairs`, `max_calls`, `scheduled_calls`,
`scorer`, `runtime`, `validator`, and `rows`. `runtime` preserves the original
full immutable-runtime byte envelope; `validator` preserves version and provenance.
They are source comparison identities of hypothetical omissions. In particular,
the original prompt fingerprint does not authorize a changed request.

`selected_ids=None` selects all report occurrences when there are at most four;
a larger bundle requires an explicit list of up to four distinct selectable IDs.
The list is sorted into original input order. Unknown IDs, context IDs and duplicate
selections refuse. An explicit empty list schedules baseline only. Identical
payload bytes under distinct IDs remain distinct selected items. `selected_positions`
records each ID and its original positions. Incident/1 forbids repeated IDs, so the
public planner refuses them; only the internal grouping helper supports collecting
several positions for a future generic scheduler. No new public contract is implied.

Trials must be an integer from 1 to 100, `include_pairs` a boolean, and `max_calls`
an integer from 1 to 10,000. The complete schedule must fit before a plan is emitted:

```
r * (1 + n + (n * (n - 1) / 2 if include_pairs else 0))
```

Baseline, source-ordered singles and lexicographically ordered pairs each get every
repeat index `0..r-1`. Each removal list retains source order. For four items, three
repeats and pairs, there are exactly 33 calls; a bound of 32 refuses. No ancestor
computations are implicit. Omission leaves the source bundle unchanged.

Rows contain exactly `trial_id`, `repeat_index`, `removed_ids`, `variant_digest`.
The variant digest hashes compact sorted-key UTF-8 JSON with fields
`version="decision-ablation-variant/1"`, `source_bundle_digest`, ordered `removed_ids`,
`scorer="recorded-result-equality/1"`, full source `runtime` and source `validator`.
Repeat index is excluded; the unique trial ID is `<variant_digest>:<repeat_index>`.
The plan digest used by outcomes is SHA-256 of the full canonical plan.

## Recorded outcomes and scoring

The exact outcome object is:

```json
{"version":"decision-ablation-outcomes/1","plan_digest":"<64 lowercase hex>","rows":[]}
```

Each supplied row has these exact fields (one row for every scheduled trial for
complete coverage):

```json
{"trial_id":"<variant_digest>:0","state":"recorded","response_digest":"<64 lowercase hex>","semantic_result":{"version":"recorded-normalized-result/1","value":{"decision":"abstain"}},"charged_units":2,"reserved_bound":10,"unit":"fixture_bytes","scorer":"recorded-result-equality/1","runtime":{"hex":"<original runtime bytes>","byte_len":0,"sha256":"<original runtime digest>"},"validator":{"version":"incident-routing-protocol/1","provenance":"declared_contract"}}
```

The displayed runtime is a shape placeholder: copy the exact plan runtime envelope,
including the correct length, lowercase hex and SHA-256. These examples assert no
provider/model execution. Outcomes are supplied attestations, not synthesized
measurements or proof of execution.

`state` is `recorded`, `failed` or `unknown`. A recorded row requires a response
digest and the versioned semantic-result object. Its `value` is any bounded strict
JSON value: null, boolean, signed i64 integer, string, array or string-key object;
no floats. Equality compares canonical encoded bytes, so boolean `true` differs
from integer `1`, object key order is irrelevant, and array order matters. The
full normalized-result object has a 1 MiB cap. A failed/unknown row has null
`semantic_result`; its response digest may be null or retained if bytes exist.
No result normalization or scientific-truth validator runs. A producer claims that
its supplied result was validated; this tool compares that recorded attestation.

`charged_units` is a nonnegative integer or null for unknown measurement;
`reserved_bound` is a nonnegative integer bounding any known charge. Unit is
`model_tokens`, `fixture_bytes`, `logical_support` or `unknown`. Outcome units
are reported separately, even though this initial source contract is narrower.
All scalar measurements and named-unit aggregate sums must fit i64. In each
known named-unit bucket, unknown charges keep `charged_units` null, with
`known_charged_units` retaining the known partial sum and `unknown_charges`
counting unknown measurements. `reserved_bound` sums that named unit's observed
row bounds; `held_units` conservatively retains the entire bound for failed/unknown
rows or any unknown charge. It is a reported uncertainty bound, not a reconstructed
Store hold. Recorded known-charge rows contribute no hold.

The label `unknown` does not establish commensurable units, even when rows share a
plan or runtime and each has a numeric charge. Its cost bucket therefore keeps
`charged_units`, `known_charged_units`, `reserved_bound` and `held_units` all null.
Only `observed_trials` and `unknown_charges` are counted. Its `observations` list
preserves each unspecified-unit row in supplied order with exactly `trial_id`,
`state`, original numeric-or-null `charged_units`, original `reserved_bound` and
conservative per-trial `held_units` (the full bound for failed/unknown rows or an
unknown charge, otherwise zero). No unknown-unit magnitudes, bounds or holds are
summed, including for overflow checks; each individual measurement remains bounded.
For example, charges 3 and 7 with unspecified units and bounds 17 and 23 remain
two observations with null aggregate measurements, rather than charges totaling 10
or bounds totaling 40.

Missing outcome rows have unknown costs and bounds, explicitly counted by
`unreported_cost_trials`; their costs are never silently assigned zero.

Before semantic scoring the scorer independently regenerates every row of the
bounded baseline/singles/enabled-pairs schedule and checks exact equality and
count. Removing baseline repetitions or pairs, duplicating repeat indices,
changing variant digests/removals or lying about counts refuses even after the
caller rehashes the plan. Unknown/duplicate outcome trial IDs or a changed plan
digest refuse. Every row must bind the plan's scorer, full runtime and validator.
Different identities yield `comparison_status="Incomparable"` and no scores/ranks.

Reports keep separate facts: `coverage` is `Complete` or `Incomplete` according
to missing rows; `baseline_status` is `Stable` or `Inconclusive` with unavailable
and/or unstable reasons; `comparison_status` is `Comparable` or `Incomparable`
according to identities. They may coexist. No absent/failed/unknown baseline or
unstable baseline supports scoring. Scheduled calls, observed outcome rows and
recorded rows are separate counts; none authenticates actual provider execution.

Every variant retains changed/unchanged/failed/unknown/missing/unscored counts out
of all scheduled repeats. Recorded rows are `unscored` when the baseline or
comparison is unusable. A variant with any missing/failed/unknown trial gets no
association score or rank. Otherwise its score is `changed / r`. Scores are
associations, including pairs, and are not additive causal effects or proof of
independent evidence. Equal scores keep equal competition ranks; display order
remains original singles followed by ordered pairs. Scores never change admission,
prune dependencies or grant authority.

Verification uses a real precompiled C3 fixture exporter/replayer identified by
`KAIROTENSOR_DECISION_REPLAY`, explicit synthetic declared-contract mutations and
recorded fixtures, plus process/network hooks that raise during actual APIs and
both CLI commands. The test harness may prepare/replay fixtures in separate offline
processes before those hooks; public plan/score never invokes them. CI requires the
precompiled runner and does not silently build or skip these checks.
