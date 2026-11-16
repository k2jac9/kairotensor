# Hosted integration verification

The first PR #20 run at `e43677d` reached the full Python scripts suite after
passing Rust and example execution. It exposed two test-environment assumptions:

- The launcher fixture inherited `LD_LIBRARY_PATH` and `PKG_CONFIG_PATH` from
  GitHub's Python setup. Its positive tests therefore stopped at the production
  refusal boundary before reaching the behavior they intended to test. Fixture
  setup now excludes those ambient values. An explicit subprocess regression
  verifies the production launcher still refuses both and performs no workload
  side effect. Production runtime selection is unchanged.
- A legacy late-receipt test depended on a 130 ms scheduling interval. The
  deterministic-clock correction already present in PR #18 is included here,
  retaining real child pipes, receipt parsing, settlement, and separate wall-clock
  watchdog tests. A narrow procfs teardown-race fix is shared too.

Before correction the inherited-path case failed locally with the same refusal.
Afterward, all 46 focused runtime/controller tests passed with those paths
present in the parent environment, and the full 377-test scripts suite passed
with the previously verified Rust fixture binaries. This is local evidence;
exact-head Actions results remain the hosted acceptance gate.

The run also reported checkout v4's deprecated Node 20 runtime. On 16 November
2026, the authoritative actions/checkout release list identified v7.0.1 as
latest stable and v6.1.0 as the newest release of the previous major. The
maintained workflows now pin v7.0.1 to
`3d3c42e5aac5ba805825da76410c181273ba90b1`, preserving `contents: read` and
`persist-credentials: false`. A small separate job verifies the previous-major
checkout at `d23441a48e516b6c34aea4fa41551a30e30af803`. The published v6 beta is
older than both stable lines; no upcoming-release preview was available in the
58-entry release inventory. Its absence is recorded rather than substituting
an old beta. Upstream gives no formal support promise for the previous line.

The exact prior checkout remains recoverable in `e43677d`; no history was
rewritten. The new action uses Node 24 and its safer fork rules do not require
an opt-out for these push, pull_request and workflow_dispatch triggers. No
live recovery workflow is dispatched by this maintenance change. The recovery
workflow's changed checkout still needs its own future execution evidence.

Source-bound logs and upstream selection responses are retained under the
project artifact packet `submission-finalization-mkpl39bc/ci-fix/`.

The combined PR #18 Python-previous lane at `10f618c` exposed another startup
assumption: the prepared-worker hang test spent its two-second global budget
before claiming a slot. The supervisor correctly killed the child with no
claimed work and no hold; the test incorrectly expected a post-claim hold.
The load-failure check is now separate. The hang test waits for a real durable
claim within the usual bounded fixture startup window, then advances only the
supervising test process's clock to the unchanged manifest deadline. The real
child, worker, pipes, ledger and process-group termination remain in use. It
requires exactly one claim, the 269-unit hold, no receipts or settlement, zero
model calls, and a timed-out, reaped child killed by the supervisor.

A retained reproduction delays the real child's readiness by 2.1 seconds:
the original test fails with the hosted zero-claim result, and the corrected
test passes with that same delay. All 14 prepared-controller tests and all
438 Python scripts tests passed locally using the accepted Python runtime and
existing verified Rust fixture binaries. Production code and runtime guards
are unchanged. Evidence is retained in
`submission-finalization-mkpl39bc/pr18/integrated/fixture-fix/`; exact-head hosted
checks remain required. The injected supervisor clock is deterministic test
evidence, not a real-model timing measurement.

The PR #18 Python-previous run at `3c4a386` then exposed a separate hosted-runner
fixture assumption. Its crash/hang/partial test allowed only 200 ms for durable
setup and execution; expiry before the claim correctly retained zero hold, so
the intended partial-response path was never reached. A 250 ms delay after the
real request write reproduces that failure and is retained with the original
hosted log.

Hosted-runner protocol and audit fixtures now use a bounded 30-second startup
allowance. Crash and partial-response tests require one actual claim, the full
400-unit synthetic hold, the expected child exit, and the exact retained partial
body length. The separate hang test waits for the real forked child to enter
its hanging send path before advancing only the parent's test clock to expiry;
the real watchdog must kill and reap it. Dedicated deterministic tests still
exercise expiry both before and after the claim, forbid a fork at either
boundary, and require respectively zero hold or the full 400-unit hold.

All 15 hosted-runner tests passed both normally and with the injected setup
delay. The full 440-test Python scripts suite passed with the accepted runtime
and verified existing Rust binaries. This correction changes only PR #18 tests
and this record; production deadlines, claims, holds and live authority are
unchanged. Evidence is retained in
`submission-finalization-mkpl39bc/pr18/integrated/hosted-fixture-fix/`.
Exact-head hosted acceptance remains required; these offline process fixtures
do not establish provider behavior or billing.
