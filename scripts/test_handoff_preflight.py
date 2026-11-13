"""Worker protocol tests with a counting tokenizer and no model or network."""
import json
import unittest
from types import SimpleNamespace

from handoff_model import RequestHandler


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.renders = []
        self.generations = []

        def tokenize(request):
            tensor = {"input_ids": SimpleNamespace(shape=(1, 181))}
            self.renders.append(tensor)
            return tensor, request["generation"]

        def generate(inputs, generation):
            self.generations.append((inputs, generation))
            return {"text": "result"}

        self.worker = RequestHandler(tokenize, generate)

    def prepare(self, cap=96):
        frozen = json.dumps({"generation": {"max_new_tokens": cap}})
        return self.worker.handle({"op": "prepare", "request_utf8": frozen})

    def test_quote_is_exact_and_bound_to_the_retained_generation_input(self):
        quote = self.prepare(95)
        self.assertEqual(quote["input_tokens"], 181)
        self.assertEqual(quote["max_new_tokens"], 95)
        self.assertEqual(len(self.renders), 1)
        self.assertEqual(self.generations, [])
        for key, different in [("input_tokens", 180), ("max_new_tokens", 96), ("handle", "wrong")]:
            with self.assertRaises(ValueError):
                self.worker.handle(dict(quote, op="generate", **{key: different}))
        self.assertEqual(self.worker.handle(dict(quote, op="generate")), {"text": "result"})
        self.assertEqual(len(self.renders), 1)
        self.assertIs(self.generations[0][0], self.renders[0])
        self.assertEqual(self.generations[0][1], {"max_new_tokens": 95})
        with self.assertRaises(ValueError):
            self.worker.handle(dict(quote, op="generate"))
        self.assertEqual(len(self.generations), 1)

    def test_discard_frees_slot_and_does_no_model_work(self):
        quote = self.prepare()
        with self.assertRaises(ValueError):
            self.prepare()
        with self.assertRaises(ValueError):
            self.worker.handle({"generation": {"max_new_tokens": 96}})
        self.assertEqual(self.worker.handle({"op": "discard", "handle": quote["handle"]}),
                         {"discarded": quote["handle"]})
        next_quote = self.prepare(32)
        self.assertNotEqual(quote["handle"], next_quote["handle"])
        with self.assertRaises(ValueError):
            self.worker.handle(dict(quote, op="generate"))
        self.assertEqual(self.generations, [])

    def test_failed_generation_consumes_handle(self):
        quote = self.prepare()
        def fail(*args):
            raise RuntimeError("example transport failure")
        self.worker.generate = fail
        with self.assertRaises(RuntimeError):
            self.worker.handle(dict(quote, op="generate"))
        with self.assertRaises(ValueError):
            self.worker.handle(dict(quote, op="generate"))
        self.prepare()

    def test_legacy_direct_request_remains_one_shot(self):
        self.worker.handle({"generation": {"max_new_tokens": 96}})
        self.assertEqual(len(self.renders), 1)
        self.assertEqual(len(self.generations), 1)
        with self.assertRaises(ValueError):
            self.worker.handle({"generation": {"max_new_tokens": 32}})


if __name__ == "__main__":
    unittest.main()
