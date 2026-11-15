# Complete policy comparison attestations

This module validates supplied records without opening a Store, authenticating an
actor, running a model, fetching a URL, or activating a policy. Management authority
comes from the trusted runtime. SHA-256 establishes byte identity and internal
consistency; it does not prove execution, honest normalization, or semantic truth.

The Rust API is `policy_registry::{validate_policy_artifact,
validate_policy_candidate, parse_policy_evaluation, evaluate_policy_comparison,
evaluation_sha256, paired_baseline_matches}`. Public types live in
`src/policy_registry/types.rs`. The Python counterpart is
`scripts/policy_evaluation_v2.py`. Structural errors are `rusqlite::Error` /
`ValueError`; a valid but unsuitable comparison returns a typed refusal. No database
schema, policy admission, migration, or dependency changes are part of this format.

## Encoding and bounds

All objects have exactly the documented fields: unknown, duplicate and missing
keys refuse. Nullable fields must occur explicitly as JSON `null`; omission is an
error. Integers are unsigned 64-bit, except artifact schema version (u32, exactly
1), and byte-array elements (u8). Booleans cannot stand in for integers. Digests
are exactly 64 lowercase hexadecimal characters. Identifiers and purpose are
nonempty, non-whitespace strings. Unicode must contain valid scalar values.

A byte payload is a JSON array of integers in 0..255, not text, base64, hex, or a
URL. `DigestedRecord` is exactly `{bytes, sha256}`; the digest covers those original
bytes, including whitespace in an embedded JSON document. Artifact bytes are
limited to 64 KiB. The complete serialized evaluation and manifest are each at
most 16 MiB; JSON nesting is at most 64 levels; each side and manifest contain at
most 10,000 scheduled cases. Caller-constructed values receive bounded checks
before building a serialized JSON tree. The frozen label payload is also limited
to 64 KiB. Every accounting addition is checked for u64 overflow, including
reservation quotes, row costs, and aggregate costs.

The derived evaluation identity is SHA-256 of ASCII `policy-evaluation/2\n`
followed by compact JSON of the complete typed `PolicyEvaluation`, with recursively
sorted object keys, UTF-8 scalar strings, ordinary JSON escaping, and no spaces.
Arrays retain order. Floats are absent. Python uses `ensure_ascii=False` and
separators `(',', ':')`; Rust uses sorted `serde_json::Value` object maps. All
original payload bytes remain arrays; embedded documents are never normalized
before their original-byte digests are checked. Provenance occurs in both the
comparison manifest and evaluation identity. It does not alter SHA-256 over an
original artifact, request, evidence, response, or label payload.

## Exact records

The Rust definitions are the normative field/type listing. The following groups
cover every record and its relationships; names are wire names unless noted.

- `PolicyArtifactV1`: `schema_version=1`, `policy_name`, `purpose`,
  `compatibility_contract_sha256`. Policy names are only `always_stop`,
  `affordable_retry`, `evidence_gated_retry`. No executable configuration exists.
- `PolicyCandidate`: `version_id`, `actor`, `scope`, `purpose`, `artifact` (bytes),
  `sha256`. The purpose must equal the parsed artifact purpose and its digest must
  match. Future Store registration supplies the evidence occurrence.
- `PolicyEvaluation`: `evaluation_id`, `actor`, `scope`, `purpose`, `baseline_id`,
  `candidate_id`, `baseline_sha256`, `candidate_sha256`, `experiment_manifest`
  (bytes), `experiment_sha256`, `evaluator_version`, `evaluator_sha256`,
  `provenance`, `baseline_rows`, `candidate_rows`. Selected version IDs are nonempty.
  Their artifact digests select entries in the shared universe. A baseline-only receipt uses equal IDs, equal digests and
  identical ordered rows on both sides. It undergoes full validation and returns
  `NoImprovement` when complete, never `Eligible`; unknown facts remain incomplete
  and malformed facts error. Conflicting same-ID digests or rows are incomparable.
