# Reproduce the archived checks

These steps inspect the submitted evidence without model inference. Use Python
3.11 or newer. The source snapshot has no third-party Python requirements for
these specific summary and audit commands.

## 1. Open the pinned source

From the extracted reviewer package, verify the manifest and unpack the source:

```sh
sha256sum -c SHA256SUMS
python3 -m zipfile -e kairotensor-source-20fab84.zip source
cd source
```

The archive places repository files directly in `source/`. If reviewing through Git instead,
use a separate checkout of `20fab84a6baa525452191ecf80eeee339f0b5ac7`.
Do not reset a working checkout that contains local work.

## 2. Recompute the benchmark tables

```sh
python3 scripts/summarize_revision_bench.py docs/benchmarks/atomic-revision-2026-11-13.csv
python3 scripts/summarize_revision_bench.py docs/benchmarks/bulk-setup-2026-11-13.csv
```

Compare with the `verification/atomic-summary.md` and `bulk-summary.md` files in
the reviewer package. Ratios are medians of matched per-trial ratios, not ratios
of medians. A lighter full-scan contract is not an equally durable baseline.

## 3. Audit the frozen recovery records

Choose a new temporary directory so existing evidence cannot be overwritten:

```sh
recovery_audit_dir=$(mktemp -d)
python3 -m zipfile -e docs/evaluations/recovery-policy-v2-2026-11-14.zip "$recovery_audit_dir"
python3 scripts/audit_recovery_bundle.py "$recovery_audit_dir"
```

Expected: `verified_complete`, mode `live_qwen`, 10 cases, 20 recorded backend
calls, 5,241 charged tokens and zero outstanding tokens. All held-out policy
success counts are zero. The audit field `new_model_calls: 20` describes the
historical collection recorded in the bundle. Running this audit creates **zero**
new model calls. The policy replay itself also generates no model calls.

## 4. Run the lightweight contract tests

```sh
python3 -m unittest discover -s reference -p test_memory.py
python3 reference/belief_oracle.py
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Packaging verification on 15 November observed 18 reference passes, 8 oracle
passes, and 203 script tests with 28 skips (175 executed, no failures). Some
script tests require separately built Rust examples and are skipped without them.
Resource warnings about unclosed SQLite connections are retained in the receipts.

The 16 November finalization reran these exact commands after unpacking the
reviewer package, using Python 3.15.2. It observed the same 18 reference passes,
8 oracle passes, and 203 script tests with 28 skips. Two unclosed-SQLite-connection
resource warnings were emitted. The new stdout/stderr and exact runtime identity
are retained under `verification/finalization-20261116/`; the original receipts
remain unchanged. Both benchmark summary outputs matched the earlier receipt
bytes, and the recovery archive remained unchanged after the read-only audit.

The final ZIP adds those fresh receipts to the tested payload. Its manifest and
unpacked source bytes are checked again; the source archive is identical to the
one used for the fresh command run. No new source build or model call is implied.

## Full Rust and model reproduction

The repository README and historical workflows contain full Rust commands.
These require a Rust toolchain, a C compiler and build capacity. On the author's
workstation, storage admission is required before builds. The packaging checks
did not rerun Rust; [exact-main CI](https://github.com/k2jac9/kairotensor/actions/runs/43622988515)
is the recorded Rust evidence.

Live Qwen reproduction is optional and separate: follow
`docs/scoped-model-reuse.md`, `docs/corrected-handoff.md` and the frozen recovery
protocol at its recorded commit, preserving model, tokenizer and runtime pins.
It requires downloading assets and running inference. There is no claim that
the author performed a fresh live replication while preparing this package.
