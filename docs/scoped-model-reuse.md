# Scoped model-result reuse

A change to deployment policy can require a new plan while leaving a test report unchanged. The example models those decisions as separate calls. One model call extracts a typed judgment from the report; the planner uses the applicable policy and the validated judgment. All sources and identifiers in the experiment are synthetic.

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
