# Deadline-aware recovery admission

The timed recovery route adds a conservative dispatch gate to durable recovery.
It is opt-in: existing untimed APIs retain their established behavior, and a timed
operation cannot bypass its gate by using the legacy claim method. No model is
needed to exercise the contract.

## Time and restart contract

Bind the original operation start, absolute deadline, and clock-domain identity
once. A restarted process must retain the same operation key and original window.
It may continue only if its trusted runtime can supply time in that same monotonic
clock domain. A process-local elapsed timer restarted at zero, a different boot,
or an unknown mapping to another machine is not equivalent. Do not fall back to
wall-clock time or extend the deadline to make an old operation fit.

The Store obtains a clock sample through a caller-supplied callback while holding
the SQLite writer lock. This includes time spent waiting for that lock, unlike a
timestamp sampled before the claim call. The clock domain and readings remain
trusted runtime attestations; the database does not authenticate hardware clocks.
Clock regression or ambiguous continuity refuses dispatch. The callback must be
fast, nonblocking, and must not perform database or network I/O.

A successful admission is not a hard real-time guarantee. Database commit,
process scheduling and external I/O occur after the checked instant. The caller
must recheck the original deadline immediately before I/O, send at most once after
its own successful claim, and never interpret a saved audit receipt as permission
to resend. If work stops after a committed claim, its potentially-sent hold remains
until an actual measured response reconciles it.

## Calibration is a constraint, not a promise

Persist the exact runtime and calibration identity with an explicit safety margin.
Identity must cover model and revision, tokenizer and prompt contract, runtime,
hardware, threads, decoding settings and cold/warm readiness. A warm measurement
must not authorize a cold worker. Expired calibration, mismatched profiles, unknown
readiness or requests outside the measured input/output envelope refuse dispatch.
Calibration validity is checked at claim time (`now < expires_at_ms`), not at the
estimated completion time. The estimated finish must still be strictly before the
original operation deadline. A validity interval is not a completion guarantee.

Use all relevant samples, including cold or slow ones. A measured maximum plus a
margin is an empirical allowance, not a statistical reliability bound or guaranteed
completion time. The earlier 179-token initial-request calibration does not cover
267-token feedback requests. Functional tests and synthetic timing profiles do not
supply that missing live evidence. The historical calibration pools also lack an
explicit readiness label and mix first-after-load with later calls; they are not
automatically valid cold or warm deployment profiles. The operation-bound profile
is immutable: a readiness change after restart must refuse, not silently replace
the original binding.

## Atomic claim and accounting

Prepare exact request bytes and token counts before reserving capacity. At the
send boundary, compare the prepared request and quote with the frozen reservation,
recheck evidence and branch eligibility, runtime/calibration coverage, budget and
remaining time, then grant the existing single-use claim atomically. The budget
already includes the staged reservation; do not count that quote twice. Other
outstanding holds still consume capacity.

This retains the trusted local runtime and database-owner boundary. The Rust
transaction checks current evidence and budget; SQL fences provide additional
consistency checks, not independent authentication against an owner who can edit
the database or forge grant/accounting rows directly.

A refused claim is not a send permission and does not fabricate usage or silently
release a reservation. Explicit cancellation can release only proven-unsent work.
A crash after claim is still ambiguous. Actual late responses retain normal
exactly-once settlement accounting; successful admission does not certify a later
response as on time. Historical settlement and current eligibility remain separate.

## Validation scope

Offline tests cover queue delay, cold/warm mismatch, clock discontinuity, restart,
profile expiry and coverage, immutable windows, fresh evidence, concurrent claims,
cancellation, late response accounting and migration. No new paid inference or
held-out model-performance claim is part of this implementation.

## API outline

- `create_timed_recovery(..., &RecoveryTiming)` binds a new operation or checks an
  identical existing binding. It does not retrofit a previously untimed run.
- `recovery_timing(recovery)` reads the durable original window and calibration
  needed by a restarted worker.
- Existing `begin_recovery_attempt` and `reserve_recovery_attempt` allocate and
  prepare work. Timed operations require complete evidence candidate capture.
- `claim_timed_recovery_dispatch(attempt, &RecoveryPreparation, clock)` compares
  prepared bytes, quote and identity, samples the trusted clock under the writer
  lock, and returns a single dispatch permission or an auditable refusal.
- `cancel_recovery_attempt` and measured settlement retain their established
  accounting rules. `NotReserved` is never permission to send.

`RecoveryTiming` contains the clock domain, original start/deadline, runtime,
calibration and margin. `RecoveryCalibration` includes its identity, measured
maximum duration, sample count, covered input/output sizes and validity interval.
The validity timestamps are in the operation's stated clock domain. Missing
runtime or calibration identity in a prepared request fails closed.

Run `cargo run --example deadline_aware_recovery` and `cargo test --all-targets`.
