# Evidence-backed deterministic verdict

This route computes a CI report verdict from admitted Store evidence, without a
model, tokenizer, dispatch, or token reservation. It is an application contract,
not a model-accuracy improvement or a live inference benchmark.

## Contract

The requested candidate manifest must contain exactly one observation occurrence.
That occurrence must be admitted for the authenticated actor and scope. Missing
and unauthorized evidence have the same generic refusal. A list with one usable
report plus a missing or unauthorized report is not a complete input: it is
refused, not reduced to its surviving member. Multiple reports, including repeated
IDs or conflicting reports, are refused rather than selected or reconciled.

The captured payload must be one strict JSON object with exactly `release`,
`passed`, and `total`. Release is a nonempty Unicode string. Counts are unsigned
64-bit integers, not booleans or fractional numbers, with `0 <= passed <= total`
and `total > 0`. The Rust unsigned-integer parser also refuses JSON `-0`;
the historical Python baseline accepts it as zero. Duplicate keys, missing/extra
fields and malformed bytes are refused. The result copies the release, formats `passed/total`, and returns `pass`
only when the counts are equal; otherwise it returns `fail`.

## Publication and reads

`begin_report_verdict(actor, scope, branch, candidates)` captures admitted bytes
and binds the deterministic result to an attempt. `commit_report_verdict(attempt)`
publishes the persisted result only if the captured view and expected branch
parent are still current. These are separate transaction boundaries so a source
change or competing publication between them is detected. The binding survives
restart. Callers cannot replace the output with their own verdict through this
route.

A repeated commit returns the original settlement disposition. That is a
historical receipt, **not a fresh authorization to use the answer**. After commit,
use `read_current(actor, scope, branch)` to obtain bytes eligible at that read's
snapshot. Revocation can make a previously accepted result need review. A caller
must not treat its old receipt or cached output as proof of current eligibility.
The existing caller-authenticated actor/scope and trusted local database boundary
remain unchanged. The binding protects these attempt IDs; it does not lock a
branch against a trusted caller starting other opaque computations. Previously
delivered bytes cannot be recalled.

Accepted deterministic deliveries have zero model-token usage. This does not mean
CPU work or storage is free. The route does not settle, cancel, release, or reuse a
model recovery reservation; uncertain sends retain their original holds. Existing
subset selection through `begin_from_candidates` keeps its documented behavior.

## Demonstration and verification

Run `cargo run --example deterministic_verdict` and `cargo test --all-targets`.
The example is deterministic and requires no model weights or network inference.
Tests exercise complete versus omitted candidates, strict payload parsing,
capture/commit changes, duplicate commits, restart, competing writers, historical
receipts after revocation, and preservation of model charges and uncertain holds.

The standalone Python `scripts/recovery_verdict.py` remains an offline baseline
with a caller-supplied admission flag. It does not become an authenticated Store
API merely because it produces the same arithmetic result. Historical calibration
archives, frozen validators and pilot scores remain unchanged.
