# Scoped model-result reuse

A change to deployment policy can require a new plan while leaving a test report unchanged. The example models those decisions as separate calls. One model call extracts a typed judgment from the report; the planner uses the applicable policy and the validated judgment. All sources and identifiers in the experiment are synthetic.

[Exact prompts from the first and revised runs](evaluation-prompts.md#scoped-reuse-first-run)

## Use the contract

1. Call `begin_model_attempt` with an actor, scope, branch, fixed ordered list of candidate observations, and a versioned computation contract. Use the captured observations to assemble the final adapter request, including model revision, messages and decoding parameters.
2. Pass the exact serialized bytes to `resolve_model_attempt`. On `Dispatch`, send those bytes once, validate the response, then call `settle` with the output and measured usage.
3. On `Reused`, use the returned output. The store has already published a new branch head and recorded its original execution. A hit incurs zero new model usage. Callers requiring a fresh sample should use the ordinary execution path.

The check compares request bytes, contract version, branch, candidate list and ordered submitted occurrences. An effective change in the admission of any candidate after the original capture disqualifies that result, even if admission was subsequently restored. An unrelated change in the same actor's view can permit reuse. Only the current head of the same branch can supply an eligible result. A chain of hits points directly back to the original accepted dispatch. An intervening opaque head prevents lookup past it. Open stores migrate to schema v4, and historical dispatches keep their original execution-only semantics.

Resolution is one-shot. After an uncertain dispatch or lost response the caller must reconcile transport before attempting another send. Eligibility is checked at resolution and again on settlement of fresh work. Historical accounting does not grant current access to a withdrawn result.

The caller is responsible for naming all inputs that can change the decision and for updating the contract version when the validator or input-selection semantics change. This applies to closed computations over immutable observations. The payload of a hit is the same realization produced by the original model call. Usage remains charged to that original attempt.

## Reproduce

```sh
cargo run --example scoped_model_reuse -- --trace reuse-trace.json
cargo run --release --example scoped_model_reuse -- --live --trace reuse-live.json
```

The first command uses a deterministic adapter. The second uses the pinned CPU model from the [corrected handoff example](corrected-handoff.md#run), which lists the Python dependencies. Inspect the trace for the fresh, reused and negative-control operations.

## Evaluation design

The bootstrap uses a policy authorizing staging and a test report reporting 12/12 passed. The report and its initially unadmitted replacement are the report judgment's fixed candidate list. Deployment policy is an input only to the planner. Both model tasks use strict typed checklists. A model response with the wrong field, duplicate key, or unexpected key fails validation.

The repair withdraws the old policy and admits a new canary policy in a controlled single-writer scenario. The fresh route uses a separate store with the same revised policy, admitted report and unadmitted replacement. Both routes require identical model-visible requests and the same validated plan. The negative control withdraws the passing report and admits its replacement with 11/12 passed. It must generate a new judgment and hold the release.

Marginal route timing starts after preparing each evidence view and covers the two tasks through publication. The bootstrap and negative control have separate metrics. A reuse event records its origin attempt and zero new model tokens. Its lookup and validation still take application time.

## Initial live evaluation

In the first live Qwen run, the repaired route reused its passing report judgment and produced the same canary plan as fresh execution. Replacing the report forced a fresh call as required. The model copied the new `11/12` count but incorrectly labeled its status `pass`. The validator stopped publication and the experiment exited before generating a plan from the invalid judgment. The trace records the unsuccessful response and its measured usage; this invalid attempt remained unsettled in that historical run. The revised extraction prompt gives balanced examples of passing and failing reports. The report data, Qwen checkpoint and strict output check are unchanged. [First run](https://github.com/k2jac9/kairotensor/actions/runs/43490325882) · [First trace](evaluations/scoped-reuse-first-qwen-2026-11-13.json).

## Live result after the prompt revision

Qwen2.5-1.5B-Instruct ran on CPU with the pinned model and tokenizer revision `989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, two threads, PyTorch 2.6.0+cpu, and Transformers 4.48.3. All returned plans and judgments passed the same field validator used in the first evaluation.

| Phase | Actual model calls | Reused judgments | Input tokens | Output tokens | Observed route time |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial staging plan | 2 | 0 | 326 | 78 | 15.78 s |
| Repair after policy correction | 1 | 1 | 145 | 48 | 9.02 s |
| Fresh rebuild on corrected evidence | 2 | 0 | 326 | 78 | 15.42 s |
| Replacement report, 11/12 passing | 2 | 0 | 326 | 78 | 15.65 s |

Repair and clean execution produced the same checked canary plan, with approval `C42-B` and the original 12/12 test result. The opt-in report judgment was the only reuse hit. Its attempt points to the original accepted dispatch; the additional model usage was zero. The two paths used byte-identical model requests for the corresponding tasks. Reuse saved 211 of 404 prompt-plus-completion tokens (52.2%) during correction. On the negative control the report changed, Kairotensor ran both tasks, and the plan action changed to `hold`.

All four routes together ran seven generations and consumed 1,123 input and 282 output tokens. Route timings are individual observations on one synthetic task, with the views already prepared and the model loaded. They do not establish a latency distribution or expected savings on production workloads. [Successful live workflow](https://github.com/k2jac9/kairotensor/actions/runs/43491028189) · [All requests, outputs, usage, and source lineage](evaluations/scoped-reuse-qwen-2026-11-13.json).


## Replay the failed verdict

```sh
cargo run --example scoped_model_reuse -- --replay-invalid
```

The replay reads the archived model request, response, and measured usage from the first run and applies the report validator to them in a fresh store. It records the response that reported `11/12` and `pass` as `invalid_output`. Its 93 input and 34 output tokens give a single 127-unit ledger entry. An identical second delivery returns the original disposition. The replay issues zero model calls and the archived trace retains its original pending state.

For a newly dispatched response that fails its checklist, the caller supplies the validator version and reason to `reject_invalid`. The store records both the validation outcome and whether the attempt was current, had a stale view, or had a stale branch at settlement. A retry must reproduce the validator, reason, bytes and usage exactly. The rejected attempt cannot advance a head or become the origin for model-result reuse. An authorization to dispatch is the store's precondition; the adapter remains responsible for accurately reporting the response and usage that it received.
