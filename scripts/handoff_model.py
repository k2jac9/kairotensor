"""Persistent CPU model worker for the synthetic corrected_handoff example.

The Rust adapter chooses an immutable Hub revision and supplies it on startup.
Every subsequent JSONL record is a complete stateless chat. Model weights are
shared across calls; each generation gets a fresh template rendering and cache.
"""
import json
import os
import sys
import time


def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)


def main():
    import torch
    import transformers
    from transformers import AutoModelForCausalLM, AutoTokenizer

    model_id, revision = sys.argv[1:]
    torch.set_num_threads(max(1, min(4, len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else (os.cpu_count() or 1))))
    torch.manual_seed(0)
    start = time.perf_counter_ns()
    source = dict(revision=revision, trust_remote_code=False, token=False)
    tokenizer = AutoTokenizer.from_pretrained(model_id, **source)
    model = AutoModelForCausalLM.from_pretrained(
        model_id, torch_dtype=torch.bfloat16, low_cpu_mem_usage=True,
        use_safetensors=True, **source,
    ).eval()
    emit({
        "ready": True, "model": model_id, "revision": revision, "torch": str(torch.__version__),
        "transformers": transformers.__version__, "threads": torch.get_num_threads(),
        "load_us": (time.perf_counter_ns() - start) // 1000,
    })

    for line in sys.stdin.buffer:
        request = json.loads(line)
        if (request["adapter"] != "transformers-cpu/v1" or request["model"] != model_id
                or request["revision"] != revision or request["dtype"] != "bfloat16"
                or request["add_generation_prompt"] is not True):
            raise ValueError("model worker received an incompatible request")
        generation = request["generation"]
        if generation != {"do_sample": False, "num_beams": 1, "max_new_tokens": 96}:
            raise ValueError("unexpected decoding parameters")
        inputs = tokenizer.apply_chat_template(
            request["messages"], tokenize=True, add_generation_prompt=True,
            return_tensors="pt", return_dict=True,
        )
        start = time.perf_counter_ns()
        with torch.inference_mode():
            ids = model.generate(**inputs, **generation)
        elapsed_us = (time.perf_counter_ns() - start) // 1000
        input_length = inputs["input_ids"].shape[1]
        completion = ids[0, input_length:]
        # Usage counts include chat-template tokens and any generated end token.
        # Response text follows normal chat decoding and omits special tokens.
        emit({
            "text": tokenizer.decode(completion, skip_special_tokens=True),
            "input_tokens": input_length, "output_tokens": completion.numel(),
            "generation_us": elapsed_us,
        })


if __name__ == "__main__":
    main()