- `ComparisonManifest`: `format="policy-comparison/1"`, `actor`, `scope`, `purpose`,
  `provenance`, `accounting_unit`, `workload_sha256`, `prompt_sha256`,
  `runtime_sha256`, `model_sha256`, `tokenizer_sha256`, `validator_sha256`,
  `scorer_sha256`, `compatibility_contract_sha256`, `policies`, `cases`.
- Each frozen policy: `policy_name`, `artifact` (`DigestedRecord`). Names and
  artifact digests are unique; parsed artifacts match name, purpose and contract.
  The universe contains one to three supported policy choices, independently of
  a particular evaluation pair or evaluation ID.
- Each case: `case_id`, `family_id`, `split_id`, `initial_evidence`
  (`DigestedRecord`), `label` (`DigestedRecord`), `budget`, `deadline_ms`,
  `timing_start_ms`, `timing_end_ms`, `policy_opportunities`. Case IDs are unique;
  a family cannot cross split IDs. All offsets share that case's original time
  origin; start <= deadline <= end. Every scheduled case stays in the denominator.
- Label bytes strictly encode `{answerable, admissible_support,
  normalized_format:"opaque-bytes/1", scorer_sha256, expected_normalized}`.
  `expected_normalized` is one exact byte array, frozen before observation. This
  format does not infer JSON, booleans, numbers, Unicode normalization, or alternate
  spellings from those bytes. The scorer must match the manifest scorer.
- Each `policy_opportunities` entry: `policy_name`, `opportunities`. Every frozen
  policy has exactly one entry. `always_stop` has one initial opportunity; each
  retry policy has the initial opportunity plus one retry. Initial opportunities
  agree across policies except for their local opportunity ID.
- Each opportunity: `opportunity_id`, `request`, `evidence_manifest` (both
  `DigestedRecord`), `admitted`, `consistent`, `input_bound`, `output_cap`,
  `start_min_ms`, `start_max_ms`. IDs are unique within the path. Initial evidence
  matches the case's exact record. The start window lies inside case timing bounds.
- Each row: `case_id`, `outcome`, `correct`, `eligible`, `answerable`,
  `completed_ms`, `charged`, `reserved_bound`, `false_refusal`, `unsafe_acceptance`,
  `deadline_miss`, `simulated`, `attempts`. Outcomes are `Accepted`, `Refused`,
  `Failed`, `Unknown`; correctness, eligibility, charge and completion are nullable.
  Both sides cover the same ordered manifest cases exactly once.
- Each observed attempt: `execution_id`, `opportunity_id`, `request_sha256`,
  `evidence_manifest_sha256`, `execution`, `disposition`, `response`,
  `measured_input`, `measured_output`, `retained_bound`, `start_ms`, `completed_ms`,
  `validator_sha256`, `scorer_sha256`, `label_sha256`, `scoring`. `response` is a
  nullable `DigestedRecord`. Measurements, completion and scoring are nullable.
  Execution is `Executed`, `NotExecuted`, or `Unknown`; disposition is `Accepted`,
  `InvalidOutput`, `Stale`, `Failed`, `Uncertain`, or `Refused`.
- A scoring assertion is exactly `{response_sha256, scorer_sha256, label_sha256,
  normalized}`. `normalized` is a byte array, compared exactly to the frozen
  expected bytes. The assertion binds the actual response record and frozen label
  and scorer identities. Raw response -> normalized bytes is an **external
  attestation**, not an implemented general scientific scorer. Missing scoring for
  an observed response remains incomplete.

Accounting units are `model_tokens` or `fixture_bytes`. `logical_support` and
`unknown` can be represented but make comparisons incomparable. Units, workload,
requests, labels, runtime, cap, budget and deadlines are frozen in one shared
manifest; the baseline and candidate cannot choose different manifests. A future
bootstrap and later paired evaluations must reuse those exact bytes.
`paired_baseline_matches` checks the exact baseline projection (authority scope,
policy version/digest, manifest bytes/digest, evaluator, provenance and ordered
rows); evaluation IDs and candidate fields are deliberately excluded. It is not
an activation or a substitute for validating either record.

