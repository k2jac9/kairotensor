# Fixed-host recovery calibration: syntax improves, semantics still fail

[Run 43576919529](https://github.com/k2jac9/kairotensor/actions/runs/43576919529)
completed the approved eight-call plan on one AMD EPYC 7763 runner. Source:
`6347003f3a84ec2d1101440b970d85f4c825d1d9`.
[Artifact 14106656697](https://github.com/k2jac9/kairotensor/actions/runs/43576919529/artifacts/14106656697)
ZIP SHA256: `7d982eeb43c4d5ac99f01b9b5081edb2da726defaff19b8978c19752dc2b4cc7`.
Hosted retention ends 2026-11-15 at 08:47 UTC. With owner approval, the unchanged
[ZIP is archived here](recovery-calibration-2026-11-14.zip). No held-out cases were executed.

## Results

All eight calls completed, with zero uncertain invocations. Usage was 1,640 tokens:
1,432 input and 208 output. Each prompt used 179 tokens. Free replies used 33
output tokens; construction-assisted replies used 19 including EOS. No output cap
was hit. The pinned model, prompt, two threads and machine were shared across cells.

| Dtype / output method | Strict correctness | Generation time, cal-pass | Generation time, cal-repair |
| --- | ---: | ---: | ---: |
| BF16 / free | 0/2 | 52.95s | 52.63s |
| BF16 / construction-assisted | 1/2 | 48.14s | 47.84s |
| FP32 / free | 0/2 | 16.92s | 16.31s |
| FP32 / construction-assisted | 1/2 | 76.24s (first after load) | 12.13s |

The constrained path produced valid raw JSON in every cell, but the model chose
`pass` for both 12/12 and 11/12. Therefore it enforced raw JSON formatting and copied fields, but did not fix
the incorrect judgment. Free replies remained fenced and failed the unchanged
strict validator. Candidate construction copied release/counts from admitted
evidence and offered both statuses; it did not supply the correct status label.
This is construction-assisted output, not unrestricted model JSON compliance.

Warm observed FP32 calls were faster on this same machine, supporting further
investigation of dtype choice. The first FP32 call was much slower and must not be
dropped: dtype loading, cold caches, memory behavior and execution order remain
confounded. Model load durations were 17.59s BF16 and 24.07s FP32, excluded from
reported generation. Two cases per profile do not establish a latency bound,
production accuracy or a universally preferable runtime.

## Admission implications

The recorded conservative generation-through-validation maxima were 52,952ms for
BF16/free, 48,148ms BF16/constrained, 76,245ms FP32/constrained and 16,927ms
FP32/free. With the declared 1,000ms margin, hypothetical admission at 55s into a
60s operation rejected every profile. At elapsed zero the cold-inclusive
FP32/constrained profile was also rejected. These are explicit diagnostic
calculations, not Store dispatches or measured recovery outcomes.

These 179-token initial requests do not cover the longer feedback request from
the earlier pilots. The helper refuses quotes outside the measured input-size
coverage instead of extrapolating. Production retry integration still requires
representative feedback calibration and a clear cold/warm readiness contract.

## Cost and scope

The live job ran 08:40:22–08:47:14 UTC (6m52s), rounding to seven Linux minutes:
$0.042 nominal compute at $0.006/minute. Preparation CI added $0.024 and launch CI
$0.018, so the approved window had used $0.084 before result/cleanup CI. The
28,530-byte hosted artifact contributes roughly $0.00000022 gross storage over
one day. Account allowances/rounding may change the invoice; actual charges are
unverified. Earlier pilots are excluded from this window.

The one-shot trigger is removed after collection. No repeat, second experiment,
held-out run or merge was performed. Artifact staging copied all four requested
files, within its 4 MiB payload limit.

## Next engineering decision

Keep strict validation. For this report task, deterministic code can calculate
whether passed equals total; the existing validator already performs that check.
A model-free route could return that computed verdict directly instead of asking
the model to select status. This is a different construction path, not a gain in
model accuracy.
If retaining a model-driven verdict as an experimental baseline, test explicit
semantic feedback on calibration rather than accepting valid JSON as sufficient.
Separate cold readiness from warm generation measurements, instrument the slow
first-call path, and calibrate the exact feedback request before deploying a
latency gate. Freeze those decisions before any fresh held-out evaluation.
