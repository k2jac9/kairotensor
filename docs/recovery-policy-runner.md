# Collecting recovery-policy trajectories

The Store-backed `recovery_policy_eval` example collects one affordable-retry
trajectory per case. It emits an exact-context paired replay for always-stop,
affordable-retry and evidence-gated retry. These are **policy replays of shared
observed trajectories**, not three independently executed policy arms.

## Run locally

```sh
cargo run --example recovery_policy_eval -- --trace /tmp/recovery-run/trace.json
python3 scripts/recovery_policy_eval.py /tmp/recovery-run/trace.json
```

The default mode uses deterministic byte-token answers and actual host timings.
It tests the collector, durable Store and evaluator together; it is not a model
accuracy or latency experiment. `new_model_calls` is zero. Choose a fresh trace
path for every run. The collector refuses existing output destinations and never
resends a previous uncertain attempt automatically.

The output bundle contains:

- `trace.json`: the versioned policy-evaluator input.
- `trace.json.raw.json`: corpus/configuration provenance and raw execution data,
  including partial failure information.
- `trace.json.sqlite`: the durable ledger containing recovery IDs, captured
  evidence, reservation decisions, send claims and deliveries.

Keep the whole bundle together. A trace without its raw evidence and ledger is
not a complete audit trail. Schema v2 allows both retry cells to be `null` when
the initial outcome cannot legally lead to a retry. This represents an unprepared
request, not a fictitious zero-token quote. Schema v1 fixtures remain supported.

## Frozen experiment settings

The [versioned corpus](../examples/recovery_policy_eval/corpus-v1.json),
`ci-report-recovery-v1`, contains ten cases:
four calibration cases and six held-out cases, with disjoint report families.
Calibration families are `cal-basic` and `cal-contrast`; held-out families are
`held-boundary`, `held-format` and `held-conflict`, with two cases per family.
Cases cover first-pass success, invalid-output repair, retry exhaustion, numeric
boundaries, exact release/count copying and newly admitted conflicting reports.
Each case has an independent reference verdict (or no admissible verdict for
contradictory evidence). Reference labels and split names
are scorer metadata, not model instructions. The corpus and prompts are stored
with the raw artifact so later source edits cannot silently change a recorded run.
Scripted fixture outputs are used only in deterministic mode, never supplied as
model instructions or forced failures in live mode. Conflicting retry evidence is
introduced only if the initial response actually unlocks a retry; a live first-pass
success skips it. Thus a live run may not exercise every scripted recovery path.

Each case has a fresh 4,096-token budget, a 96-token output cap, and a 60,000 ms
end-to-end deadline. A logical operation may make at most two model calls, so the
full corpus makes at most twenty. Every changed feedback/evidence request is
quoted afresh. All gates run before generation. The runtime claims a durable send
slot before I/O; unknown sends retain their worst-case reservation.

The deadline is soft: admission and receipt are checked, but a running model call
is not forcibly interrupted. A late reply remains charged. Process startup/model
loading are outside the per-case window. Measured durations include preparation,
Store operations, generation, validation and settlement within that window.
The hosted job timeout is a separate safety limit, not a model-latency result.

## Optional live collection

With the pinned runtime from the existing output-cap experiment installed:

```sh
cargo run --release --example recovery_policy_eval -- --live --trace /tmp/recovery-live/trace.json
python3 scripts/recovery_policy_eval.py /tmp/recovery-live/trace.json
```