## Observed replay and unknowns

Provenance is `observed_policy_replay` or `synthetic_registry_mechanics`.
Observed rows set `simulated=false`. References must match the selected named
policy's frozen ordered opportunity prefix. Only a settled `InvalidOutput`
unlocks the single retry; accepted, stale, failed, uncertain and unknown outcomes
cannot unlock it. Correctness labels never influence retry selection.

Before execution, admitted evidence, full fresh `input_bound + output_cap`,
remaining budget, and start strictly before the original deadline are required.
The evidence-gated retry also requires frozen consistency. A failed gate requires
an explicit terminal `NotExecuted` refusal with zero usage, no response/scoring,
no hold, and completion equal to its start. An unlocked but missing retry reference
is incomplete, including when the producer must explicitly attest nonexecution.
Attempt starts must respect the frozen window and prior completion. Completion
must be between start and the case timing end. Completion **equal to the deadline
is on time**.

Settled execution has bounded measured input/output, zero retained hold and a
non-uncertain disposition. Unknown usage retains the full quote, leaves row charge
unknown, and is never eligible. An infrastructure failure without a response is a
failed result, not a semantic success; missing response/scoring for other settled
response outcomes is incomplete. Stale dispositions are ineligible. Invalid-output
exhaustion and explicit refusal produce a refused row. Row charge is the checked
sum of measured input/output, completion is the terminal completion, correctness
comes from the scoring assertion, and eligibility comes from frozen admission and
support (and stale disposition). All supplied redundant row values must agree.

An execution ID can recur across policy paths only for the same case and compatible
present facts. Fixed request/evidence/validator/scorer/label identities, start time,
and opportunity input bound/output cap must agree; path-local opportunity IDs may
differ. Two present optional facts or known execution/terminal states cannot
conflict. A null optional fact, `Unknown` execution, or `Uncertain` disposition
remains missing knowledge. Known facts accumulate solely for order-independent
conflict detection, including response/scoring associations supplied separately.
They never complete a local record, infer a measurement, or release its hold.
Each local hold is validated independently: missing usage retains the full quote;
settled measured usage retains zero. Their resulting hold difference is permitted.
All independently known timing, reference, scoring, gate and transition facts are
checked before an incompleteness refusal; a missing completion cannot hide an
accepted-to-retry transition or a denied later gate. Incomparable shared facts take
precedence over incompleteness independent of which side is checked first.
An execution cannot be counted for another scheduled case. Distinct execution IDs
can refer to equal request bytes: request equality is not proof of identical
independent realizations.

Synthetic rows have `simulated=true`, no attempt references, and explicitly
simulated counters. Their eligibility and answerability still match frozen support
and labels. They can qualify only for `fixture/recovery-policy`. The positive
fixture tests registry selection mechanics; it does **not** describe feasible
always-stop/retry measurements or demonstrate superiority of a real policy.

## Selection and refusals

Success means accepted, correct, eligible and completed at or before the original
deadline. False refusal means refused on a frozen answerable case with admissible
support; a row-local ineligible flag cannot hide it. Unsafe acceptance means an
accepted incorrect or ineligible result. Deadline miss means completion after the
original deadline. Charge plus retained bound is the cost. Known flags are checked
even on incomplete rows; unknown rows cannot hide fabricated later rows.

