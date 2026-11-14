# Controlled recovery calibration lab

This lab prepares a small diagnostic experiment after the failed v1/v2 pilots.
It does not alter their prompts, corpus, worker, validators or frozen scores.
No new live result is implied by adding this code.

## Fixed scope

The plan contains eight cells: the existing `cal-pass` and `cal-repair` cases,
BF16 and FP32, and free versus construction-assisted generation. All cells run
sequentially on the same machine with the pinned Qwen model/runtime and two
threads. Only one dtype's model is resident at a time. There are no model warmup
calls, retries, held-out cases or automatic follow-up runs. Cell order is fixed
and recorded, alternates cases/format and reverses format order between dtypes.
Two samples per profile are not enough for a reliability estimate; load, cold
start, cache and order effects remain confounders.

Run the fixture without installing a model runtime:

```sh
python3 scripts/recovery_calibration.py --output /tmp/recovery-calibration/result.json
python3 -m unittest discover -s scripts -p 'test_recovery_calibration.py'
python3 -m unittest discover -s scripts -p 'test_recovery_admission.py'
```

Use a fresh output path; existing results are never resumed or overwritten.
The manual **Calibrate recovery runtime and output** workflow defaults to fixture
mode. Its `live` input is the only workflow opt-in to model loading/generation;
there is no push or PR inference trigger.

Default execution is an offline fixture with zero model calls. Live execution
requires explicit opt-in and separate bounded-cost approval. A hosted job has a
30-minute limit and a 25-minute collection step, with best-effort time to upload
partial artifacts. At the previously checked Linux rate, the job ceiling is
approximately US$0.18 nominal compute plus storage; separate CI and account
allowances also apply. Neither elapsed time nor those limits establish an actual
invoice. Memory pressure or startup/network failures may produce a partial run.

## What structured generation means here

For exactly one complete admitted report, code copies the release and renders
the numeric test counts, then constructs two canonical JSON candidates differing
only in status (`pass` or `fail`). A tokenizer-prefix trie constrains generation
to these candidates and EOS. The model still chooses status; the correct label
is never used to construct the candidates. Conflicting, incomplete or unadmitted
evidence is refused. Tokenizer round-trip and cap checks fail closed.

This is **construction-assisted output**, not a test of unrestricted JSON ability
or a pure formatting-only intervention. It removes copying/formatting choices
from the model as well as fences. Correctness is still checked against the
unchanged strict reference contract. A syntactically valid but wrong status is
still a failure. This path is a calibration experiment, not an automatic rewrite
of previous replies or a production Store integration.

## Latency-aware admission

The separate pure admission helper compares a fresh full reservation quote to
available budget without releasing existing holds. It requires an exact runtime
profile match and uses the maximum observed calibration duration plus an explicit
margin. It admits only when estimated completion is strictly before the retained
original deadline. Unknown, invalid, mismatched or unaffordable inputs fail closed.

The estimate is a conservative heuristic from a tiny calibration sample, not a
hard runtime bound or a percentile. Callers must preserve the original deadline
and clock domain across retries/restarts; a pure helper cannot independently
prove that callers supplied the same origin. It does not cancel a sent request,
reset clocks, release uncertain reservations or change the existing recovery API.
Lab decisions are recorded simulations/prospective recommendations, not extra
model calls or evidence of production retry performance. These eight cells measure
initial-request profiles. Longer feedback requests outside the observed input-token
envelope are rejected by the helper; they need their own representative calibration
before a production retry policy can use an estimate.

## Reporting

Keep rendered chat, token IDs/hash, effective generation settings, CPU/runtime
identity, dtype, memory observations, call order, load time and per-call timings
with each result. Distinguish fixture from measured output and complete from
partial execution. Compare runtime profiles on the same hardware before selecting
one; do not silently fall back to another dtype. Resolve calibration blockers
before freezing a new held-out evaluation. 
## Preparation checks

The pinned real Qwen tokenizer was checked without model weights or inference:
both calibration prompts contain 179 tokens, and each canonical status candidate
requires 19 tokens including EOS. Exact decode round-trips and all trie prefixes
passed. This validates token construction only; it does not establish live model
correctness, speed, or memory fit. The live workflow explicitly installs the
Jinja2 dependency needed by the chat template.

The hosted artifact staging step preserves at most 4 MiB of payload plus a small
manifest, with one-day retention. Oversized or non-regular files are reported as
omitted rather than silently uploaded. Stage/upload steps have separate one- and
two-minute limits; job cancellation and queue delays still prevent an account-wide
hard billing or wall-clock guarantee.
