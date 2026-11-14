# Bounded recovery after validation failure

`cargo run --example budgeted_recovery` exercises the next controller decision:
after a verdict fails validation, spend at most one more call or abstain. It uses
the existing SQLite attempt, evidence, dispatch and token-budget contracts; it
does not change the public Store API or schema.

## What the example does

1. Capture the currently admitted report. Abstain unless exactly one well-formed
   report supplies a nonempty release and consistent positive total/pass counts.
2. Assemble and preflight the complete request. Check the deadline before and
   after preparation, then atomically reserve input plus output cap.
3. Recheck the deadline after reservation, because acquiring SQLite's writer
   lock can take time. Generate only if budget and time permit.
4. Validate the response against the captured report and settle its actual usage.
   A rejected response is charged and cannot become the branch head.
5. After the first invalid response only, recapture current evidence and include
   validation feedback in a new request. Obtain a new quote, not the old prompt's
   token count. A second invalid answer ends the invocation.

The fixed prompt requests JSON string fields `release`, `tests`, and `status`.
The user payload contains `ci_report` with `release`, `passed`, and `total`.
The retry additionally includes `validation_error`: “Return complete JSON matching
the captured release, passed/total count, and pass/fail status.” Rejected answer
bytes are never inherited as evidence. Exact serialized requests are recorded
by `reserve_dispatch`; this demonstration uses a fresh in-memory database.

## Deterministic versus live execution

The default adapter deliberately maps each UTF-8 byte to one fixture token. It
returns an incorrect `11/12 → pass` verdict, followed by the correct `fail`
verdict. It truncates at the cap and decodes incomplete UTF-8 with replacement.
Its charges test accounting mechanics, **not Qwen token usage or performance**.
The output explicitly identifies `deterministic_byte_token_fixture` and reports
zero new model calls.

`cargo run --example budgeted_recovery -- --live` uses the existing pinned local
Qwen worker, its actual prepared tensors, and measured usage. It needs the same
Python/model dependencies as the [output-cap pilot](token-cap-results.md).
The model can answer correctly on its first call; a retry is not forced. This
change does not include a live-model evaluation or claim improved accuracy,
latency, or token efficiency. The 96-token cap is conservative; the earlier
64-token calibration did not test this different prompt with retry feedback.

## Limits and failure handling

- The two-call limit is **per invocation**, not a durable recovery workflow ID.
  A caller must not restart a completed invocation to obtain more retries and
  call that the same bounded recovery. Persistent retry lineage is future work.
- An outstanding reservation causes this example to abstain, including on a
  second invocation with the same Store/budget. A lost response is `unknown_usage`;
  it retains the entire hold rather than guessing zero usage or sending again.
- If the deadline expires during reservation, no generation starts. The result
  is `deadline_expired_reserved`, with the conservative hold still outstanding.
  There is currently no reservation cancellation API. The example does not free
  such a hold by fabricating a delivery or charging zero tokens.
- The deadline measures eligibility to start and receipt after committed
  settlement. It is **not a hard process timeout**: in-flight generation is not
  interrupted. A correct late result is charged and can be stored, but it is
  reported as `late_result`, not a successful on-time completion.
- Changes during preflight are rejected by the Store dispatch gate; changes
  during generation are caught by settlement. A corrected report is recaptured
  for the retry. Missing evidence produces abstention, not a speculative answer.
- This is one synthetic scenario. A preregistered held-out comparison of always
  stopping versus affordable retries versus evidence-gated retries is still
  needed before drawing effectiveness conclusions. The historical 403/404
  replay remains in `cargo run --example token_cap_eval` and is not relabeled as
  a measurement of this new feedback prompt.

## Verification

`cargo test --example budgeted_recovery` covers retry feedback and combined
charges, a second invalid response, missing evidence, expired deadlines before
and after preparation and reservation, unaffordable retries, unknown usage and
re-entry, late charged output, and evidence replacement between attempts.
`cargo test --all-targets` runs these alongside the existing Store contracts.
