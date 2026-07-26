import contextlib
import importlib.util
import io
import pathlib
import types
import unittest
from unittest import mock


def load_worker():
    worker_path = pathlib.Path(__file__).parents[1] / "src" / "needle_worker.py"
    spec = importlib.util.spec_from_file_location("needle_worker_under_test", worker_path)
    worker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(worker)
    return worker


def openai_tool(name="get-weather"):
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": "Get weather",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": {"type": "string", "minLength": 1},
                    "days": {"type": "integer", "minimum": 1},
                },
                "required": ["city"],
                "additionalProperties": False,
            },
        },
    }


def generate_payload(tools=None, parallel_tool_calls=None):
    return {
        "frequency_penalty": None,
        "ignore_eos": False,
        "max_new_tokens": 512,
        "min_p": None,
        "parallel_tool_calls": parallel_tool_calls,
        "presence_penalty": None,
        "prompt": "Weather in Berlin and Paris",
        "repeat_penalty": None,
        "seed": 7,
        "stop": [],
        "temperature": 0.0,
        "tools": [openai_tool()] if tools is None else tools,
        "top_k": 1,
        "top_p": 1.0,
    }


class FakeTokenizer:
    tools_token_id = 5
    eos_token_id = 1

    def encode(self, text, add_special_tokens=False):
        del add_special_tokens
        return [10] if text == "Weather in Berlin and Paris" else [11]

    def decode(self, tokens, skip_special_tokens=False):
        del tokens, skip_special_tokens
        return (
            '<tool_call>[{"name":"get_weather","arguments":{"city":"Berlin"}},'
            '{"name":"get_weather","arguments":{"city":"Paris","days":2}}]'
        )


class FakeTensor:
    def __init__(self, values):
        self.values = values
        if values and isinstance(values[0], list):
            self.shape = (len(values), len(values[0]))
        else:
            self.shape = (len(values),)

    def unsqueeze(self, dimension):
        del dimension
        return self


class FakeLogits:
    def __init__(self, token):
        self.token = token

    def __getitem__(self, item):
        del item
        return self.token


class FakeModel:
    def __init__(self):
        self.decoder_steps = 0

    def cactus_source_encode(self, input_ids, attention_mask):
        del input_ids, attention_mask
        return object(), object()

    def cactus_decoder_cross_kv(self, encoder_hidden, encoder_mask):
        del encoder_hidden, encoder_mask
        return (object(), object())

    def cactus_decoder_step(
        self,
        decoder_ids,
        position_ids,
        encoder_mask,
        *cross_kv,
    ):
        del position_ids, encoder_mask, cross_kv
        self.decoder_steps += 1
        generated_count = decoder_ids.shape[1] - 1
        return FakeLogits([20, 21, 1][generated_count])


class FakeTorch:
    long = object()

    def __init__(self):
        self.cuda = types.SimpleNamespace(
            synchronize=lambda: None,
            manual_seed_all=lambda seed: None,
        )
        self.mps = types.SimpleNamespace(synchronize=lambda: None)

    def manual_seed(self, seed):
        del seed

    def tensor(self, values, dtype=None, device=None):
        del dtype, device
        return FakeTensor(values)

    def ones_like(self, value):
        return value

    def arange(self, length, dtype=None, device=None):
        del dtype, device
        return FakeTensor(list(range(length)))

    def argmax(self, value):
        return types.SimpleNamespace(item=lambda: value)

    def inference_mode(self):
        return contextlib.nullcontext()


