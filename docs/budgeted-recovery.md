# Durable bounded recovery

`cargo run --example budgeted_recovery` connects a validation failure to one retry
or abstention. Schema v7 makes the logical recovery identity, attempt slots,
reservation decisions and single-use send claims persistent. The example uses
these APIs with an in-memory store; the integration tests close and reopen actual
SQLite files at each boundary and race independent connections.

## State and API contracts

| API | Contract |
| --- | --- |
| `create_recovery(key, budget, actor, scope, branch)` | Create or return the same logical operation. A reused key with different bindings fails. The caller must retain the key and budget identity across restarts. |
| `begin_recovery_attempt(recovery, candidates)` | Atomically capture current evidence and allocate ordinal 0, or ordinal 1 only after the first invalid delivery. `New` returns captured bytes and the persisted retry reason. Concurrent/restarted callers receive `Existing(attempt)` for unfinished work or `Finished` for a terminal workflow. |
| `reserve_recovery_attempt(attempt, request, input, cap)` | Freeze exact request and quote, reserve capacity, and record `reserved` or `denied` atomically. Does not authorize inference. Duplicate calls fail rather than replacing the quote. |
| `recovery_reservation(attempt)` | Read the exact bytes, input count and cap of a still-reserved attempt. Read-only, no send permission. |
| `claim_recovery_dispatch(attempt)` | Check current evidence/branch eligibility, change `reserved` to `sent`, and insert the dispatch in one transaction. Exactly one caller gets `Some(request)`. Non-reserved states return `None`; stale eligibility or database failures return errors. |
| `cancel_recovery_attempt(attempt)` | Cancel `planned` or `reserved` work and release its hold without inventing usage. Works even with stale evidence. Repeated cancellation is true; sent or denied returns false. |

`planned → reserved → sent → measured delivery` is the normal path. Preparation
can be cancelled before reservation; reserved work can be cancelled before claim.
Budget denial or cancellation terminates the latest slot. A first `invalid_output`
delivery unlocks one fresh capture, including when its
separately recorded applicability is stale. This allows corrected evidence to be
recaptured after an invalid answer without reusing the old report. Once ordinal 1
exists, neither cancellation nor denial can reopen ordinal 0. The database permits
only ordinals 0 and 1. Accepted and stale *validation-passing* deliveries terminate
recovery without a retry. The evaluation protocol intentionally uses a stricter
policy: it normalizes invalid-but-stale applicability to a terminal stale outcome.
That experimental policy does not change the core recovery contract.

**Every transport must claim before sending, and send at most once after its own
successful claim.** A caller must not treat `Existing`, a saved request, a missing
delivery, or a duplicate claim as permission to send. The database does not
control external model/network I/O. As elsewhere in Kairotensor, the local caller
is trusted to obey these boundaries; creating a fresh recovery key intentionally
starts a different operation.

## Restart and concurrency

A live worker's prepared tensors do not survive process death. For a still-reserved
attempt, read `recovery_reservation`, prepare those exact saved bytes again, and
compare request bytes, input count and output cap to the saved quote **before**
claiming. The example does this; a changed quote cancels the unsent slot and never
runs inference. If another worker or cancellation wins the claim, discard the
local preparation. Never replace the reserved prompt or token counts.

A crash after a committed claim is ambiguous even if it happened before the
network call. The `sent` state and full hold survive restart. Another worker cannot
claim again or cancel it. Reconcile only a known measured delivery, including a
late valid, invalid, or stale response. Duplicate identical delivery charges once.
No timeout-based release or automatic resend is provided. A crash while still
`planned` returns `Existing`; a caller may explicitly cancel that unsent slot.
Cancellation is terminal, not a way to reset the workflow's retry count.

Claim/cancel, conflicting reservations, duplicate initial/retry allocation, and
claim/claim all serialize under SQLite's immediate writer transaction. Deliveries
for unclaimed recovery attempts are rejected by a database trigger, including
ordinary `settle` calls that would otherwise publish a head. Legacy
`record_dispatch` and `reserve_dispatch` reject recovery-owned attempts. Existing
ordinary `reserve_dispatch` semantics stay conservative: those reservations may
already have been sent, so migration never makes them cancellable.

## Evidence, deadlines and validation

Each new attempt captures current admitted evidence. The example requires exactly
one consistent report (`release`, `passed`, positive `total`). The user payload
is `ci_report`; retry payloads also contain the persisted `validation_error`:
“Return complete JSON matching the captured release, passed/total count, and
pass/fail status.” Rejected answer bytes are never inherited as evidence. Each
changed prompt is freshly quoted. Returned JSON must have exactly the string
fields `release`, `tests`, `status`, matching the captured evidence.

Deadline checks occur before preparation, after preparation, after reservation,
and after claim. Expiry after reservation cancels known-unsent work and returns
`deadline_expired_cancelled` with no remaining hold. Expiry after claim returns
`deadline_expired_claimed` without inference, but preserves the potentially-sent
hold: the claim is intentionally the conservative uncertainty boundary. Missing
input cancels a planned slot. Preparation/reservation/claim errors attempt safe
cancellation; cancellation itself can never release a sent hold. If database
cleanup fails, the original error is returned and the caller must inspect state.

The deadline is an admission and receipt deadline, **not a hard process timeout**.
In-flight generation is not interrupted. A correct late result is charged and may
be stored, but is reported as `late_result`, not an on-time success. The database
stores the retry bound, not the application's deadline policy; a resumed caller
must supply the same end-to-end deadline rather than restarting its clock.

## Deterministic versus live execution

The default adapter maps each UTF-8 byte to one fixture token. It returns an
incorrect `11/12 → pass` response then `fail`, truncates at the cap, and decodes
incomplete UTF-8 with replacement. Its charges test mechanics, **not Qwen usage
or performance**. Output is marked `deterministic_byte_token_fixture` with zero
new model calls. The demo creates a fresh in-memory budget; its recovery key is
stable for that budget across calls to the controller.

`cargo run --example budgeted_recovery -- --live` uses the existing pinned Qwen
worker and actual prepared-token counts, with the same dependencies as the
[output-cap pilot](token-cap-results.md). It may succeed on the first call. No
live recovery experiment has been performed as part of this change, and no
accuracy, latency or efficiency gain is claimed. The conservative cap remains
96: the earlier 64-token calibration did not evaluate this feedback prompt.

The [offline policy evaluator and comparison protocol](recovery-policy-protocol.md)
now define always-stop, affordable-retry, and evidence-gated retry comparisons.
Its synthetic checks are not a live held-out experiment; measured recovery
trajectories are still needed. The historical 403/404 boundary remains in
`cargo run --example token_cap_eval`; its archived counts are not reused as
measurements of this new prompt.

## Verification

`cargo test --test recovery` covers file reopen, one retry across processes,
concurrent initial/retry allocation, claim/claim, claim/cancel, reserve/reserve,
reserve/cancel, uncertain sends, late and duplicate replies, immutable binding,
stale cancellation, preclaim settlement rejection, transaction rollback, and
populated v6 migration. `cargo test --example budgeted_recovery` additionally
checks feedback quotes, evidence correction, preparation cleanup, all deadline
boundaries, reserved-worker replacement, quote mismatch, and repeated invocation.
`cargo test --all-targets` runs both with existing contracts.
