# Separate-process decision replay

`decision_replay` exports one fixed, offline incident fixture through the actual
Store export API. It uses the existing incident adapter, canned response bytes,
synthetic timing and fixture-byte accounting. This exercises request capture,
manifest/runtime binding and format validation; it makes no model-inference,
semantic-correctness or measured-calibration claim.

```sh
cargo run --locked --offline --example decision_replay -- export-fixture /path/new-bundle.json
cargo run --locked --offline --example decision_replay -- replay /path/new-bundle.json
python3 scripts/audit_recovery_bundle.py --decision-v1 /path/new-bundle.json
```

Use the workstation storage/Cargo wrappers and admitted output profile for local
builds. The two Rust commands run separate processes. Both print the recorded
replay report, so their source identities and results can be compared. Fixture
export uses `CurrentRead` and `DenyDiagnostics`, accepts no owner database path,
and creates the output exclusively. It writes the complete canonical payload
using `DecisionBundleV1::to_json()`, not the tagged `BundleExport` wrapper.
Existing files and symbolic links refuse. A failed export can leave its newly
created empty/partial file as a failure artifact; it is never silently replaced.
Only the literal `export-fixture PATH` and `replay PATH` modes exist.

The replay branch opens its input read-only, checks metadata and reads at most
16 MiB plus one sentinel byte before parsing. The pure codec additionally bounds
nesting, attempt count and decoded envelopes. Replay does not initialize a Store,
run migrations, create an adapter, or invoke any backend callback. Its only
output is the JSON report on stdout. Malformed, oversized and redacted inputs
refuse without modifying the input. A valid bundle can successfully produce an
`incomplete`, `mismatch` or `not_reproducible` report; process success means the
report was produced, not that every contract matched. Unknown validators remain
unavailable. Historical acceptance stays recorded metadata and historical timing
always stays unavailable. See [the exact v1 schema](decision-bundle-v1.md).

The installed `strace` can observe the compiled executable directly:

```sh
strace -f -e trace=process,network -o replay.trace /absolute/path/to/decision_replay replay /path/new-bundle.json
```

The C3 retained trace observes the exercised compiled replay path, including
process and network syscalls. It does not trace Cargo compilation. Absence of
child-process/network calls in one run is bounded observational evidence; it is
not a universal proof for other paths or a detector of an in-memory Store.
The source boundary supplies the separate evidence for Store initialization.

## Report version 2

The executable emits `report_schema: "decision-replay-report/2"`. `accounting`
contains totals only for known compatible units; `unknown_accounting` preserves
ordered `{attempt_id, charged, held}` observations without cross-attempt addition.
Empty unknown lists are explicit. Missing unit requirements remain visible.
Invalid bundles refuse in this CLI; the direct replay API returns a bounded
mismatch with empty accounting lists. Per-record ledger checks and known-unit
aggregate overflow checks remain enforced.

This versions the report, not DecisionBundleV1. Existing bundle bytes and digests
remain valid; no ledger history or disposition is rewritten. Consumers must use
the report schema to distinguish these semantics from the historical unversioned
report, whose unknown totals were invalid dimensional aggregates. The repository
runner serializes the report generically; the Python reader consumes bundle bytes,
not this report. The local consumer audit found no other report-shape consumer
requiring migration. External consumers were not inventoried.

The exact integrated-review bundles (charges 10/20 and `i64::MAX`/1) are retained
and replayed by the new S3 binary under `final-fix/replay-probes/`; their original
input hashes are checked before and after the separate processes. Old review
outputs remain historical evidence beside the original inputs.

## Python reader and E1 helper boundary

`scripts/decision_bundle_v1.py` uses only the Python standard library. It performs
bounded reads, strict UTF-8 JSON parsing, duplicate-key and depth checks, the v1
top-level shape check, lowercase hex/length/SHA-256 checks for byte envelopes,
and the documented independent canonical outer hash. It deliberately does not
implement the complete Rust wire/ledger validator, contract renderers, validator
execution, authenticated access checks, historical timing or policy/cost scoring.
An integrity success is not a statement of ledger validity or actual availability.

Public helpers are `read_bundle(path)` and `parse_bundle(raw_bytes)` for an
integrity-checked recorded dictionary, `decode_envelope(envelope)` for exact
binary bytes, `canonical_bytes(value)` for the documented sorted UTF-8 encoding,
`inspect_bundle(bundle)` for explicit integrity-only facts, and `audit_file(path)`
for those facts plus the separate transport-file SHA-256. `parse_bundle` accepts
only the complete payload; redacted and complete wrappers are not auto-unwrapped.
Unknown schema versions refuse; unknown validator names are retained as declared
names and are never treated as executable validators. Arbitrary binary payloads
are decoded as bytes, without text normalization. Public encoding/inspection
helpers accept in-memory values from trusted bounded callers; file consumers
must use `read_bundle`/`audit_file` to enforce the transport bound before allocation.

For E1, `attempts`, `inputs`, their ordered `role` strings, `evidence` and envelopes
remain available verbatim. E1 must establish role and contract applicability,
including exact incident manifest/runtime bindings, before choosing an ablation.
A role string or historical Accepted receipt confers no access or applicability.
Python's hash check must not be presented as Rust validation. To obtain supported
recorded reconstruction/validation, invoke the verified Rust replay executable
and retain its source digest and availability report alongside the input.

The legacy auditor still accepts only an explicit directory in its original
mode. The separate `--decision-v1 FILE` branch never opens SQLite. Combining
both modes refuses; a file is never silently auto-detected as a new mode.

Subprocess tests require explicitly bound precompiled executables and never
invoke Cargo or install dependencies:

```sh
KAIROTENSOR_DECISION_REPLAY=/absolute/path/to/decision_replay \
KAIROTENSOR_RECOVERY_RUNNER=/absolute/path/to/recovery_policy_eval \
python3 -m unittest discover -s scripts -p 'test_*.py'
```

CI builds both before discovery and runs offline export/replay commands while
retaining the prior reference, archived-data, recovery and incident gates.
Configured CI is not evidence of hosted execution or runtime-matrix acceptance.
