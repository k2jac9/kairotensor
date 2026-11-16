# Controlled correction pilot: observed results, 16 November 2026

One bounded local run completed all four planned evidence changes. After the
shared bootstrap, Store repair used **751 reported tokens and four calls**;
full recomputation used **1,167 tokens and six calls**. The difference was 416
tokens, or **35.6% of post-change recomputation usage**. All corresponding
requests matched byte-for-byte and all checked structured plans agreed.

| Evidence change | Checked behavior in both arms | Repair calls / tokens | Fresh calls / tokens |
| --- | --- | ---: | ---: |
| Replace passing report with failing report | Hold deployment; 11/12 tests | 2 / 389 | 2 / 389 |
| Change policy; retain report | Deploy to the new canary target | 1 / 181 | 2 / 389 |
| Withdraw both reports without replacement | Refuse before model dispatch | 0 / 0 | 0 / 0 |
| Withdraw A; retain distinct occurrence B | Use B's own accepted origin; retain staging plan | 1 / 181 | 2 / 389 |

The common bootstrap cost three calls and 597 tokens. Adding that same prefix
to each hypothetical lifecycle gives 1,348 repair tokens versus 1,764 fresh
tokens, a **23.6% difference**. Actual collection charged the shared prefix once:
13 calls and 2,515 tokens overall. Two reuse hits are previously sampled results
with retained origin identities; they are not additional independent samples.

The run finished in 70.14 seconds, within its original 30-minute window. All 13
calls returned reconciled usage; there were no unresolved holds, excess observed
usage, retries, or unexecuted conditions. The owned runtime was stopped and
observed with PID 0, no OOM kill and no remaining running container. Exit 137
records the supervisor's planned final SIGKILL. Only the original shared Ollama
service remained running. Remote model-provider spend was zero; workstation
CPU/electricity cost is unpriced.

Execution used source `f6694110e3df5b1a13f71dbf9fae2bde89e07c44`, the reviewed
binary built at `c7f1da715ec3de2929b0009600acc2ef4fb81c92` with identical compiled
inputs, cached `ornith-1.5:9b` Q4_K_M and Ollama 0.40.16. Every recorded loaded
model observation passed the selected-digest, CPU-only and context checks.
The [protocol](controlled-correction-pilot.md) specifies the frozen requests,
authority, containment and accounting limits. The
[recorded trace](evaluations/controlled-correction-pilot-2026-11-16.json) includes
exact requests, base64-encoded raw responses, usage, Store events, runtime
observations and hashes for the retained evidence files.

An independent read-only post-run audit reconciled every request, raw response,
claim, token receipt and Store event against the SQLite records. It checked the
two retained origins, all 13 CPU observations (context 2,048; VRAM zero), and
stopped-runtime evidence. The audit found no unknown, invalid or stale outcomes;
its hash is included in the trace. This audit made no model calls.

This is one correlated synthetic example per condition using existing report and
policy fixtures. The collected conditions share one bootstrap and one runtime; cache warmth
and order affect timings. These observations support the tested reuse and
withdrawal behavior, but do not establish population accuracy, latency savings,
held-out incident quality, or broader epistemic improvement. The arithmetic
verdicts already have a deterministic product route. Runtime-reported counts are
observations; hashes do not authenticate model execution. Configured token
envelopes remain unproven hard bounds even though this run stayed within them.

No dataset was expanded and no new training run was launched. The existing
AutoScientist experiment is separate. The submission report and its earlier
measurements remain pinned to `20fab84`; this readout is a separate supplement.
