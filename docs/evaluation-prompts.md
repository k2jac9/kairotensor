# Prompts used in the live Qwen evaluations

On 13 November 2026, the corrected-handoff and scoped-reuse experiments recorded three live evaluations. The tables map each operation to the exact system, user and optional assistant messages reproduced below. You can reconstruct any of the 19 recorded requests from these blocks or extract its serialized bytes from the linked trace. Qwen ran 17 times. Two repair operations reused an earlier report judgment.

The [output-cap pilot](#output-cap-pilot) reuses one of these system instructions with ten new user messages.

## Which messages reached the model?

Within each run, read the rows from top to bottom. In the message column, the first label names a system instruction and the last names a user payload. Only the delayed handoff inserts an assistant message between them. A reuse row shows the request assembled for comparison and consumed zero new tokens.

### Corrected handoff

[Recorded requests and outputs](evaluations/corrected-handoff-qwen-2026-11-13.json) · [Live workflow](https://github.com/k2jac9/kairotensor/actions/runs/43487537572)

| Phase | Task | Messages in order | Execution / outcome |
| --- | --- | --- | --- |
| `initial` | deployment-plan | [Handoff planner](#handoff-planner) → [Handoff staging](#handoff-staging) | Dispatched, accepted |
| `delayed` | deployment-plan | [Handoff planner](#handoff-planner) → [Old plan checkpoint](#old-plan-checkpoint) → [Handoff staging](#handoff-staging) | Dispatched, rejected on delivery after correction |
| `repair` | deployment-plan | [Handoff planner](#handoff-planner) → [Handoff canary](#handoff-canary) | Dispatched, accepted |
| `fresh` | deployment-plan | [Handoff planner](#handoff-planner) → [Handoff canary](#handoff-canary) | Dispatched, accepted |

### Scoped reuse, first run

[Recorded requests and outputs](evaluations/scoped-reuse-first-qwen-2026-11-13.json) · [Live workflow](https://github.com/k2jac9/kairotensor/actions/runs/43490325882)

| Phase | Task | Messages in order | Execution / outcome |
| --- | --- | --- | --- |
| `initial` | ci-judgment | [Report judgment v1](#report-judgment-v1) → [Passing report](#passing-report) | Dispatched, accepted |
| `initial` | release-plan | [Release planner v1](#release-planner-v1) → [Staging with pass](#staging-with-pass) | Dispatched, accepted |
| `repair` | ci-judgment | [Report judgment v1](#report-judgment-v1) → [Passing report](#passing-report) | Reused initial judgment (zero new tokens) |
| `repair` | release-plan | [Release planner v1](#release-planner-v1) → [Canary with pass](#canary-with-pass) | Dispatched, accepted |
| `fresh` | ci-judgment | [Report judgment v1](#report-judgment-v1) → [Passing report](#passing-report) | Dispatched, accepted |
| `fresh` | release-plan | [Release planner v1](#release-planner-v1) → [Canary with pass](#canary-with-pass) | Dispatched, accepted |
| `report_replaced` | ci-judgment | [Report judgment v1](#report-judgment-v1) → [Replacement report](#replacement-report) | Dispatched, failed validation (unsettled) |

### Scoped reuse, revised run

[Recorded requests and outputs](evaluations/scoped-reuse-qwen-2026-11-13.json) · [Live workflow](https://github.com/k2jac9/kairotensor/actions/runs/43491028189)

| Phase | Task | Messages in order | Execution / outcome |
| --- | --- | --- | --- |
| `initial` | ci-judgment | [Report judgment v2](#report-judgment-v2) → [Passing report](#passing-report) | Dispatched, accepted |
| `initial` | release-plan | [Release planner v2](#release-planner-v2) → [Staging with pass](#staging-with-pass) | Dispatched, accepted |
| `repair` | ci-judgment | [Report judgment v2](#report-judgment-v2) → [Passing report](#passing-report) | Reused initial judgment (zero new tokens) |
| `repair` | release-plan | [Release planner v2](#release-planner-v2) → [Canary with pass](#canary-with-pass) | Dispatched, accepted |
| `fresh` | ci-judgment | [Report judgment v2](#report-judgment-v2) → [Passing report](#passing-report) | Dispatched, accepted |
| `fresh` | release-plan | [Release planner v2](#release-planner-v2) → [Canary with pass](#canary-with-pass) | Dispatched, accepted |
| `report_replaced` | ci-judgment | [Report judgment v2](#report-judgment-v2) → [Replacement report](#replacement-report) | Dispatched, accepted |
| `report_replaced` | release-plan | [Release planner v2](#release-planner-v2) → [Canary with fail](#canary-with-fail) | Dispatched, accepted |

## What changed between the scoped runs?

Both runs used the same replacement report, with one failing case and `11/12` passing. In the first run the model copied `11/12` correctly but returned `"status":"pass"`. The validator stopped publication, so the planner never saw that judgment.

The revised **report judgment** prompt embeds one failing and one passing example in the system instruction. The revised **release planner** prompt spells out that a failing certificate requires a hold even with an approval code. The validator and model revision stayed fixed. Against the same replacement report the second run produced `"status":"fail"` and then `"action":"hold"`. The report reuse contract advanced from `ci-report-extractor/1+strict-verdict-validator/1` to `ci-report-extractor/2+strict-verdict-validator/1`. The trace records that identifier separately from the message text.

## System instructions

These blocks come from the decoded `messages[].content` strings in the traces. They preserve capitalization, punctuation and line breaks.

### Handoff planner

````text
You are a deployment planner. Return exactly one JSON object with five string fields: release, target, approval, tests, action. Copy release, target and approval from requirement. Copy tests from test_certificate. For a passing certificate use action deploy. Do not add other keys or explanation.
````

### Report judgment v1

````text
Read ci_report. Return only a JSON object with three string fields: release, tests, status. Copy release. Copy test_summary as tests. Set status to pass if failing_cases is zero, otherwise fail. No other keys or explanation.
````

### Release planner v1

````text
Return only a JSON deployment plan with five string fields: release, target, approval, tests, action. Copy release, target and approval from requirement. Copy tests from test_certificate. Use action deploy for status pass, or hold for status fail. No other keys or explanation.
````

### Report judgment v2

````text
Extract the release test verdict. Even ONE failing case makes the entire verdict "fail".
CI example A: {"release":"EX1","test_summary":"7/8","failing_cases":1} -> {"release":"EX1","tests":"7/8","status":"fail"}
CI example B: {"release":"EX2","test_summary":"8/8","failing_cases":0} -> {"release":"EX2","tests":"8/8","status":"pass"}
Read the user's ci_report. Copy release and test_summary. If failing_cases is positive use "fail"; only zero uses "pass". Return only JSON with keys release, tests, status.
````

### Release planner v2

````text
Create a JSON release plan. Copy release, target and approval from requirement. Copy tests from test_certificate. The certificate decides the action: {"status":"pass"} means {"action":"deploy"}; {"status":"fail"} means {"action":"hold"} even with an approval code. Return exactly five string fields: release, target, approval, tests, action. No explanation.
````

## User messages

The Rust adapter sent each of these compact JSON objects as the content of a single user message. The three planner payloads for scoped reuse carry the validated judgment, and the report-judgment payloads carry only the report.

### Handoff staging

````json
{"requirement":{"approval":"S42-A","release":"R42","target":"staging","version":"policy/v1"},"sources":["policy/v1","ci/run-17"],"test_certificate":{"release":"R42","tests":"12/12"},"test_verifier":"release-fixture/check-all-12/v1"}
````

### Handoff canary

````json
{"requirement":{"approval":"C42-B","release":"R42","target":"canary","version":"policy/v2"},"sources":["policy/v2","ci/run-17"],"test_certificate":{"release":"R42","tests":"12/12"},"test_verifier":"release-fixture/check-all-12/v1"}
````

### Passing report

````json
{"ci_report":{"failing_cases":0,"release":"R42","test_summary":"12/12","version":"run/17"}}
````

### Replacement report

````json
{"ci_report":{"failing_cases":1,"release":"R42","test_summary":"11/12","version":"run/18"}}
````

### Staging with pass

````json
{"report_version":"run/17","requirement":{"approval":"S42-A","release":"R42","target":"staging","version":"policy/v1"},"test_certificate":{"release":"R42","status":"pass","tests":"12/12"}}
````

### Canary with pass

````json
{"report_version":"run/17","requirement":{"approval":"C42-B","release":"R42","target":"canary","version":"policy/v2"},"test_certificate":{"release":"R42","status":"pass","tests":"12/12"}}
````

### Canary with fail

````json
{"report_version":"run/18","requirement":{"approval":"C42-B","release":"R42","target":"canary","version":"policy/v2"},"test_certificate":{"release":"R42","status":"fail","tests":"11/12"}}
````

## Assistant history

The delayed handoff received the prior accepted plan with the checkpoint marker prepended. Its final three characters are the closing backticks. The repaired and fresh requests both omit this entire message.

### Old plan checkpoint

````text
old-plan checkpoint
```json
{
  "release": "R42",
  "target": "staging",
  "approval": "S42-A",
  "tests": "12/12",
  "action": "deploy"
}
```
````

## Model settings and extraction

All three runs used `Qwen/Qwen2.5-1.5B-Instruct` at revision `989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, CPU bfloat16, greedy decoding (`do_sample=false`, `num_beams=1`) and at most 96 new tokens. The worker applied that revision's tokenizer chat template with `add_generation_prompt=true`. The trace stores the serialized adapter envelope before the worker renders token IDs. [Worker code](../scripts/handoff_model.py)

From the repo root, this extracts the **exact UTF-8 envelope** for the revised failing-report judgment, without appending a newline. The second command displays its role-tagged messages. For the corrected handoff, use `.calls[]` in place of `.events[]` and select by phase.

```sh
jq -j '.events[] | select(.phase == "report_replaced" and .task == "ci-judgment") | .request_utf8' \
  docs/evaluations/scoped-reuse-qwen-2026-11-13.json > request.json
jq '.messages' request.json
```

The Rust examples define the current prompts and strict field validators. The historical traces preserve the instructions from before the revision. [Corrected handoff source](../examples/corrected_handoff.rs) · [Scoped reuse source](../examples/scoped_model_reuse.rs)

## Output-cap pilot

Each trial uses two role messages in order. The system message is the exact multiline [Report judgment v2](#report-judgment-v2) instruction above, including both EX1 and EX2. The user message is the compact JSON string listed below. Each case keeps these two strings fixed across caps of 32, 48, 64 and 96 new tokens. The cap is a generation parameter in the request envelope. Case IDs, split names and reference labels stay outside the messages. [Frozen fixtures](../examples/token_cap_eval/cases.rs) · [Protocol](token-cap-protocol.md) · [Results](token-cap-results.md).

| Case ID (harness only) | User message content |
| --- | --- |
| `unit-green` | `{"ci_report":{"failing_cases":0,"release":"U9","test_summary":"9/9","version":"unit/81"}}` |
| `unit-red` | `{"ci_report":{"failing_cases":2,"release":"unit-runner-linux-arm64-release-2026.11.09+build.371","test_summary":"22/24","version":"unit/82"}}` |
| `integration-green` | `{"ci_report":{"failing_cases":0,"release":"integration-bundle-pg17-linux-aarch64-2026.11.10+run.204","test_summary":"36/36","version":"integration/204"}}` |
| `integration-red` | `{"ci_report":{"failing_cases":1,"release":"INT-14","test_summary":"30/31","version":"integration/205"}}` |
| `package-green` | `{"ci_report":{"failing_cases":0,"release":"PKG-72","test_summary":"6/6","version":"package/52"}}` |
| `package-red` | `{"ci_report":{"failing_cases":1,"release":"package-registry-x86_64-release-2026.11.08+dist.490","test_summary":"17/18","version":"package/53"}}` |
| `migration-green` | `{"ci_report":{"failing_cases":0,"release":"ledger-migration-v004-to-v005-postgresql-2026.11.12+schema.142","test_summary":"48/48","version":"migration/310"}}` |
| `migration-red` | `{"ci_report":{"failing_cases":3,"release":"MIG-5","test_summary":"13/16","version":"migration/311"}}` |
| `compatibility-green` | `{"ci_report":{"failing_cases":0,"release":"sdk-compatibility-matrix-rust-1.99-python-3.11-linux-arm64+bundle.850","test_summary":"52/52","version":"compat/70"}}` |
| `compatibility-red` | `{"ci_report":{"failing_cases":3,"release":"client-api-v3.8.2","test_summary":"7/10","version":"compat/71"}}` |

From the repo root, these commands extract the exact adapter request for the 48-token `unit-red` trial and display its two messages.

```sh
jq -j '.samples[] | select(.case_id == "unit-red" and .cap == 48) | .request_utf8' \
  docs/evaluations/token-caps-unit-2026-11-13.json > request.json
jq '.messages' request.json
```
