# Runtime boundary for timed recovery

The timed Store claim is an admission decision, not the external I/O itself.
This runtime helper joins that claim to one local backend invocation, with another
clock check after the claim commits. It performs no automatic transport retry and
does not turn an uncertain send into a cancellable reservation.

## Concrete Linux clock

The Linux clock provider uses `CLOCK_BOOTTIME`, which includes suspended time.
Its domain identifies the machine, boot, current thread's active time namespace
and boot-time offset. It checks identity around the clock sample. Boot identity
alone is insufficient because Linux time namespaces can offset boot-time clocks.
Linux exposes `timens_offsets` on the process-level procfs entry, not the
thread-task entry. The provider reads `/proc/self/timens_offsets` only after
verifying that both the current thread and process leader have matching active
and child time namespaces. It refuses unavailable or ambiguous metadata.

A normal process restart in the same supported clock domain can resume the
original persisted window. Reboot, host/namespace changes, unavailable metadata,
or clock regression cannot silently reset that window. There is no wall-clock,
process-local timer, or coarse uptime fallback. Unsupported platforms fail closed.
The provider trusts the kernel and local system metadata; it does not authenticate
a hostile host or guarantee detection of virtual-machine snapshot rollback or
cloning. Namespace inode reuse is not independently authenticated.
Actual host identifiers must not be copied into public fixtures or reports.

These clock semantics follow the Linux [time namespace documentation](https://kernel.googlesource.com/pub/scm/docs/man-pages/man-pages/+/master/man/man7/time_namespaces.7).

## One invocation, not exactly-once remote delivery

The wrapper obtains the original durable timing binding and attempts the timed
single-use claim. A refusal or an already-claimed attempt never invokes the
backend. After a successful claim, it samples time again and requires the same
clock domain, a nonregressing reading, unexpired calibration, and enough time
for the same measured maximum plus margin to finish strictly before the original
deadline. Only then does it invoke its one-shot backend callback with the frozen
request. No unrelated preparation belongs between that check and the callback.

Metadata verification after the underlying clock sample, scheduling, or network
delays can still occur after the checked instant. The helper does not
guarantee a remote side effect occurs exactly once or completes before the
deadline. The backend must obey its own transport contract and must not secretly
retry or send more than once.

If the final check fails after claim, the backend is not invoked, but the Store
keeps its conservative potentially-sent hold. This intentionally preserves the
existing crash-after-claim contract. No synthetic zero-usage delivery is created.
Backend errors or panics likewise do not authorize release or resend.

## Validation and settlement

The backend's returned measured reply explicitly distinguishes valid from invalid
output and includes actual token usage. Application validation and token
measurement remain trusted caller responsibilities. Returning a valid reply is
not proof of current evidence eligibility; Store settlement still checks its
captured view and branch.

The wrapper reconciles known replies through existing settlement APIs. A failure
to settle must preserve the returned reply for later reconciliation rather than
losing its measurement or rerunning the backend. Late responses retain ordinary
exactly-once accounting; neither admission nor settlement implies timely receipt.

## Offline scope

Tests use controlled clocks and backends to exercise postclaim expiry, changed
clock domains, regressions, callback failures/panics, duplicate invocations,
concurrent workers and settlement failures. Linux metadata parsing and identity
checks are tested with fixtures; an unavailable concrete clock is reported as
unavailable, not presented as a successful real-clock test.

No model weights, paid inference, new timing calibration, or model-performance
claim are part of this milestone. Existing synthetic calibration profiles remain
synthetic and must not authorize real model work.

## Entry points

`LinuxBootClock::sample()` returns a concrete clock sample or an explicit error.
`Store::run_timed_recovery_linux` uses that provider for both claim and final
checks. `Store::run_timed_recovery` accepts a trusted custom clock callback for
other supported runtimes and deterministic tests. The backend callback returns a
`MeasuredRecoveryReply` with `ReplyValidation::Valid` or a versioned invalid-output
reason. Results distinguish no claim, refused admission, claimed-but-not-invoked,
backend failure, settlement success and settlement failure with retained reply.

Run `cargo run --example runtime_recovery` for the zero-model synthetic example
and `cargo test --all-targets` for the full suite. The concrete-clock subprocess
smoke test prints an explicit unavailable outcome when required metadata is
missing; this does not weaken the provider into a fallback clock. Ubuntu CI sets
`KAIROTENSOR_REQUIRE_LINUX_CLOCK=1`, making unavailability a test failure rather
than a skipped supported-host check.
