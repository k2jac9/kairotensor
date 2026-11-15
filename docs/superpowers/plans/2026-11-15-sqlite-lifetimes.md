# SQLite connection lifetime repair

> Execute with superpowers:subagent-driven-development, test-first and independent review.

**Goal:** close owned Python SQLite connections on every exit without changing
audit decisions, transaction behavior, stored bytes, sends or held resources.
**Architecture:** retain current transaction contexts; add deterministic resource
ownership around them. No new dependency, service, schema or model call.
**Spec:** this document and the proven diagnosis in
`/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/warning-diagnosis/REPORT.md`.

## Global Constraints

- Preserve transaction commit/rollback behavior and original raised exceptions.
- Never suppress ResourceWarning, weaken audit checks, modify retained bundles,
  release uncertain holds, execute inference or connect to Rocket.
- A connection returned successfully transfers ownership to the caller; a setup
  failure must close it. Source evidence and rejected fixture copies remain retained.
- Use existing admitted fixture runner; no rebuild is needed for Python changes.
  Any heavy work uses the project storage wrappers and bounded profile.

### Task 1: Deterministic connection ownership and exception regression tests

**Allowed files:** `scripts/audit_recovery_bundle.py`,
`scripts/incident_calibration_controller.py`,
`scripts/test_audit_recovery_bundle.py`,
`scripts/test_recovery_policy_runner.py`, and create
`scripts/test_sqlite_lifetimes.py`. No other production or test files.

- [ ] Add failing focused lifecycle tests for successful, incomplete and rejected
  archived audits, proving owned handles close without changing results/errors
  or bundle file hashes. Check actual connection closure as well as collected
  ResourceWarnings after forced GC; warning absence alone is insufficient.
- [ ] Cover the test helper's normal and exceptional contexts, retaining its
  transaction behavior. Repair directly owned fixture connections in the two
  allowed existing test files so assertions/exceptions cannot bypass close.
- [ ] Reproduce `_connect` PRAGMA failure on malformed SQLite and assert the
  original error survives while the opened connection closes. Also use bounded
  fault injection to establish and repair `_initialize` failure after connection
  transfer and `audit_run` snapshot query failure. Preserve success ownership,
  commit/rollback, incomplete audit behavior and all ledger/hold decisions.
- [ ] Implement minimal closing/finally ownership; do not replace a transactional
  context with a closing-only context where commit/rollback semantics are needed.
- [ ] Run new focused tests and the auditor/controller/runner tests with the
  precompiled fixture runner set. Run the full script suite once at the final
  candidate. Record interpreter, source hashes, runner identity, passes, skips,
  residual warnings, exact commands and artifact digests. No warning filtering.
- [ ] Self-review and commit `fix: close SQLite audit and controller connections`.

**Done:** reproduced owned-handle leaks are closed on success/early return/error,
and offline audit/accounting behavior remains verified. Residual warnings outside
the allowed scope are reported honestly, not treated as fixed.
