# Output-cap pilot protocol

This pilot tests how often a pinned local model emits a valid, correct report verdict before a host deadline at four output limits. The 2026-11-13 protocol freezes the prompt, data, split, policy and scoring rules before running these fixtures. The [executable corpus and validator](../examples/token_cap_eval/cases.rs) define the exact ten reports and their reference answers.

## Fixtures and intervention

Each report uses the existing `ci_report` schema. The system instruction is copied from the archived [successful report judgment](evaluations/scoped-reuse-qwen-2026-11-13.json). Its two examples refer to EX1 and EX2. The user message for each new case contains only the source release, run version, passed/total summary and number of failures. The model receives no scoring label, case ID, split, deadline or other cap's answer. The pinned Qwen model, tokenizer, generation prompt, template, greedy decoding and validator stay fixed.

| Data role | Families | Cases per family | Use |
| --- | --- | ---: | --- |
| Calibration | Unit, integration | 2 | Determine which caps meet the empirical success target |
| Held-out evaluation | Packaging, migration, compatibility | 2 | Score the frozen choice |

Every family contains one fully passing case and one case with at least one failure. The release identifiers vary in token length. Held-out families have distinct release and run identifiers and numerators/denominators. All families share a schema and task. These are synthetic fixtures, and the six held-out cases provide limited resolution.

Actually generate each of 32, 48, 64 and 96 new-token caps on every case, giving 40 evaluations. Within each case the prompt is identical across caps. The fixed ordering rotates the caps to reduce warm-up effects; the four calibration cases cover all four positions evenly. Use the real model outputs for each cap. Cap trials on the same case are paired and dependent.

## Accounting and outcome

Each trial has its own 1,024-token budget. The exact input count comes from applying the model tokenizer and template before generation. Reserve that input count plus the enforced cap. Record the actual input and generated token counts and reconcile the hold. The separate historical boundary check starts from a recorded charge of 127 tokens. Its revised prompt used 181 input tokens; a 96-token cap needs a reservation of 277. A total budget of 403 misses by one. At 404 there is capacity; replaying the recorded 30-token completion results in a combined 338-token charge and releases 66. Record historical usage distinctly from new measurement.

A verdict is schema-valid when it is one JSON object containing exactly the three required string fields (`release`, `tests`, `status`); status must be `pass` or `fail`. A single surrounding JSON code fence is allowed. Correctness also requires matching the independently declared reference values. Score the final outcome after validation, durable settlement and receipt by the local caller. A success meets the schema, reference, and the 60-second per-attempt deadline, including preflight, reservation, inference, validation and settlement; model startup and fixture setup are outside that window. Rejections, truncated JSON, late replies and abstentions contribute zero successes. Operational failures and missing cells are surfaced; incomplete data cannot establish a complete cap matrix.

A cap hit means generation returned exactly the configured maximum number of tokens. Record whether it also generated an end token. If the final token is an end token, reaching the cap is consistent with normal completion. A syntactically complete, correct answer can count even if generation exhausts the limit. An incomplete response at the cap counts as unsuccessful. The cap supplies only a lower bound on how long that generation would have continued without the limit when no natural stop is observed.

## Frozen selection rule

For each cap, calculate valid, correct, on-time verdicts divided by **all four calibration cases**. The threshold is 75%, so a cap qualifies with at least three successes. Before considering held-out outcomes, choose the smallest qualifying cap that fits the held-out request's exact input and remaining budget. Abstain if none qualifies or fits. Apply the same selection rule to every case; the chosen cap must come from the four measured values. Do not choose a cap retrospectively using the held-out labels.

Report the complete per-cap outcomes on the six held-out cases, the selected policy's outcomes with abstentions in the denominator, fixed-96 results on the same cases, and spent tokens including invalid and late answers. Across the sweep, report measured prompt and generated tokens, cap hits and end-token flags. The per-policy resource total uses only its selected trial per case; the research sweep actually executes all 40 trials. The 75% target is a calibration threshold. This small pilot cannot guarantee a success probability on unseen production requests or a stable cross-runner latency distribution.
