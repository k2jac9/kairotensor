# Choosing an output-token allowance

The experiment uses the pinned Qwen2.5-1.5B-Instruct checkpoint to extract a release, test count and pass/fail verdict from ten synthetic CI reports. Each report was assigned four real generations at output limits of 32, 48, 64 and 96. The [preregistered protocol](token-cap-protocol.md) fixes the data split, success threshold and deadline. The [prompt catalogue](evaluation-prompts.md#output-cap-pilot) documents every input. [Live workflow and per-family logs](https://github.com/k2jac9/kairotensor/actions/runs/43506070105).

The frozen rule selected **64 output tokens**. Both 64 and 96 returned the correct verdict for all six held-out reports. Five arrived within 60 seconds. The same compatibility report missed the deadline at 64 (66.28 seconds) and 96 (67.54 seconds), generating 55 tokens in both calls. Every preflight count matched the actual input count across all 40 calls. None exceeded its reservation, and all 40 reservations reconciled to zero outstanding.

On the six held-out requests, cap 64 reserved 1,553 tokens in aggregate and cap 96 reserved 1,745. **Cap 64 freed 32 tokens of upfront capacity per request**, or 192 summed over six independent budgets. Each policy would have charged 1,430 tokens for the same six verdicts. A smaller reservation can let work begin sooner under a shared ceiling even when actual generation uses the same number of tokens. That concurrency effect was outside this pilot.

Small caps cut off otherwise recoverable answers. At cap 32, the packaging failure report ran out of space partway through the JSON and consumed its entire 32-token output allowance. The same prompt used 54 output tokens to finish correctly at cap 64. Across the entire pilot there were 11 length stops, all with invalid JSON, and three late receipts, one of which was also a length stop. Each family ran on one hosted CPU runner, so these observations do not separate report-family effects from runner speed.

## Recorded traces

[Unit](evaluations/token-caps-unit-2026-11-13.json) · [Integration](evaluations/token-caps-integration-2026-11-13.json) · [Packaging](evaluations/token-caps-package-2026-11-13.json) · [Migration](evaluations/token-caps-migration-2026-11-13.json) · [Compatibility](evaluations/token-caps-compatibility-2026-11-13.json)

The run used evaluation commit `55923d38` and the [protocol frozen before generation](https://github.com/k2jac9/kairotensor/commit/81c1f405667cc907b58a021aff82cb866037d510). The tables below are reproducible from the five traces.

## Calibration

Caps qualify with at least 3 correct, valid, on-time completions among 4 calibration cases.

| Output cap | Successes / 4 | Qualifies |
| ---: | ---: | --- |
| 32 | 2 | no |
| 48 | 2 | no |
| 64 | 4 | yes |
| 96 | 4 | yes |

## Held-out cap sweep

Each row covers the same six cases. Token totals include unsuccessful and late calls. Reaching the cap can coincide with EOS; a length stop counts a cap hit without EOS.

| Cap | Dispatched | Schema valid | Correct | Correct, valid, on time / 6 | Cap hits | Length stops | Charged tokens |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 | 6 | 2 | 2 | 2 | 4 | 4 | 1357 |
| 48 | 6 | 3 | 3 | 3 | 3 | 3 | 1407 |
| 64 | 6 | 6 | 6 | 5 | 0 | 0 | 1430 |
| 96 | 6 | 6 | 6 | 5 | 0 | 0 | 1430 |

## Matched policy comparison

The simulated selector uses only calibration outcomes, the held-out prompt's exact preflight count and the 1,024-token allowance. Each selected outcome comes from that case's measured generation at the selected cap. Abstentions remain in the six-case denominator.

| Policy | Cap selections | Dispatched | Abstained | Correct, valid, on time / 6 | Charged tokens | Successes / 1,000 tokens |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Calibrated | 64 × 6 | 6 | 0 | 5 | 1430 | 3.50 |
| Fixed 96 | 96 × 6 | 6 | 0 | 5 | 1430 | 3.50 |

The complete sweep used 40 model calls and 9,322 tokens across 40 measured cells (7,760 input and 1,562 output). The archive replay is accounted separately. These are paired descriptive results on three held-out fixture families; six cases cannot establish a production success probability.

## How the budget works

Tokenize the complete templated request once. The worker holds those exact inputs and binds them to the proposed output cap. The database grants a dispatch when the following condition holds, including equality.

```text
settled tokens + outstanding reservations + exact input tokens + proposed output cap <= budget
```

After the worker returns, charge the input tokens and the actual generated tokens, including a generated end token. Invalid and late responses incur their measured usage. If the worker disappears after dispatch, the full outstanding reservation continues to consume capacity. These guarantees cover attempts made through the budgeted dispatch API.

In the historical retry, the first invalid answer cost 127 tokens. The revised input required 181, and a 96-token allowance brought the admission threshold to **404**. A budget of 403 denied that request. A budget of 404 admitted it, and the archived completion used 30 tokens, bringing total usage to **338** and freeing 66. A freshly quoted 95-token cap fit at 403. The boundary check recorded a reservation for that counterfactual and made zero new model calls.

## Reproduce the evaluation

The repository workflow evaluates one report family per CPU runner. You can run the same example locally with the pinned Python runtime dependencies in the [workflow](../.github/workflows/token-caps.yml). The default example needs no model and replays the historical accounting boundary.

```sh
cargo run --example token_cap_eval
cargo run --release --example token_cap_eval -- --live --family package --trace package.json
python3 scripts/summarize_token_caps.py docs/evaluations/token-caps-*-2026-11-13.json
```
