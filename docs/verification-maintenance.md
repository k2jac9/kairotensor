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
