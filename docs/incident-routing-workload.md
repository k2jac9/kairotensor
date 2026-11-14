# Incident-owner routing workload v1

This is an **offline preparation and scoring milestone**, not a model run. The
synthetic corpus contains 12 cases: four calibration and eight held-out, with five
route decisions and seven semantic abstentions. It exercises language
interpretation of causal conclusions, negation, explicit corrections, conflicting
diagnoses and ownership records. It is a smoke benchmark, not representative
statistical evidence of model quality or proof that an LLM is necessary.

Arithmetic report verdicts remain on the deterministic route. This workload does
not change Kairotensor Store, the historical Adapter, experiments, or validators.

## Task and objective annotations

The question is which documented team owns the **diagnosed causal component**.
Mentioned symptoms and unconfirmed hypotheses are not diagnoses. An explicit
correction can supersede an earlier finding; mere later position does not.
Unresolved incompatible definite diagnoses require abstention. Routing requires a
unique documented owner; explicitly absent or conflicting owners require
abstention. No outside operational knowledge is needed or allowed.

The response has exactly four fields:

- `decision`: `route` or `abstain`.
- `owner`: a supplied owner ID for a route, otherwise JSON `null`.
- `reason`: `diagnosed_component` for routes; otherwise `undetermined_cause`,
  `conflicting_diagnoses`, or `owner_not_unique`.
- `evidence_ids`: a nonempty list of unique supplied evidence IDs.

Route citations identify the decisive incident note and matching ownership
record. Undetermined-cause citations identify the uncertainty note; conflict
citations identify both incompatible conclusions. Nonunique-owner citations
identify the diagnosis and the directory evidence establishing no owner or
multiple owners. Citation order is ignored, duplicates are invalid. The prompt explicitly requires
only the minimal supporting set and excludes unrelated or superseded notes. The frozen
reference specifies one exact supporting set per case. The corrections in v1 are
self-contained. Duplicate equivalent ownership records and repeated compatible
diagnoses are not tested; future versions must specify acceptable citation sets
for those cases rather than accidentally penalizing equivalent evidence.

## Protocol validation is not semantic scoring

`validate_output` checks strict JSON, exact fields, enums, route/abstention shape,
owner roster membership and known, unique citation IDs. It rejects duplicate JSON
keys (including escaped spellings), trailing prose and nonfinite JSON constants.
It does **not** consult reference labels or decide whether the language supports
the answer. A well-formed but wrong route can be protocol-valid.

Reference annotations are used only by offline `audit`. They must never become
runtime `ReplyValidation`, retry feedback, eligibility, admission or candidate
selection. This avoids giving recovery an answer oracle. A future recovery
experiment can retry malformed outputs using public format rules; semantic-error
retries require a separately justified non-oracle signal.

The scorer separately reports protocol validity, exact decision/owner/reason,
exact citation set, joint exactness and current joint success. Counts are reported
by split as well as overall. `received` counts every delivered response;
`current_received` counts only receipts declared current. They are not confidence intervals or population
accuracy estimates. Synthetic expected-response fixtures test scorer mechanics;
their scores must not be reported as model or held-out efficacy.

## Frozen files and split discipline

`workloads/incident-routing-v1/` contains separate `prompt.json`, `cases.json`,
`labels.json` and `contract.json`. `freeze.json` records their exact byte SHA-256
values; its own digest is pinned by version in `scripts/incident_routing.py`.
Changing a file, changing a version, or rewriting the manifest to bless changed
bytes fails loading. A future revision requires a new reviewed version and pins,
not in-place adjustment after seeing held-out results.

Case IDs are globally unique. Incident-family IDs cannot cross splits, and
normalized exact evidence text cannot cross splits. These guards detect declared
family and text reuse, not all possible semantic leakage. Here a family means the
underlying synthetic incident, not a broad reasoning skill such as negation.
Calibration and held-out cases may exercise the same skill on different incidents.
The split was authored offline before any model run on this version. The freeze
is integrity metadata, not independent proof of blind construction or annotation
correctness. Human review remains necessary.

