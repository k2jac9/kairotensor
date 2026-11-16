# Example warnings and verification log preservation

The simple handoff and scoped-reuse examples compile the basic adapter. Examples
that prepare a request before reserving usage opt into
`examples/corrected_handoff/prepared_adapter.rs`. Preparation, generation and
discard operations retain their original byte protocol and single-use handles.
Recovery-policy traces retain both adapter source files in their manifest.

The two simple examples deny dead code during compilation, so their former
warning categories now fail the existing Cargo checks if reintroduced. This
cleanup adds no warning suppression and changes no Store or accounting contract.

## Recording new verification runs

`scripts/verify_runtime_matrix.py` requires a fresh directory for each runtime
lane. Its command runner creates stdout and stderr exclusively before starting
the command. An existing file or symlink, including a redirected stdout target,
refuses dispatch without replacing that evidence. Concurrent attempts to use the
same log paths cannot both dispatch. Receipts retain the actual process exit,
elapsed time and hashes of both streams, including failed commands.

If creating the second stream fails, the newly created first stream can remain
empty. Preserve it as an incomplete attempt and use a fresh run directory. Do not
remove files to retry under an old name. A directory owned by one run may update
its aggregate progress receipt; this is not an append-only storage service.

Outer controllers and shell redirection must also use unique run directories and
exclusive writes (`open('xb')` in Python or `set -o noclobber` in Bash). Keep
command output, process exit and digests together. The verifier cannot protect
output files independently opened by an outer shell or executed application.

## Historical evidence gap

One superseded standalone preflight display was overwritten during an earlier
controller run. A bounded search did not recover its bytes. Its original exit
status and SHA-256 remain recorded, as do the original inner command logs and
artifacts. The gap remains a historical preservation defect; later runs do not
reconstruct or replace it.

The original incident and search records remain in the project artifact directory
under `reliability-implementation-20261115/task-g1/`, named
`controller-log-collision.json` and `old-preflight-recovery-search.json`.
Subsequent complete qualification packets are separate evidence. No historical
receipt, runtime selection or accepted rollback asset is changed by this cleanup.

## Verification

Executable source `62787673a61d3f126464a5087036194726d67ec6` was checked locally
on 16 November 2026 UTC using the existing selected runtimes:

- Rust 1.100.0, 1.99.1 and the pinned 1.102.0 nightly each passed all 362
  `cargo test --locked --offline --all-targets` tests with zero compiler warnings.
- Python 3.15.2, 3.14.10 and 3.16.0a2 each passed all 376 script tests, including
  the actual Rust fixture-runner integrations. No tests were skipped.
- The 18 reference and eight oracle tests passed. Seven affected offline example
  runs passed, including invalid-output replay. A fresh recovery trace retained
  the exact bytes of both adapter source files.
- The compile guards failed on the original warnings before the refactor. New
  overwrite tests failed before exclusive creation was implemented and passed
  afterward. Independent review found no actionable issues.

Receipts, stream hashes, failures and review are retained in the project artifact
directory under `maintenance-warnings-9ug0kq4c/`. These results are local offline
regression evidence; hosted CI and live-model execution were not performed.
