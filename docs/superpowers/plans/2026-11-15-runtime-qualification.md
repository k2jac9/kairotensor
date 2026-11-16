# Project runtime qualification implementation plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`. This is a separate bounded successor to A0;
> do not rewrite its historical verification receipt or host runtime defaults.

**Goal:** Qualify exact current stable, upcoming and previous project runtimes,
then pin the accepted project launch/CI identities and retain a working rollback.
**Architecture:** Isolated runtime lanes run the same locked fixture suite against
one integrated source; only independently passing stable is the normal target.
**Tech Stack:** Rust/Cargo, Python, existing GitHub Actions and storage tooling.
**Spec:** `/home/neo/Projects/LOCAL_CI_RUNTIME_POLICY.md` and the A0
`docs/runtime-adoption.md` findings. Run after A-E1 to avoid repeated full matrices.

## Global Constraints

- No host-wide default replacement, service deployment, dependency bulk upgrade,
  old runtime removal, data deletion or model inference.
- Preserve original CI gates and the exact accepted source/runtime/lock artifacts.
- Use current primary upstream records; floating channel names are lookup inputs,
  never final identities. Unknown/unavailable lanes remain explicit.
- Admission budgets cover downloads, unpacking and compilation. A refusal blocks
  the heavy lane, not lightweight review. Never extend the temporary exception.

## Review Focus

1. A recorded pin must resolve to the executable actually running the tests.
2. A passing stable lane cannot conceal a failing or unexecuted preview lane.
3. Older consumers and the exact rollback set remain distinct from previous stable.
4. The keyed Cargo output must use the selected runtime and unchanged lock.
5. Switching back to the retained runtime/source must actually reproduce a basic
   current read and accounting fixture; stored backups alone are not a rehearsal.

### Task 1: Qualify isolated lanes and document actual outcomes

**Files:** modify `docs/runtime-adoption.md`; create
`scripts/verify_runtime_matrix.py`, `scripts/test_verify_runtime_matrix.py`.
Toolchain downloads/installations are allowed only into user-owned isolated
runtime directories/toolchain slots with a declared output lifecycle. No default
rustup toolchain change and no write to `/etc` or system packages.

Initial candidate observations are Rust1.100.0, upcoming nightly2026-11-15
(and separately recorded beta1.101.0-beta.2), previous1.99.1; Python3.15.2,
preview3.16.0a2 and previous3.14.10. Refresh official release/manifest records at
intake and retain bytes/digests before choosing. The normal accepted baseline
remains A0's exact1.99.1/3.15.2 until the stable candidate passes.

The verifier accepts an explicit JSON manifest containing source commit, lock
digest and absolute Rust/Cargo/Python executable paths plus expected identities
and binary/archive hashes per lane. It never installs tools or searches mutable
PATH for a replacement. Before each lane it verifies exact identities, runs
locked all-target Rust tests, existing Python suites and compiled-runner checks,
then emits command/exit/count/skip and artifact receipts. Unknown prerequisites
produce Unexecuted, never Pass. Tests use tiny local fake executables for manifest
rejection/command identity only; real qualification uses the real full suites.

- [ ] Add failing tests for missing lane, wrong executable identity/digest,
  changed lock, nonzero subprocess exit, missing receipt and false pass on skip.
- [ ] Implement bounded verification and artifact receipts. Install missing
  candidates only after storage admission; verify archive hashes before use.
- [ ] Run each available lane serially on one source. Preserve failures; do not
  alter tests or dependency pins to make a lane pass. Scope any genuine migration
  fix separately with a reproduced regression and review.
- [ ] Rehearse the retained baseline in an isolated checkout with original lock
  and fixture database, checking current read and unchanged uncertain hold.
- [ ] Document exact executed/skipped lanes, upstream source responses (including
  Python and previous-line patch freshness), NixOS host build display, supported
  consumer inventory limits and rollback receipt; focused review and commit.

**Done:** actual runtime qualification evidence is available. Unavailable external
toolchains remain an explicit adoption gap; they do not become an invented pass.

### Task 2: Pin qualified project defaults and CI lanes

**Files:** create `rust-toolchain.toml`, `.python-version`,
`scripts/project_runtime.py`, `scripts/test_project_runtime.py`;
modify `.github/workflows/check.yml`, `docs/runtime-adoption.md`.
No application source or dependency resolution change.

The project launcher uses an explicit accepted local runtime-selection manifest
with absolute executable paths, hashes and versions, distinct from the immutable
source-bound qualification receipt. Verify all identities before launch, bind
selected Rust/Cargo and child python3 on a private PATH, and route Cargo through
the approved storage wrapper. Ordinary editable source is recorded honestly and
does not inherit the old source's qualification. Never silently install/fallback,
change host defaults or commit workstation paths as portable CI identities.
The system Rust commands are rustup proxies and may honor pins; test actual
resolution instead of assuming they override them. Python child selection still
requires explicit verification. Document the durable project invocation.
Meaningful launcher tests cover absent/wrong identity, selected child Python,
editable source without false qualification, and admitted Cargo routing.

- [ ] If stable qualification passed, pin its exact Rust release in
  rust-toolchain.toml with the minimal required components, and exact Python
  stable patch in .python-version. Otherwise preserve the accepted baseline and
  report the failed candidate without promoting it.
- [ ] Pin setup actions to verified upstream commits and exact runtime versions;
  preserve checkout's accepted pin and every existing CI command. Define named
  stable/upcoming/previous roles with explicit identities; include a retained
  baseline lane if it is distinct. Do not claim hosted execution from YAML.
- [ ] Verify project-selected tools match qualified identities, then run the
  exact stable fixture workload once after pinning. Validate CI configuration
  and preserve matrix failures/skips honestly; no shared-branch push in this task.
- [ ] Independent review and commit. Retain old binaries, lock and recovery
  evidence; no runtime retirement is authorized.

**Done:** project defaults reflect the accepted current stable candidate and CI
describes exact lanes. Local evidence and eventual hosted execution remain separate.