Rendering uses only the prompt plus each case's question, owner roster and
evidence ID/text pairs. It excludes labels, expected abstention, family, split and
case ID from the actual model request. Calibration-only preparation excludes
held-out requests, evidence and labels; whole-corpus file hashes are harmless
provenance metadata, not held-out content. Case IDs in JSONL are external routing
metadata: a future adapter must send only the `request` string, not the whole
record, to a model.

## Prepare requests without inference

Run:

```
python scripts/incident_routing.py prepare --out /tmp/incident-calibration
```

The default is calibration only. `--split heldout` or `--split all` explicitly
selects other cases. The destination must not exist. Files are `prepared.json`
and `requests.jsonl`. There is no live mode, model adapter, tokenizer, token
estimate or network call in this tool. `--live` is rejected before creating output.

Every requested evidence ID must have an eligibility state. By default the
synthetic fixture declares all current. `--states states.json` can supply a list
of exact case records, each with `case_id` and an `evidence` list of `{id,state}`.
Allowed states are `current`, `missing`, `stale`, and `refused`. Records must cover
exactly the selected cases and every selected evidence ID without duplicates.
Any noncurrent selected item yields the same generic `evidence_unavailable`
refusal and no request. The renderer never silently selects a surviving subset.
This infrastructure refusal is distinct from a model's semantic abstention.

These states are fixture/replay declarations, **not Store-authenticated admission**.
A future integration must capture complete authorized evidence and recheck actual
Store freshness at claim/publication. Missing or revoked evidence is not grounds
to ask a model to infer its hidden content.

## Audit a complete response bundle

Run:

```
python scripts/incident_routing.py audit --prepared /tmp/incident-calibration/prepared.json --responses responses.json
```

Optional `--out result.json` exclusively creates a score file. The response object
has exact fields `schema_version` (integer 1), `version` (`incident-routing/1`),
`mode` (`synthetic_response_fixture` or `recorded_response_replay`),
`prepared_sha256`, `complete` (boolean), and `responses` (list). The prepared digest
is SHA-256 of the canonical prepared JSON: UTF-8, sorted keys, compact separators,
no ASCII escaping or nonfinite numbers.

Each response has exactly `case_id`, `request_sha256`, `state`,
`receipt_eligibility`, and `output`:

- For a generated request: `state=received`, output is the exact raw response
  string, and receipt eligibility is `current`, `stale`, or `refused`.
- For an uncertain result: `state=unknown`, output and receipt eligibility are
  null. This leaves the entire selected bundle unscored.
- For a preparation refusal: `state=not_requested`, output, receipt eligibility
  and request digest are null. Such cases are reported separately, never counted
  as correct model abstentions.

The audit independently reloads the digest-pinned workload and rejects mutated
in-memory annotations before scoring. It rebuilds preparation from the pinned
workload and declared states,
checks request/provenance hashes, and requires unique complete selected case
coverage. Missing/unknown results, mismatched hashes, unexpected IDs or malformed
metadata produce no aggregate score. Invalid model JSON in an otherwise complete
record counts as a protocol failure and incorrect answer, not a dropped case.
Malformed outer JSON is rejected before audit. Incomplete audit exits nonzero.

A response can match historical gold but be stale or refused at receipt. Its
semantic match remains distinguishable, but it cannot count as current joint
success. Receipt eligibility is still a producer attestation, not independently
verified authorization. Request hashes establish internal consistency, not model
identity, execution authenticity, token accounting or trusted source provenance.
No cost, latency or completion guarantee is inferred by this tool.

## Before any later live work

Use calibration cases only for prompt changes and runtime measurement, then freeze
a new version before an explicitly approved held-out run. A live integration needs
fresh exact prepared token counts for initial and feedback requests, a matching
model/revision/tokenizer/runtime/dtype/CPU/thread/readiness profile, coverage of
cold versus warm behavior, a measured timing allowance and an explicit bounded
call/budget plan. Historical report-task or synthetic fixture calibration does not
authorize this workload. No live inference is enabled or authorized by these files.
