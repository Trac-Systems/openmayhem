"""CPU-only worker observation regressions; no vLLM or model imports."""

import ast
import asyncio
import copy
import inspect
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from mayhem_vllm_execution_probe import MayhemExecutionProbe


def worker_namespace():
    tree = ast.parse(Path(__file__).with_name("vllm_worker.py").read_text())
    nodes = [node for node in tree.body if isinstance(
        node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
    ) or (isinstance(node, ast.Assign) and any(
        isinstance(target, ast.Name) and target.id.startswith("MAX_")
        for target in node.targets
    ))]
    namespace = {"asyncio": asyncio, "inspect": inspect, "copy": copy}
    exec(compile(ast.Module(body=nodes, type_ignores=[]), "vllm_worker.py", "exec"), namespace)
    return namespace


def rank(index=0, mode=0, graphs="FULL_DECODE_ONLY", world_size=1):
    return {"rank": index, "local_rank": index, "world_size": world_size, "pid": 100 + index,
            "compilation_mode": mode, "cudagraph_mode": graphs}


class ObservationTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.ns = worker_namespace()
        self.rows = [rank()]
        self.instances = []
        self.rpc_calls = []
        self.shutdowns = []
        self.shutdown_error = False
        self.shutdown_async = False
        self.shutdown_hangs = False
        self.rpc_hangs = False
        self.rpc_cancelled = False
        self.missing_rpc = False
        self.world_size = None
        self.dimensions = {"pipeline_parallel_size": 1, "data_parallel_size": 1,
                           "prefill_context_parallel_size": 1}
        self.received = []
        self.profile = {"path": "/unused", "vllm_enforce_eager": False,
                        "vllm_compilation_mode": 0, "vllm_cudagraph_mode": "FULL_DECODE_ONLY"}
        test = self

        class Args:
            def __init__(self, **kwargs):
                test.received.append(copy.deepcopy(kwargs))
                self.__dict__.update(kwargs)

        class Engine:
            def __init__(self, args):
                self.vllm_config = {
                    "model_config": {name: getattr(args, name) for name in
                                     ("enforce_eager", "seed", "use_fp64_gumbel")},
                    "scheduler_config": {"async_scheduling": args.async_scheduling},
                    "kernel_config": {}, "cache_config": {},
                    "compilation_config": getattr(args, "compilation_config", {}),
                    "parallel_config": {"world_size": test.world_size or args.tensor_parallel_size,
                                        "tensor_parallel_size": args.tensor_parallel_size,
                                        **test.dimensions},
                }
                test.instances.append(self)
                if test.missing_rpc:
                    self.collective_rpc = None

            async def collective_rpc(self, method, timeout=None):
                test.assertIsNone(test.ns["execution_properties"])
                test.rpc_calls.append((method, timeout))
                if test.rpc_hangs:
                    try:
                        await asyncio.Event().wait()
                    finally:
                        test.rpc_cancelled = True
                return copy.deepcopy(test.rows)

            def shutdown(self, timeout=None):
                test.shutdowns.append((self, timeout))
                if test.shutdown_error:
                    raise RuntimeError("shutdown failed")
                if test.shutdown_async:
                    async def close():
                        if test.shutdown_hangs:
                            await asyncio.Event().wait()
                    return close()

        self.ns.update({
            "import_attr": lambda candidates: Args if candidates[0][1] == "AsyncEngineArgs" else Engine,
            "configure_deterministic_runtime": lambda path: None,
            "model_uses_nvfp4": lambda path: False,
            "get_tokenizer": lambda: None,
            "model_ctx": lambda size: size,
            "vocab_size": lambda: 100,
            "runtime_kv_cache_info": lambda: {"size_tokens": 262144, "max_concurrency": 1},
            "GenerationMultiplexer": lambda *args, **kwargs: object(),
            "send": lambda message: None, "finish_request": lambda request_id: None,
            "check_cancelled": lambda request_id: None,
            "engine": None, "tokenizer": None, "processor": None,
            "generation_multiplexer": None, "execution_properties": None,
            "engine_health_monitor": None,
            "batch_invariant": False, "kernel_policy": "auto",
        })
        self.addAsyncCleanup(self.ns["stop_engine_health_monitor"])
        auto = SimpleNamespace(from_pretrained=lambda *args, **kwargs: None)
        self.transformers = patch.dict(sys.modules, {"transformers": SimpleNamespace(
            AutoTokenizer=auto, AutoProcessor=auto
        )})
        self.transformers.start()
        self.addCleanup(self.transformers.stop)

    async def load(self, payload=None):
        return await self.ns["handle_load"](self.profile if payload is None else payload)

    async def rejection(self, pattern):
        with self.assertRaisesRegex(Exception, pattern):
            await self.load()
        self.assertIsNone(self.ns["engine"])
        self.assertIsNone(self.ns["execution_properties"])
        self.assertIsNone(self.ns["generation_multiplexer"])
        self.assertEqual(len(self.shutdowns), len(self.instances))

    async def test_frontend_full_worker_none_rejected_and_shutdown(self):
        self.rows = [rank(graphs="NONE")]
        await self.rejection("worker execution mismatch")
        self.assertEqual(self.instances[0].vllm_config["compilation_config"]["cudagraph_mode"], "FULL_DECODE_ONLY")

    async def test_matching_all_ranks_are_awaited_and_reported(self):
        self.profile["tensor_parallel"] = 2
        self.rows = [rank(1, world_size=2), rank(0, world_size=2)]
        result = await self.load()
        evidence = result["execution"]["worker_execution_observation"]
        self.assertEqual(evidence["rank_count"], 2)
        self.assertEqual(evidence["ranks"], [rank(0, world_size=2), rank(1, world_size=2)])
        self.assertEqual(self.rpc_calls, [("mayhem_execution_snapshot", 10.0)])
        self.assertEqual(self.received[0]["worker_extension_cls"],
                         "mayhem_vllm_execution_probe.MayhemExecutionProbe")
        self.assertEqual(self.shutdowns, [])

    async def test_missing_and_duplicate_ranks_rejected(self):
        self.profile["tensor_parallel"] = 2
        for rows, pattern in [([rank()], "exactly 2"), ([rank(), rank()], "duplicate"),
                              ([rank(), rank(2)], "out-of-range"),
                              ([rank(1), rank(2)], "out-of-range"),
                              ([rank(), {**rank(1), "rank": True}], "invalid rank")]:
            with self.subTest(rows=rows):
                self.rows = [{**item, "world_size": 2} for item in rows]
                await self.rejection(pattern)

    async def test_missing_compilation_fields_and_unknown_rank_fields_rejected(self):
        for value in [{**rank(), "compilation_mode": None},
                      {**rank(), "cudagraph_mode": None},
                      {**rank(), "unexpected": 1}, {**rank(), "pid": 0}]:
            self.rows = [value]
            await self.rejection("observation")

    async def test_unrequested_companion_must_agree_across_ranks(self):
        self.profile.pop("vllm_cudagraph_mode")
        self.profile["tensor_parallel"] = 2
        self.rows = [rank(world_size=2), rank(1, graphs="NONE", world_size=2)]
        await self.rejection("inconsistent")

    async def test_resolved_companion_is_reported_not_frontend_guess(self):
        self.profile.pop("vllm_cudagraph_mode")
        self.rows = [rank(graphs="NONE")]
        result = await self.load()
        self.assertEqual(result["execution"]["vllm_cudagraph_mode"], "NONE")

    async def test_rpc_timeout_cancels_call_and_shuts_down(self):
        self.rpc_hangs = True
        self.ns["MAX_EXECUTION_PROBE_SECONDS"] = 0.02
        await self.rejection("")
        self.assertTrue(self.rpc_cancelled)

    async def test_missing_named_rpc_is_rejected_and_shutdown(self):
        self.missing_rpc = True
        await self.rejection("named execution observation RPC")

    async def test_shutdown_failure_is_not_swallowed(self):
        self.rows = [rank(graphs="NONE")]
        self.shutdown_error = True
        await self.rejection("shutdown failed")

    async def test_async_shutdown_and_shutdown_timeout(self):
        self.rows = [rank(graphs="NONE")]
        self.shutdown_async = True
        await self.rejection("worker execution mismatch")
        self.shutdown_hangs = True
        self.ns["MAX_EXECUTION_PROBE_SECONDS"] = 0.02
        await self.rejection("shutdown failed")

    async def test_legacy_load_has_no_extension_or_rpc(self):
        self.rpc_hangs = True
        result = await self.load({"path": "/unused"})
        self.assertEqual(self.rpc_calls, [])
        self.assertNotIn("worker_extension_cls", self.received[0])
        self.assertNotIn("worker_execution_observation", result["execution"])

    async def test_control_response_never_emits_success_before_observation(self):
        self.rows = [rank(graphs="NONE")]
        messages = []
        self.ns["send"] = messages.append
        await self.ns["emit_control_response"]({"id": 1, "op": "load", "payload": self.profile})
        self.assertEqual(len(messages), 1)
        self.assertFalse(messages[0]["ok"])
        self.assertEqual(len(self.shutdowns), 1)

    async def test_non_tp_world_size_is_rejected(self):
        self.profile["tensor_parallel"] = 2
        self.world_size = 4
        self.rows = [rank(index, world_size=4) for index in range(4)]
        await self.rejection("rank geometry")

    async def test_unsupported_or_missing_parallel_dimensions_are_rejected(self):
        for name in self.dimensions:
            for value in (2, None, True):
                with self.subTest(name=name, value=value):
                    self.dimensions[name] = value
                    await self.rejection(name)
            self.dimensions[name] = 1

    async def test_worker_world_size_mismatch_is_rejected(self):
        self.rows = [rank(world_size=2)]
        await self.rejection("inconsistent world_size")

    def test_extension_reads_worker_config_and_does_not_mutate_it(self):
        config = SimpleNamespace(mode=SimpleNamespace(value=0),
                                 cudagraph_mode=SimpleNamespace(name="NONE"))
        worker = SimpleNamespace(rank=3, local_rank=1, parallel_config=SimpleNamespace(world_size=4),
                                 compilation_config=config)
        before = copy.deepcopy(worker)
        result = MayhemExecutionProbe.mayhem_execution_snapshot(worker)
        self.assertEqual(result["cudagraph_mode"], "NONE")
        self.assertEqual((result["rank"], result["local_rank"], result["world_size"]), (3, 1, 4))
        self.assertEqual(worker, before)


if __name__ == "__main__":
    unittest.main()
