# Corrected agent handoff

This example asks a release planner for a structured deployment instruction. The original requirement names staging. A correction changes the target to canary. The same test report applies to either environment. All sources, approvals and release identifiers in this example are synthetic.

[Exact handoff prompts and message order](evaluation-prompts.md#corrected-handoff)

## Run

Use the deterministic adapter to exercise the storage and correction protocol without downloading weights.

```sh
cargo run --example corrected_handoff -- --trace handoff-trace.json
```

The live adapter runs a pinned Qwen2.5-1.5B-Instruct checkpoint locally on CPU. The Python worker remains running across calls so the experiments reuse model weights. Each call renders its own complete request with a fresh attention cache. The model receives only the synthetic messages.

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install --index-url https://download.pytorch.org/whl/cpu torch==2.6.0
python3 -m pip install transformers==4.48.3 accelerate==1.3.0 huggingface_hub==0.34.4 safetensors==0.5.3 'numpy<2'
cargo run --release --example corrected_handoff -- --live --trace handoff-trace.json
```

The worker downloads the model on first use. Its tokenizer and chat template come from the same immutable revision as the weights. Each trace contains the client-visible request, raw decoded answer and source identifiers. Live token counts include the chat template and generated end tokens. The deterministic adapter does not report token estimates. The application records generation time and adapter round-trip time. Model download and initialization happen before individual calls.

## What the checks establish

The planner must produce the required release, target, approval, test status and action. An accepted staging output is retained as an old continuation. A second invocation uses that continuation; the fixture delays delivery of its completed response until after the requirement changes. It checks the late disposition and retries the same settlement to test that the charge is recorded once.

The repair starts a fresh plan from eligible evidence. A separate database supplies a clean rebuild from the revised raw sources. The two final requests must match at the adapter boundary while the repaired branch retains the previously verified test result. Each result is checked against the same release requirements. A single successful run measures this fixture under this adapter; it does not measure general planning ability or latency distributions.

Schema v3 stores exact assembled request bytes by attempt before submission. The write checks the current view, branch head and pending state. Settlement checks the view and branch again when the response arrives. Stored request bytes represent the adapter input at that boundary; the tokenizer subsequently renders model input IDs. Historical audit material includes retired evidence. The store's current read APIs continue to check current eligibility.

## Live evaluation, 13 November 2026

Qwen2.5-1.5B-Instruct generated the following four plans using two CPU threads, PyTorch 2.6.0+cpu and Transformers 4.48.3. The pinned model and tokenizer revision was `989aa7980e4cf806f80c7fef2b1adb7bc71aa306`. Each answer passed the field checklist for the source view in which its request was captured.

| Phase | Target / approval | Settlement | Input / output tokens | Model round-trip |
| --- | --- | --- | ---: | ---: |
| Initial | staging / S42-A | Accepted | 142 / 52 | 49.81 s |
| Late delivery with original checkpoint | staging / S42-A | Rejected as stale | 202 / 52 | 64.43 s |
| Repair | canary / C42-B | Accepted | 142 / 52 | 50.02 s |
| Fresh rebuild | canary / C42-B | Accepted | 142 / 52 | 50.63 s |

The repaired plan reused the test certificate tied to occurrence 2 and verification attempt 1. Its next model request excluded the old requirement version, target, approval and checkpoint. The fresh database repeated the exact report check once. Both final adapter requests were byte-identical, and both generated the same five field values, including release R42 and test status 12/12.

The late response had already been generated when its delivery was withheld. After the correction, the same stale delivery was submitted twice. The ledger increased by 254 measured token units exactly once. The four actual generations together used 628 input and 208 output tokens. Both corrected routes used the same number of model tokens. Their single observed route times, including local work, were 50.018 s and 50.630 s; no repeated-run latency comparison was made. The independent check is deliberately small, and the example demonstrates one saved verifier invocation. A result that fails the field checklist remains unaccepted; its raw response and usage are retained in the experiment trace when requested.

[Live workflow](https://github.com/k2jac9/kairotensor/actions/runs/43487537572) · [Frozen requests, source IDs, outputs, usage and timing](evaluations/corrected-handoff-qwen-2026-11-13.json).