class NeedleWorkerTest(unittest.TestCase):
    def setUp(self):
        self.worker = load_worker()

    def test_tools_only_and_normalized_name_collisions_are_rejected(self):
        with self.assertRaisesRegex(self.worker.ProtocolError, "tools-only"):
            self.worker.normalize_openai_tools([])
        with self.assertRaisesRegex(self.worker.ProtocolError, "collide"):
            self.worker.normalize_openai_tools(
                [openai_tool("foo-bar"), openai_tool("foo.bar")]
            )
        with self.assertRaisesRegex(self.worker.ProtocolError, "at most 10 tools"):
            self.worker.normalize_openai_tools(
                [openai_tool(f"tool-{index}") for index in range(11)]
            )

    def test_openai_tools_are_normalized_to_upstream_schema(self):
        source = openai_tool("getWeather")
        tools, names = self.worker.normalize_openai_tools([source])
        self.assertEqual(tools[0]["name"], "get_weather")
        self.assertTrue(tools[0]["parameters"]["city"]["required"])
        self.assertFalse(tools[0]["parameters"]["days"]["required"])
        self.assertEqual(tools[0]["parameters"]["days"]["type"], "number")
        self.assertNotIn("minimum", tools[0]["parameters"]["days"])
        self.assertNotIn("json_schema", tools[0])
        self.assertEqual(names["get_weather"]["name"], "getWeather")

    def test_every_unsupported_schema_keyword_is_rejected_before_generation(self):
        for keyword, value in (
            ("dependentRequired", {"city": ["days"]}),
            ("format", "email"),
            ("pattern", "(a+)+$"),
            ("prefixItems", [{"type": "string"}]),
            ("propertyNames", {"type": "string"}),
        ):
            with self.subTest(keyword=keyword):
                tool = openai_tool()
                tool["function"]["parameters"]["properties"]["city"][keyword] = value
                with self.assertRaisesRegex(
                    self.worker.ProtocolError,
                    "unsupported JSON Schema keywords",
                ):
                    self.worker.normalize_openai_tools([tool])

    def test_recursive_local_schema_references_are_rejected(self):
        tool = openai_tool()
        tool["function"]["parameters"] = {
            "$defs": {
                "node": {
                    "type": "object",
                    "properties": {
                        "next": {"$ref": "#/$defs/node"},
                    },
                },
            },
            "type": "object",
            "properties": {
                "node": {"$ref": "#/$defs/node"},
            },
        }
        with self.assertRaisesRegex(
            self.worker.ProtocolError,
            "recursive JSON Schema reference",
        ):
            self.worker.normalize_openai_tools([tool])

    def test_nested_object_array_constraints_are_retained_and_enforced(self):
        tool = openai_tool("run-jobs")
        tool["function"]["parameters"] = {
            "$defs": {
                "job": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "minLength": 2},
                        "retries": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 3,
                        },
                        "tags": {
                            "type": "array",
                            "items": {"enum": ["fast", "safe"], "type": "string"},
                            "maxItems": 2,
                            "uniqueItems": True,
                        },
                    },
                    "required": ["name"],
                    "additionalProperties": False,
                },
            },
            "type": "object",
            "properties": {
                "jobs": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/job"},
                    "minItems": 1,
                    "maxItems": 3,
                },
            },
            "required": ["jobs"],
            "additionalProperties": False,
        }
        tools, names = self.worker.normalize_openai_tools([tool])
        self.assertEqual(
            tools[0]["json_schema"],
            tool["function"]["parameters"],
        )
        _, tools_json = self.worker.build_encoder_tokens(
            FakeTokenizer(),
            "Weather in Berlin and Paris",
            tools,
        )
        self.assertIn('"json_schema"', tools_json)
        self.assertIn('"maxItems":3', tools_json)
        calls = self.worker.parse_and_validate_calls(
            (
                '[{"name":"run_jobs","arguments":{"jobs":'
                '[{"name":"one","retries":1,"tags":["fast"]}]}}'
                ',{"name":"run_jobs","arguments":{"jobs":'
                '[{"name":"two","tags":["safe"]}]}}]'
            ),
            names,
        )
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["arguments"]["jobs"][0]["retries"], 1)
        with self.assertRaisesRegex(self.worker.ProtocolError, "violates maximum"):
            self.worker.parse_and_validate_calls(
                (
                    '[{"name":"run_jobs","arguments":{"jobs":'
                    '[{"name":"one","retries":4}]}}]'
                ),
                names,
            )

    def test_branch_validation_has_one_global_deterministic_work_budget(self):
        tool = openai_tool()
        tool["function"]["parameters"]["properties"]["city"] = {
            "anyOf": [{"type": "integer"} for _ in range(16)],
        }
        _, names = self.worker.normalize_openai_tools([tool])
        with mock.patch.object(self.worker, "MAX_SCHEMA_VALIDATION_WORK", 8):
            with self.assertRaisesRegex(
                self.worker.ProtocolError,
                "deterministic work budget of 8",
            ):
                self.worker.parse_and_validate_calls(
                    '[{"name":"get_weather","arguments":{"city":"Berlin"}}]',
                    names,
                )

    def test_json_schema_numeric_equality_matches_json_schema_semantics(self):
        budget = self.worker._SchemaWorkBudget(64, "numeric equality")
        self.assertTrue(self.worker._json_equal(1, 1.0, budget))
        self.assertFalse(self.worker._json_equal(True, 1, budget))
        self.assertTrue(
            self.worker._json_equal(
                {"outer": [1, {"inner": 2}]},
                {"outer": [1.0, {"inner": 2.0}]},
                budget,
            )
        )

        self.worker.validate_json_schema(1.0, {"type": "integer"})
        self.worker.validate_json_schema(
            {"outer": [1.0, {"inner": 2.0}]},
            {"const": {"outer": [1, {"inner": 2}]}},
        )
        self.worker.validate_json_schema(
            {"outer": [1.0, {"inner": 2.0}]},
            {"enum": [{"outer": [1, {"inner": 2}]}]},
        )
        with self.assertRaisesRegex(self.worker.ProtocolError, "duplicate items"):
            self.worker.validate_json_schema(
                [1, 1.0],
                {"type": "array", "uniqueItems": True},
            )
        with self.assertRaisesRegex(self.worker.ProtocolError, "not one of"):
            self.worker.validate_json_schema(True, {"enum": [1]})

    def test_numeric_equality_uses_the_global_validation_work_budget(self):
        with mock.patch.object(self.worker, "MAX_SCHEMA_VALIDATION_WORK", 4):
            with self.assertRaisesRegex(
                self.worker.ProtocolError,
                "deterministic work budget of 4",
            ):
                self.worker.validate_json_schema(
                    {"nested": [1, 2, 3]},
                    {"const": {"nested": [1.0, 2.0, 3.0]}},
                )

    def test_schema_enum_rejects_numerically_duplicate_values(self):
        tool = openai_tool()
        tool["function"]["parameters"]["properties"]["days"]["enum"] = [1, 1.0]
        with self.assertRaisesRegex(self.worker.ProtocolError, "enum values must be unique"):
            self.worker.normalize_openai_tools([tool])

    def test_combined_encoder_limit_rejects_instead_of_truncating(self):
        class LargeTokenizer(FakeTokenizer):
            def encode(self, text, add_special_tokens=False):
                del text, add_special_tokens
                return list(range(512))

        with self.assertRaisesRegex(
            self.worker.ProtocolError,
            "1025 encoder tokens.*not truncated",
        ):
            self.worker.build_encoder_tokens(
                LargeTokenizer(),
                "query",
                [{"name": "tool", "description": "", "parameters": {}}],
            )

    def test_multiple_calls_are_preserved_and_schema_validated(self):
        _, names = self.worker.normalize_openai_tools([openai_tool()])
        calls = self.worker.parse_and_validate_calls(
            (
                '[{"name":"get_weather","arguments":{"city":"Berlin"}},'
                '{"name":"get_weather","arguments":{"city":"Paris","days":2}}]'
            ),
            names,
        )
        self.assertEqual([call["name"] for call in calls], ["get-weather", "get-weather"])
        self.assertEqual(calls[1]["arguments"]["days"], 2)

        with self.assertRaisesRegex(self.worker.ProtocolError, "additional property"):
            self.worker.parse_and_validate_calls(
                '[{"name":"get_weather","arguments":{"city":"Berlin","bogus":1}}]',
                names,
            )

    def test_non_greedy_controls_are_rejected(self):
        payload = generate_payload()
        payload["temperature"] = 0.2
        with self.assertRaisesRegex(self.worker.ProtocolError, "greedy"):
            self.worker._validate_generate_payload(payload)
        payload = generate_payload()
        payload["max_new_tokens"] = 513
        with self.assertRaisesRegex(self.worker.ProtocolError, "outside"):
            self.worker._validate_generate_payload(payload)
        payload = generate_payload()
        payload["seed"] = 1 << 32
        with self.assertRaisesRegex(self.worker.ProtocolError, "outside"):
            self.worker._validate_generate_payload(payload)

    def test_cuda_determinism_requires_the_pinned_cublas_workspace(self):
        torch = types.SimpleNamespace(
            set_grad_enabled=lambda enabled: None,
            manual_seed=lambda seed: None,
            use_deterministic_algorithms=lambda enabled: None,
            cuda=types.SimpleNamespace(manual_seed_all=lambda seed: None),
            backends=types.SimpleNamespace(
                cuda=types.SimpleNamespace(
                    matmul=types.SimpleNamespace(allow_tf32=True)
                ),
                cudnn=types.SimpleNamespace(
                    allow_tf32=True,
                    benchmark=True,
                    deterministic=False,
                ),
            ),
        )
        with mock.patch.dict(self.worker.os.environ, {}, clear=True):
            with self.assertRaisesRegex(
                self.worker.ProtocolError,
                "CUBLAS_WORKSPACE_CONFIG=:4096:8",
            ):
                self.worker._configure_determinism(torch, "cuda")
        with mock.patch.dict(
            self.worker.os.environ,
            {"CUBLAS_WORKSPACE_CONFIG": ":4096:8"},
            clear=True,
        ):
            self.worker._configure_determinism(torch, "cuda")
        self.assertFalse(torch.backends.cuda.matmul.allow_tf32)
        self.assertFalse(torch.backends.cudnn.allow_tf32)
        self.assertFalse(torch.backends.cudnn.benchmark)
        self.assertTrue(torch.backends.cudnn.deterministic)

    def test_generate_reports_phase_separated_timing_and_token_counts(self):
        self.worker._runtime = {
            "device": "cpu",
            "model": FakeModel(),
            "tokenizer": FakeTokenizer(),
            "torch": FakeTorch(),
        }
        with mock.patch.object(
            self.worker.time,
            "perf_counter",
            side_effect=[10.0, 12.0, 13.0, 16.0],
        ):
            result = self.worker._handle_generate(generate_payload())

        self.assertEqual(result["prompt_tokens"], 3)
        self.assertEqual(result["completion_tokens"], 2)
        self.assertEqual(result["prompt_eval_ms"], 2_000.0)
        self.assertEqual(result["generation_ms"], 4_000.0)
        self.assertEqual(result["prefill_tokens_per_second"], 1.5)
        self.assertEqual(result["output_tokens_per_second"], 0.5)
        self.assertEqual(result["time_to_first_token_ms"], 3_000.0)
        self.assertEqual(result["finish_reason"], "stop")
        self.assertEqual(result["output_text"].count('"name":"get-weather"'), 2)

    def test_non_parallel_generation_stops_after_first_complete_valid_call(self):
        model = FakeModel()
        self.worker._runtime = {
            "device": "cpu",
            "model": model,
            "tokenizer": FakeTokenizer(),
            "torch": FakeTorch(),
        }
        with mock.patch.object(
            self.worker.time,
            "perf_counter",
            side_effect=[10.0, 12.0, 13.0, 14.0],
        ):
            result = self.worker._handle_generate(
                generate_payload(parallel_tool_calls=False)
            )

        self.assertEqual(model.decoder_steps, 1)
        self.assertEqual(result["completion_tokens"], 1)
        self.assertEqual(result["finish_reason"], "stop")
        self.assertEqual(result["output_text"].count('"name":"get-weather"'), 1)
        calls = self.worker.json.loads(result["output_text"])
        self.assertEqual(calls, [{"arguments": {"city": "Berlin"}, "name": "get-weather"}])

    def test_explicit_parallel_generation_preserves_all_calls(self):
        model = FakeModel()
        self.worker._runtime = {
            "device": "cpu",
            "model": model,
            "tokenizer": FakeTokenizer(),
            "torch": FakeTorch(),
        }
        with mock.patch.object(
            self.worker.time,
            "perf_counter",
            side_effect=[10.0, 12.0, 13.0, 16.0],
        ):
            result = self.worker._handle_generate(
                generate_payload(parallel_tool_calls=True)
            )

        self.assertEqual(model.decoder_steps, 3)
        self.assertEqual(result["output_text"].count('"name":"get-weather"'), 2)

    def test_request_and_model_output_failures_have_distinct_classes(self):
        self.worker._runtime = {
            "device": "cpu",
            "model": FakeModel(),
            "tokenizer": FakeTokenizer(),
            "torch": FakeTorch(),
        }
        with self.assertRaises(self.worker.RequestProtocolError):
            self.worker._handle_generate(generate_payload(tools=[]))

        with mock.patch.object(
            self.worker,
            "parse_and_validate_calls",
            side_effect=self.worker.ProtocolError("bad model output"),
        ):
            with self.assertRaises(self.worker.OutputProtocolError):
                self.worker._handle_generate(generate_payload())

    def test_protocol_is_exact_and_output_is_canonical(self):
        with self.assertRaisesRegex(self.worker.ProtocolError, "fields must be exactly"):
            self.worker._handle_message(
                {"id": 1, "op": "shutdown", "payload": None, "extra": True}
            )
        output = io.StringIO()
        with mock.patch.object(self.worker.sys, "stdout", output):
            self.worker._emit({"z": 1, "a": 2})
        self.assertEqual(output.getvalue(), '{"a":2,"z":1}\n')

    def test_worker_has_no_remote_code_or_network_escape(self):
        source = pathlib.Path(self.worker.__file__).read_text(encoding="utf-8")
        self.assertNotIn("trust_remote_code", source)
        self.assertNotIn("from_pretrained(\"http", source)
        self.assertNotIn("requests.", source)
        self.assertNotIn("urllib.", source)
        self.assertIn("local_files_only=True", source)


if __name__ == "__main__":
    unittest.main()
