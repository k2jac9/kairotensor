# Recovery-policy comparison protocol

Status: offline evaluation tooling and synthetic contract checks only. No new
model calls have been run, no policy has been selected for production, and no
accuracy, latency or efficiency gain is claimed. This protocol must be frozen
alongside the exact corpus, prompts, model revision and run configuration before
collecting a new held-out model experiment.

## Question and frozen policies

Does spending one additional, freshly quoted attempt after an invalid answer
increase correct, on-time completions enough to justify the extra tokens?
Compare three policies on the same initial attempts:

- `always_stop`: never dispatch a retry.
- `affordable_retry`: after an invalid first delivery, retry once if current
  evidence is admitted, the fresh input-plus-output reservation fits the
  remaining budget, and preparation completes before the end-to-end deadline.
- `evidence_gated_retry`: apply the same checks and additionally require that
  the current evidence is consistent.

For this report task, consistent means one complete report with mutually
consistent release and passed/total values. The consistency gate uses evidence
available **before dispatch**, never the correctness of a prospective response. It is an experimental policy distinction,
not permission to bypass the Store's stale-evidence or branch checks. The existing
Rust example requires one consistent report and does not change in this milestone.
The evaluator compares records; it does not issue send claims or model calls.

An accepted, stale, or uncertain first delivery never unlocks a retry. An invalid
response can be charged even when there is no affordable retry. A missing response
after claim leaves the full reservation outstanding. Unknown usage is not zero
usage, and a timeout alone cannot release it. At most two dispatches belong to a
logical operation under each policy.

## Pairing and timing

Keep the same initial attempt, budget and deadline across all policies for a case.
Use identical retry proposals for both retry policies, since only their gate
differs. For each retry policy, record the exact proposed request, evidence identity,
admission/consistency facts, fresh input count and output cap before dispatch.
When both policies have a retry record, their outcomes must also be identical;
this is a replay of the same observed trajectory, not two independent draws.
Only a corresponding response with the same request, evidence and quote may score
that policy. Do not reuse a completion from a different prompt or assume that a
feedback prompt would produce the original response. Shared initial trials make
the policy comparisons paired and dependent, not independent sample groups.

Each context records preparation/admission duration, and each response records
post-admission duration through receipt. The evaluator accumulates these durations
from the same operation start, including preflight, reservation, inference,
validation and settlement. Admission must occur strictly
before the deadline; a correct accepted receipt at the deadline is on time.
Late measured responses remain charged. Preparation time is not reset for a retry
or a restarted worker. Missing required response records invalidate a complete
comparison rather than silently turning into failures or invented completions.

## Split and reporting

Assign entire report families to calibration or held-out evaluation before any
model execution, with no family or case duplication across splits. This harness
has no fitted threshold: the three policies are fixed, and reports keep the two
splits separate. Any future choice of prompt, cap, budget, deadline or consistency
predicate must be fixed using calibration data only and recorded before examining
held-out outcomes. A new corpus and machine-readable run manifest are required
for a live experiment; the synthetic smoke cases are not that corpus.

The primary metric is correct, accepted, on-time final outcomes divided by **all
cases** in the split. Report abstentions and failures in that denominator. Report
correct but late outcomes separately, along with dispatches, actual charged tokens,
outstanding reservations, and successes per charged token where defined. The
`on_time` field tracks the last measured receipt when a later retry is declined;
it does not imply successful completion. An uncertain latest attempt clears the
receipt flags. The primary final outcome is `success`. Charged
usage omits unknown usage, so efficiency based on incomplete ledgers is not a
complete cost estimate. Sum policy resources separately; they are hypothetical
selected paths, not the amount spent collecting every branch in an experiment.

Record provenance, raw prompts/responses, model/tokenizer revisions, generation
settings, independent reference labels, validator version and hardware/timing
conditions in a future measured artifact. The replay tool validates record
consistency; it cannot authenticate supplied correctness labels or prove that the
claimed model executed. Synthetic examples establish evaluator contracts only.
No confidence or production-reliability claim is justified by these fixtures.

## Reproducible offline checks

Run `python3 scripts/recovery_policy_eval.py --emit-fixture` to inspect the full
versioned input schema. Run `python3 scripts/recovery_policy_eval.py --fixture` to evaluate the explicitly
synthetic paired cases with zero new model calls. Run
`python3 -m unittest discover -s scripts -p 'test_*.py'` for schema, admission,
accounting, matching, deadline and split regressions. A saved compatible document
can be scored with `python3 scripts/recovery_policy_eval.py PATH.json`.

The [Store-backed collector](recovery-policy-runner.md) now provides a versioned
corpus and instruments matching trajectories through the durable Rust recovery
APIs. A live held-out comparison still requires a completed measured run and
approval for any required compute cost. Archived output-cap results do not
measure the new feedback prompts and must not be presented as recovery results.
