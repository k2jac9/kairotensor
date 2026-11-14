# Recovery policy v2: measured contract and deadline failures

Frozen source: `11234573896b68699efa00df63e88553f8d8e99a`.
[Run 43571699931](https://github.com/k2jac9/kairotensor/actions/runs/43571699931)
completed successfully as a collection job on 2026-11-14. That is not experimental
success. [Artifact](https://github.com/k2jac9/kairotensor/actions/runs/43571699931/artifacts/14105182044),
[durable ZIP](recovery-policy-v2-2026-11-14.zip), SHA256:
`74e8895335f304be0db1d8c705b1e448cb0588dd1b1ccff7296e4a2ff6f8af0d`.

## Frozen result

Ten cases produced 20 actual Qwen calls and 5,241 charged tokens (4,545 input,
696 output), with zero outstanding reservations. Every response was Markdown
fenced and failed the unchanged strict JSON contract. All 20 ended at EOS;
none hit the 96-token cap. No retrospective fence stripping or reference change
was used for frozen scoring. V2 preserves the original validator, model, 4,096-token budget and
60-second end-to-end soft deadline.

| Held-out policy | Calls | Charged tokens | Successful cases | Last receipt on time |
| --- | ---: | ---: | ---: | ---: |
| always_stop | 6 | 1,316 | 0/6 | 6/6 |
| affordable_retry | 12 | 3,196 | 0/6 | 0/6 |
| evidence_gated_retry | 10 | 2,548 | 0/6 | 2/6 |

These are paired replays of shared actual trajectories, not three independently
executed policy arms. On-time is not success. All ten initial receipts were before
60 seconds, so a retry was admissible; all ten retry receipts were after the
end-to-end deadline. The soft deadline does not interrupt a running model call.
Generation alone ranged from 51.52 to 79.83 seconds per call; final operation
receipts ranged from 124.14 to 137.39 seconds.

## Interpretation

The calibration-only follow-up shows numeric count copying improved compared
with v1, but all eight calibration replies still used fences and some statuses
remained wrong. Stronger instructions did not establish compliance or recovery
benefit. No further prompt was tuned against the fresh held-out outputs.

The hosted CPU changed: v1 reported AMD EPYC 9V74, v2 AMD EPYC 7763. Both used
`bfloat16`, two threads and the pinned runtime. Prompt length and hardware both
changed, so this is not a controlled latency comparison and does not identify the
cause of the slowdown. CPU compatibility/performance deserves a controlled
calibration benchmark before spending on more full comparisons. The six v2
held-out inputs are fresh instances, not an external benchmark.

## Cost and audit

Job duration 07:34:04–07:58:08 UTC was 24m04s: 25 rounded Linux minutes at
$0.006/minute gives $0.150 nominal compute. Two associated CI jobs (58s, 59s)
add $0.012, totaling $0.162 for this launch. The 45,141-byte artifact contributes
approximately $0.00001036 gross storage over 30 days. Included allowances may
reduce the charge; actual account billing remains unavailable. This excludes
subsequent development/CI work.

Sources: [runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)
and [Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions).

`python3 scripts/audit_recovery_bundle.py EXTRACTED_DIRECTORY` reconciles this
bundle's requests, quotes, evidence, raw replies, delivery records, timings and
charges with SQLite. It verifies internal consistency, not independent execution
authenticity. The original ZIP is preserved unchanged. No new model run or merge
is implied by archiving these findings.