Live mode uses Qwen2.5-1.5B-Instruct revision
`989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, the pinned tokenizer/template and greedy
CPU decoding through the existing prepared-request adapter. Do not substitute a
model or change settings after examining held-out outcomes while presenting the
result as this frozen experiment. A change creates a new experiment version.

The manual **Collect recovery policy traces** workflow defaults to fixture mode.
Its `live` switch explicitly enables model installation and inference. The completed
pilots' one-shot push triggers have been removed; ordinary pushes cannot launch inference.
The workflow defaults to v2 calibration only, with live inference disabled. It has no
push or pull-request trigger. Private-repository Actions minutes may incur cost;
check the available allowance or obtain approval before starting a live run.
The workflow preserves the bundle and execution-environment description as a
30-day artifact, including available partial files on failure.

## Interpretation and limits

A complete live run measures the actual collected trajectories. The three policy
summaries remain matched replays of those trajectories, with all cases retained
in the denominator. Invalid and late replies cost tokens. Unknown usage is not
zero usage; efficiency is unavailable while holds remain. Evidence consistency
and model correctness are distinct, and the consistency gate does not bypass the
Store's eligibility checks. The collector normalizes invalid-but-stale deliveries
using their separately persisted applicability and stops that trajectory. This is
stricter than the core API, which permits one fresh recapture after invalid output
even when evidence changed during generation. The existing corrected-evidence
recovery behavior is preserved.

The synthetic run makes no claim that a model would produce the scripted answers.
The offline scorer checks internal consistency but cannot authenticate producer
labels or model identity. A small synthetic-report corpus is not a production
reliability guarantee. Report collection mode, corpus version, exact source commit,
complete/partial status and raw artifacts alongside every result. See the
[policy protocol](recovery-policy-protocol.md) for the frozen selection and scoring
rules. No live result is established merely by adding or testing this runner. The completed
[v1 pilot](evaluations/recovery-policy-v1-2026-11-14.md) failed all held-out cases
under the frozen contract; the [v2 pilot](evaluations/recovery-policy-v2-2026-11-14.md) also failed the frozen contract.


## Version 2 and calibration isolation

Version 1 remains the CLI default for reproducibility. Select v2 explicitly:

```sh
cargo run --example recovery_policy_eval -- --corpus-version v2 --split calibration --trace /tmp/recovery-v2-cal/trace.json
cargo run --example recovery_policy_eval -- --corpus-version v2 --split all --trace /tmp/recovery-v2-full/trace.json
python3 scripts/recovery_policy_eval.py /tmp/recovery-v2-full/trace.json
```

Calibration mode collects only the four calibration cases and exports the raw
manifest and SQLite ledger. It does **not** emit a complete policy trace or a
held-out score. Its manifest includes only selected cases. This prevents an
accidental calibration pass from executing the held-out set. The full mode
contains the four calibration cases plus six fresh held-out inputs; old held-out
outcomes are not reused as a new test set.

The v2 prompt explicitly requires one raw JSON object without Markdown fences,
three string fields, numerical counts separated by `/`, and `pass` only when
passed equals total. The validator and scoring rules remain unchanged. Prompt
wording was fixed from calibration observations before preparing fresh held-out
cases. The [prompt freeze](../examples/recovery_policy_eval/prompt-v2-freeze.json)
and [corpus freeze](../examples/recovery_policy_eval/corpus-v2-freeze.json) record
the source hashes and ordering. Held-out source remains visible in the repository;
this is execution isolation, not access-controlled blinding. Deterministic tests establish mechanics, not whether the wording works
better for Qwen. The new cases are fresh instances, not an independent external
benchmark or a guarantee against broader task-family overlap.

The explicitly approved v2 run has completed; see its linked report. Further
live runs need appropriate approval. A full run remains bounded by ten cases,
twenty model calls and a 30-minute hosted-job timeout. At the previously checked
Linux list rate, that timeout corresponds to about US$0.18 nominal compute plus
artifact storage. A calibration-only run is capped at eight calls but retains
the same job timeout; actual duration and billing cannot be guaranteed. Normal
PR CI is separate. Obtain approval for the chosen live scope before launching.
Because this workflow is still only on the unmerged PR branch, manual dispatch
may require default-branch registration. Do not merge or re-arm a push trigger
just to run it without the required authorization.

## Read-only bundle audit

After extracting a trusted run artifact into a fresh directory, run:

```sh
python3 scripts/audit_recovery_bundle.py /path/to/extracted-bundle
```

The auditor opens SQLite read-only and reconciles exact captured requests,
quotes, raw replies, reference validation, timings, charged usage and reservation
snapshots with the paired trace. It performs no inference and does not modify
input files. Complete supported bundles receive `verified_complete`; raw-only
calibration, incomplete or unresolved bundles are explicitly not scored. Unknown
or tampered event paths fail closed. This checks internal consistency, not the
authenticity of model, source or hardware attestations. Preserve the original
artifact hash and source commit alongside the bundle.

Future collection steps have a 25-minute timeout under the 30-minute job limit.
This leaves best-effort time to upload partial artifacts on a collection timeout,
but does not guarantee upload if setup/build consumes more than five minutes.
The completed v1/v2 runs used the original job-level timeout only.