Unknown outcomes, correctness, eligibility, usage or completion and missing
references/cases prevent eligibility (`Incomplete`). Identity/unit/reference
mismatches and unknown evaluator contracts return `Incomparable`. Structural
malformation, impossible counters, timing, budget, flags and overflow are errors.
After completeness, refusals are checked in this order: `NoImprovement`,
`IncreasedFalseRefusal`, `IncreasedUnsafeAcceptance`, `IncreasedCost`,
`IncreasedDeadlineMisses`. Eligible requires strictly more successes and all four
nonincrease conditions. Rust returns `SelectionDecision::Eligible {
validated_counters }` or `SelectionDecision::Refused(SelectionRefusal)`; Python
returns `{decision:"Eligible", validated_counters:{baseline,candidate}}` or
`{decision:"Refused", reason:<typed name>}`. Each side's counters are `cases`,
`success`, `false_refusal`, `unsafe_acceptance`, `cost`, `deadline_misses`.

The evaluator version is exactly `policy-selection/1`. Its contract bytes are the
following ASCII line sequence, with LF after each line (including the last):

```text
policy-selection/1
complete-paired-frozen-attestations;success=strict-increase;false-refusal,unsafe-acceptance,charged-plus-reserved,deadline-miss=nonincrease;synthetic=fixture/recovery-policy-only
```

Its SHA-256 is
`df97ae41b2648bef15a11bf1950883597b057df1940fe43347d485fedf8590b3`.
An arbitrary caller-supplied digest is not accepted as that evaluator identity.

## Archived input conversion and golden records

The existing recovery script's default output and `frozen-v1` semantics are
unchanged, including archived input `schema_version=2` (nullable unused retries).
That input schema is unrelated to this complete evaluation format. Explicit use:

```sh
python3 scripts/recovery_policy_eval.py --fixture --convert-v2
python3 scripts/recovery_policy_eval.py TRACE --convert-v2 --comparison-v2 COMPLETE.json
```

Conversion without a matching complete supplied attestation returns
`policy-evaluation-conversion/2`, `status:"Incomplete"`, a source digest and a
reason; it never fills unknown facts from protocol parse success. With supplied
facts, `workload_sha256` must bind compact sorted-key UTF-8 JSON of the archived
input; cases/families/splits, token unit, exact requests, quotes, gates, deadlines,
timings and old measured row totals must match. For observed conversion, each
supplied executed or nonexecuted opportunity is additionally reconciled with the
legacy path: dispatch offset equals prior completion plus recorded preparation
duration; completion adds the recorded attempt duration for executed attempts, or
equals dispatch for a gated nonexecution. Execution/disposition, per-attempt
input/output split and charge, zero/nonzero holds, and the recorded correctness
assertion must agree. Equal aggregate time or cost cannot hide changed attempt
facts. Frozen evidence-manifest bytes in this legacy binding must equal the exact
UTF-8 archived `evidence_id`; this binds the old opaque identity, not reconstructed
evidence contents. Labels, raw responses and normalization remain supplied
attestations. Missing legacy retry contexts, unrecorded events or uncertain legacy
usage stay incomplete; an omitted event is never assigned an invented duration.
A complete conversion returns the supplied evaluation and its selection result, which may refuse. The existing positive-retry corpus correctly returns
`IncreasedCost`; no worse baseline or changed selection rule is used.

The inline `fixture()` and `golden_cases()` in
`scripts/test_policy_evaluation_v2.py` define the exact shared test bytes; Rust
loads these same bytes and checks independent literal expected outcomes, including
positive synthetic and negative malformed/incomplete/incomparable/gate examples.
The positive canonical evaluation is 22,057 bytes; its manifest is 6,477 bytes.
Pinned golden identities:

| Payload | SHA-256 |
| --- | --- |
| Synthetic evaluation (domain prefix included) | `2b0ed900f956cd8fc2dd215e114a665d4799187dd6610209807fe557a1311a88` |
| Exact manifest bytes | `1373dd5ff2bc8e1a06840f8feb31329b1a6666d434712db2a6fe4a3ef537c265` |
| Baseline artifact bytes | `b8fd122e67c0094b1f7bc1fc91884387249299a603c3e8c9b3e3c56742d676bc` |
| Candidate artifact bytes | `b98a8a7d58c2592bee588fb26884da853832b44688e33ca0ede5648d57904ce5` |
