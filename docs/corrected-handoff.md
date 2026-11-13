# Corrected agent handoff

This example asks a release planner for a structured deployment instruction. The original requirement names staging. A correction changes the target to canary. The same test report applies to either environment. All sources, approvals and release identifiers in this example are synthetic.

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
