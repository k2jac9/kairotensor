"""Persistent CPU model worker for the synthetic Kairotensor examples.

A preflight prepares exactly one request. Generation consumes those already
tokenized inputs and the bound cap; direct 96-token chat requests remain
available for the earlier handoff examples.
"""
import json
import os
import secrets
import sys
import time


def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)


class RequestHandler:
    """One outstanding preparation per worker; generation or discard consumes it."""

    def __init__(self, tokenize, generate):
        self.tokenize = tokenize
        self.generate = generate
        self.pending = None

    def handle(self, message):
        op = message.get("op")
        if op is None:
            if self.pending is not None:
                raise ValueError("a request is already prepared")
            if message.get("generation", {}).get("max_new_tokens") != 96:
                raise ValueError("variable caps require preflight")
            inputs, generation = self.tokenize(message)
            return self.generate(inputs, generation)

        if op == "prepare":
            if set(message) != {"op", "request_utf8"} or not isinstance(message["request_utf8"], str):
                raise ValueError("prepare requires one frozen JSON request")
            if self.pending is not None:
                raise ValueError("generate or discard the outstanding preparation first")
            request = json.loads(message["request_utf8"])
            inputs, generation = self.tokenize(request)
            handle = secrets.token_hex(16)
            count = inputs["input_ids"].shape[1]
            self.pending = (handle, inputs, generation)
            return {"handle": handle, "input_tokens": count, "max_new_tokens": generation["max_new_tokens"]}

        if op not in ("generate", "discard"):
            raise ValueError("unknown worker operation")
        if self.pending is None or message.get("handle") != self.pending[0]:
            raise ValueError("unknown or consumed preparation")
        handle, inputs, generation = self.pending
        if op == "discard":
            if set(message) != {"op", "handle"}:
                raise ValueError("unexpected discard fields")
            self.pending = None
            return {"discarded": handle}
        if set(message) != {"op", "handle", "input_tokens", "max_new_tokens"}:
            raise ValueError("generate requires the prepared quote")
        if (type(message["input_tokens"]) is not int or type(message["max_new_tokens"]) is not int
                or message["input_tokens"] != inputs["input_ids"].shape[1]
                or message["max_new_tokens"] != generation["max_new_tokens"]):
            raise ValueError("generation quote differs from prepared request")
        # Consume before model I/O: an exception cannot authorize a duplicate send.
        self.pending = None
        return self.generate(inputs, generation)


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
    eos_ids = model.generation_config.eos_token_id
    eos_ids = {eos_ids} if isinstance(eos_ids, int) else set(eos_ids or [])
    emit({
        "ready": True, "model": model_id, "revision": revision, "torch": str(torch.__version__),
        "transformers": transformers.__version__, "threads": torch.get_num_threads(),
        "load_us": (time.perf_counter_ns() - start) // 1000,
    })

    def tokenize(request):
        if (not isinstance(request, dict) or request.get("adapter") != "transformers-cpu/v1"
                or request.get("model") != model_id or request.get("revision") != revision
                or request.get("dtype") != "bfloat16" or request.get("add_generation_prompt") is not True):
            raise ValueError("model worker received an incompatible request")
        generation = request["generation"]
        if (set(generation) != {"do_sample", "num_beams", "max_new_tokens"}
                or generation["do_sample"] is not False or type(generation["num_beams"]) is not int
                or generation["num_beams"] != 1 or type(generation["max_new_tokens"]) is not int
                or not 1 <= generation["max_new_tokens"] <= 96):
            raise ValueError("unexpected decoding parameters")
        inputs = tokenizer.apply_chat_template(
            request["messages"], tokenize=True, add_generation_prompt=True,
            return_tensors="pt", return_dict=True,
        )
        return inputs, generation

    def generate(inputs, generation):
        start = time.perf_counter_ns()
        with torch.inference_mode():
            ids = model.generate(**inputs, **generation)
        elapsed_us = (time.perf_counter_ns() - start) // 1000
        input_length = inputs["input_ids"].shape[1]
        completion = ids[0, input_length:]
        length = completion.numel()
        cap = generation["max_new_tokens"]
        if length > cap:
            raise RuntimeError("generation exceeded its output cap")
        # An EOS on the last allowed token sets both flags. Text decoding omits
        # special tokens, but usage includes every generated ID including EOS.
        return {
            "text": tokenizer.decode(completion, skip_special_tokens=True),
            "input_tokens": input_length, "output_tokens": length,
            "generation_us": elapsed_us, "hit_cap": length == cap,
            "ended_eos": length > 0 and completion[-1].item() in eos_ids,
        }

    worker = RequestHandler(tokenize, generate)
    for line in sys.stdin.buffer:
        try:
            command = json.loads(line)
            if not isinstance(command, dict):
                raise ValueError("worker command must be an object")
            emit(worker.handle(command))
        except (ValueError, KeyError, TypeError) as error:
            emit({"error": str(error)})


if __name__ == "__main__":
    main()
