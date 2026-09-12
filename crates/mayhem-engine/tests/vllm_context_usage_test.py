"""Context admission/telemetry regressions without loading GPU dependencies."""
import ast
import asyncio
import pathlib
import unittest

from worker_cancellation_test import load_cancellation_scope


WORKER_PATH = pathlib.Path(__file__).resolve().parents[1] / "src" / "vllm_worker.py"


def load_context_scope():
    scope = load_cancellation_scope("vllm_worker.py")
    names = {"prepare_generation_request", "multimodal_engine_prompt", "render_chat_template"}
    tree = ast.parse(WORKER_PATH.read_text())
    nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(WORKER_PATH), "exec"), scope)
    scope.update(
        engine=object(), ctx_size=64,
        request_max_tokens=lambda payload: payload.get("max_new_tokens", 8),
        speciality_maps=lambda payload: ({"preserve_thinking": True}, {}, []),
        decode_multimodal_data=lambda payload: {},
        encode_text=lambda text: list(text.encode()),
        make_sampling_params=lambda payload, kwargs: object(),
        reasoning_enabled=lambda payload: True,
        required_call_kwargs=lambda function, kwargs, required, label: kwargs,
    )
    return scope


class ContextAdmissionTests(unittest.IsolatedAsyncioTestCase):
    async def test_rendered_tools_and_reasoning_exhaustion_is_structured_and_request_scoped(self):
        scope = load_context_scope()

        class Renderer:
            def apply_chat_template(self, messages, **kwargs):
                self.kwargs = kwargs
                return messages[0]["content"] + messages[0]["reasoning_content"] + kwargs["tools"][0]["schema"]

        renderer = Renderer()
        scope["tokenizer"] = renderer
        payload = {"prompt": "short", "messages": [{"content": "hi", "reasoning_content": "r" * 32}],
                   "tools": [{"schema": "t" * 32}], "max_new_tokens": 8}
        with self.assertRaises(scope["PromptTooLong"]) as failure:
            scope["prepare_generation_request"](1, payload)
        self.assertEqual(failure.exception.prompt_tokens, 66)
        self.assertTrue(renderer.kwargs["preserve_thinking"])
        messages = []

        async def generate(request_id, request):
            return scope["prepare_generation_request"](request_id, request)

        async def abort(request_id):
            pass

        mux = scope["GenerationMultiplexer"](1, generate, abort, messages.append, lambda request_id: None)
        mux.submit(2, payload)
        await mux.drain()
        self.assertEqual(messages[0]["error_code"], "context_length_exceeded")
        self.assertEqual(messages[0]["prompt_tokens"], 66)
        self.assertEqual(messages[0]["ctx_size"], 64)
        # Compaction succeeds on the same worker; exhaustion is not engine death.
        payload["messages"][0]["reasoning_content"] = ""
        mux.submit(3, payload)
        await mux.drain()
        self.assertTrue(messages[1]["ok"])

    def test_multimodal_expansion_overflow_keeps_the_actual_engine_counts(self):
        scope = load_context_scope()
        fields = scope["request_error_fields"](ValueError(
            "The decoder prompt (length 9000) is longer than the maximum model length of 8192. "
            "Make sure that max_model_len is no smaller than text plus multimodal tokens."))
        self.assertEqual(fields, {"error_code": "context_length_exceeded", "prompt_tokens": 9000, "ctx_size": 8192})
        fields = scope["request_error_fields"](ValueError(
            "The decoder prompt (length 8192) plus the number of requested output tokens (at least 1) "
            "is longer than the maximum model length of 8192."))
        self.assertEqual(fields["prompt_tokens"], 8192)
        self.assertEqual(scope["request_error_fields"](ValueError(
            "The encoder prompt (length 9000) is longer than the maximum model length of 8192.")), {})

    def test_unrelated_worker_failure_is_not_a_context_error(self):
        scope = load_context_scope()
        self.assertEqual(scope["request_error_fields"](RuntimeError("CUDA failure")), {})


if __name__ == "__main__":
    unittest.main()
