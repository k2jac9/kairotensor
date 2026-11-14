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
Its `live` switch explicitly enables model installation and inference. It has no
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
rules. No live result is established merely by adding or testing this runner.
