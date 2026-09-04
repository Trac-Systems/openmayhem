import ast
import inspect
import pathlib
import sys
import types
import unittest


ENGINE_SRC = pathlib.Path(__file__).resolve().parents[1] / "src"


def load_functions(worker_name, function_names):
    worker_path = ENGINE_SRC / worker_name
    tree = ast.parse(worker_path.read_text(), worker_path.name)
    retained = [
        node
        for node in tree.body
        if isinstance(node, ast.FunctionDef) and node.name in function_names
    ]
    namespace = {"inspect": inspect}
    exec(
        compile(ast.Module(body=retained, type_ignores=[]), worker_path.name, "exec"),
        namespace,
    )
    return namespace


def tools():
    return [
        {
            "name": "lookup",
            "parameters": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": False,
            },
        },
        {
            "name": "quote",
            "parameters": {
                "type": "object",
                "properties": {"symbol": {"type": "string"}},
                "required": ["symbol"],
                "additionalProperties": False,
            },
        },
    ]


class ToolCallWorkerSchemaTests(unittest.TestCase):
    def test_workers_retain_each_tool_parameter_schema(self):
        for worker_name in ("vllm_worker.py", "mlx_worker.py"):
            with self.subTest(worker=worker_name):
                worker = load_functions(worker_name, {"tool_call_schema"})
                schema = worker["tool_call_schema"](tools())

                self.assertEqual(len(schema["oneOf"]), 2)
                for branch, tool in zip(schema["oneOf"], tools()):
                    self.assertEqual(
                        branch["properties"]["tool"], {"const": tool["name"]}
                    )
                    self.assertEqual(
                        branch["properties"]["arguments"], tool["parameters"]
                    )

    def test_vllm_guided_payload_retains_parameter_schemas(self):
        worker = load_functions(
            "vllm_worker.py",
            {"accepted_kwargs", "make_guided_params", "tool_call_schema"},
        )

        class StructuredOutputsParams:
            def __init__(self, json):
                self.json = json

        params = worker["make_guided_params"](
            StructuredOutputsParams,
            {"kind": "tool_call", "tools": tools()},
        )
        self.assertEqual(
            params.json["oneOf"][1]["properties"]["arguments"],
            tools()[1]["parameters"],
        )

    def test_vllm_guided_payload_rejects_a_dropped_schema(self):
        worker = load_functions(
            "vllm_worker.py",
            {"accepted_kwargs", "make_guided_params", "tool_call_schema"},
        )

        class UnsupportedStructuredOutputsParams:
            def __init__(self):
                pass

        with self.assertRaisesRegex(ValueError, "does not accept 'json'"):
            worker["make_guided_params"](
                UnsupportedStructuredOutputsParams,
                {"kind": "tool_call", "tools": tools()},
            )

    def test_mlx_logits_processor_retains_parameter_schemas(self):
        worker = load_functions(
            "mlx_worker.py",
            {"reasoning_enabled", "structured_logits_processor", "tool_call_schema"},
        )
        captured = {}
        structured = types.ModuleType("mlx_vlm.structured")
        structured.build_json_schema_logits_processor = (
            lambda tokenizer, schema: captured.update(
                {"tokenizer": tokenizer, "schema": schema}
            )
            or "processor"
        )
        structured.ThinkingAwareLogitsProcessor = lambda *args: args
        mlx_vlm = types.ModuleType("mlx_vlm")
        mlx_vlm.__path__ = []
        worker["tokenizer"] = object()
        previous_mlx_vlm = sys.modules.get("mlx_vlm")
        previous_structured = sys.modules.get("mlx_vlm.structured")
        sys.modules["mlx_vlm"] = mlx_vlm
        sys.modules["mlx_vlm.structured"] = structured
        try:
            processor = worker["structured_logits_processor"](
                {"grammar": {"kind": "tool_call", "tools": tools()}}
            )
        finally:
            if previous_mlx_vlm is None:
                sys.modules.pop("mlx_vlm", None)
            else:
                sys.modules["mlx_vlm"] = previous_mlx_vlm
            if previous_structured is None:
                sys.modules.pop("mlx_vlm.structured", None)
            else:
                sys.modules["mlx_vlm.structured"] = previous_structured

        self.assertEqual(processor, "processor")
        self.assertEqual(
            captured["schema"]["oneOf"][0]["properties"]["arguments"],
            tools()[0]["parameters"],
        )


if __name__ == "__main__":
    unittest.main()
