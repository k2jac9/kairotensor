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
