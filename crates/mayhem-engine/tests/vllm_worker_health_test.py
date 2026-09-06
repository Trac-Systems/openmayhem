import ast
import asyncio
import inspect
import pathlib
import queue
import sys
import types
import unittest
from unittest.mock import patch

from worker_cancellation_test import load_cancellation_scope


WORKER_PATH = pathlib.Path(__file__).resolve().parents[1] / "src" / "vllm_worker.py"
FATAL = {"id": 0, "type": "fatal", "error": "vLLM engine health check failed"}


def load_health_scope():
    namespace = load_cancellation_scope("vllm_worker.py")
    tree = ast.parse(WORKER_PATH.read_text(), WORKER_PATH.name)
    names = {
        "EngineHealthMonitor",
        "stop_engine_health_monitor",
        "async_handle_generate",
        "handle_load",
        "handle",
        "emit_control_response",
        "run_worker",
    }
    retained = [
        node for node in tree.body
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))
        and node.name in names
    ]
    namespace.update(
        inspect=inspect,
        engine_health_monitor=None,
        generation_multiplexer=None,
        execution_properties=None,
        batch_invariant=False,
        kernel_policy="auto",
        request_queue=queue.Queue(),
    )
    exec(compile(ast.Module(body=retained, type_ignores=[]), WORKER_PATH.name, "exec"), namespace)
    return namespace


class FakeEngine:
    def __init__(self):
        self.is_running = True
        self.engine_core = types.SimpleNamespace(
            resources=types.SimpleNamespace(engine_dead=False)
        )
        self.polls = 0
        self.generations = 0

    @property
    def errored(self):
        self.polls += 1
        return self.engine_core.resources.engine_dead or not self.is_running

    def die(self):
        self.engine_core.resources.engine_dead = True

    async def check_health(self):
        raise AssertionError("health polling must not use RPC-capable fallbacks")

    async def generate(self, **kwargs):
        self.generations += 1
        yield types.SimpleNamespace(outputs=[], finished=True)


class VllmWorkerHealthTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.worker = load_health_scope()
        self.messages = []
        self.engine = FakeEngine()
        self.worker.update(engine=self.engine, send=self.messages.append)
        self.health = self.worker["EngineHealthMonitor"](
            self.engine, self.messages.append, interval=0.005
        )
        self.worker["engine_health_monitor"] = self.health

    async def asyncTearDown(self):
        await self.worker["stop_engine_health_monitor"]()
        await self.health.stop()

    async def wait_until(self, predicate):
        async def wait():
            while not predicate():
                await asyncio.sleep(0.001)

        await asyncio.wait_for(wait(), timeout=1.0)

    def multiplexer(self, generate, abort=None):
        async def noop_abort(request_id):
            pass

        multiplexer = self.worker["GenerationMultiplexer"](
            1, generate, abort or noop_abort, self.messages.append,
            self.worker["finish_request"], health=self.health,
        )
        self.worker["generation_multiplexer"] = multiplexer
        return multiplexer

    async def test_dead_core_with_live_wrapper_emits_one_bounded_frame(self):
        self.engine.die()
        self.assertTrue(self.engine.is_running)
        for _ in range(5):
            self.assertTrue(self.health.check())
            with self.assertRaisesRegex(RuntimeError, FATAL["error"]):
                self.health.raise_if_dead()
        self.assertEqual(self.messages, [FATAL])
        self.assertEqual(self.engine.polls, 1)
        self.engine.engine_core.resources.engine_dead = False
        self.assertTrue(self.health.check())
        self.assertEqual(self.messages, [FATAL])

    async def test_idle_backend_death_is_detected_and_polling_stops(self):
        self.health.start()
        await self.wait_until(lambda: self.engine.polls >= 2)
        self.assertEqual(self.messages, [])
        self.engine.die()
        await self.wait_until(lambda: self.messages == [FATAL])
        self.assertTrue(self.health._task.done())
        self.assertEqual(self.engine.generations, 0)

    async def test_stopped_wrapper_is_also_fatal(self):
        self.engine.is_running = False
        self.assertTrue(self.health.check())
        self.assertEqual(self.messages, [FATAL])

    async def test_optional_or_unreadable_health_api_does_not_poison(self):
        class Unreadable:
            @property
            def errored(self):
                raise ValueError("unsupported property")

        for engine in (object(), Unreadable(), types.SimpleNamespace(errored=None)):
            with self.subTest(engine=type(engine).__name__):
                health = self.worker["EngineHealthMonitor"](engine, self.messages.append)
                self.assertFalse(health.check())
                health.raise_if_dead()
        self.assertEqual(self.messages, [])

    async def test_request_error_remains_healthy_and_next_request_runs(self):
        async def generate(request_id, payload):
            if request_id == 1:
                raise ValueError("invalid request")
            return {"text": "ok"}

        multiplexer = self.multiplexer(generate)
        multiplexer.submit(1, {})
        multiplexer.submit(2, {})
        await multiplexer.drain()
        self.assertEqual([message["type"] for message in self.messages], ["response"] * 2)
        self.assertEqual(self.messages[0]["error"], "invalid request")
        self.assertTrue(self.messages[1]["ok"])
        self.assertFalse(self.health.check())

    async def test_failure_checks_health_immediately_and_blocks_queued_generation(self):
        started = []

        async def generate(request_id, payload):
            started.append(request_id)
            self.engine.die()
            raise ValueError("opaque backend exception " + "x" * 10000)

        multiplexer = self.multiplexer(generate)
        multiplexer.submit(1, {})
        multiplexer.submit(2, {})
        await multiplexer.drain()
        self.assertEqual(started, [1])
        self.assertEqual(self.messages[0], FATAL)
        self.assertEqual(sum(message["type"] == "fatal" for message in self.messages), 1)
        self.assertEqual(self.messages[-1]["error"], FATAL["error"])
        self.assertIsNone(self.health._task)

    async def test_dead_engine_rejected_before_preprocessing(self):
        started = []

        async def generate(request_id, payload):
            started.append(request_id)

        self.engine.die()
        multiplexer = self.multiplexer(generate)
        multiplexer.submit(1, {})
        await multiplexer.drain()
        self.assertEqual(started, [])
        self.assertEqual(self.messages[0], FATAL)
        self.assertFalse(self.messages[1]["ok"])

    async def test_death_during_preprocessing_prevents_engine_generate(self):
        def prepare(request_id, payload):
            self.engine.die()
            return {"empty": False}

        self.worker["prepare_generation_request"] = prepare
        multiplexer = self.multiplexer(self.worker["async_handle_generate"])
        multiplexer.submit(1, {})
        await multiplexer.drain()
        self.assertEqual(self.engine.generations, 0)
        self.assertEqual(self.messages[0], FATAL)
        self.assertEqual(self.messages[1]["error"], FATAL["error"])

    async def test_cooperative_cancellation_keeps_healthy_engine_usable(self):
        started = asyncio.Event()
        release = asyncio.Event()
        aborted = []

        async def generate(request_id, payload):
            if request_id == 1:
                multiplexer.engine_started(request_id)
                started.set()
                await release.wait()
            return {"text": "ok"}

        async def abort(request_id):
            aborted.append(request_id)
            release.set()

        multiplexer = self.multiplexer(generate, abort)
        multiplexer.submit(1, {})
        await asyncio.wait_for(started.wait(), timeout=1.0)
        self.worker["mark_cancelled"](1)
        await multiplexer.cancel(1)
        multiplexer.submit(2, {})
        await multiplexer.drain()
        self.assertEqual(aborted, [1])
        self.assertTrue(self.messages[0]["cancelled"])
        self.assertTrue(self.messages[1]["ok"])
        self.assertEqual([message["type"] for message in self.messages], ["response"] * 2)
        self.assertFalse(self.health.check())

    async def test_monitor_start_is_idempotent_and_stop_awaits_cleanup(self):
        self.health.start()
        task = self.health._task
        self.health.start()
        self.assertIs(self.health._task, task)
        await self.wait_until(lambda: self.engine.polls > 0)
        await self.health.stop()
        await self.health.stop()
        self.assertTrue(task.done())
        self.assertIsNone(self.health._task)
        polls = self.engine.polls
        self.engine.die()
        await asyncio.sleep(0.02)
        self.assertEqual(self.engine.polls, polls)
        self.assertEqual(self.messages, [])

    async def fake_load(self, engine, compilation=False):
        transformers = types.ModuleType("transformers")
        transformers.AutoProcessor = transformers.AutoTokenizer = types.SimpleNamespace(
            from_pretrained=lambda *args, **kwargs: object()
        )

        async def abort(request_id):
            pass

        self.worker.update(
            create_engine=lambda payload: engine,
            optional_compilation_config=lambda payload: compilation,
            get_tokenizer=lambda: None,
            model_ctx=lambda size: size,
            vocab_size=lambda: 100,
            abort_engine_request=abort,
        )
        with patch.dict(sys.modules, {"transformers": transformers}):
            await self.worker["handle_load"]({"path": "fake-model"})

    async def test_load_replacement_stops_old_monitor_and_watches_new_engine(self):
        self.health.start()
        old_task = self.health._task
        replacement = FakeEngine()
        await self.fake_load(replacement)
        current = self.worker["engine_health_monitor"]
        self.assertTrue(old_task.done())
        self.assertIsNone(self.health._task)
        self.assertIs(current.engine, replacement)
        self.assertIs(self.worker["generation_multiplexer"]._health, current)
        self.engine.die()
        self.assertFalse(current.check())
        self.assertEqual(self.messages, [])
        replacement.die()
        await self.wait_until(lambda: self.messages == [FATAL])

    async def test_rejected_load_stops_monitor_before_engine_shutdown(self):
        monitors = []

        async def observe(engine, payload):
            monitor = self.worker["engine_health_monitor"]
            monitors.append((monitor, monitor._task))
            await asyncio.sleep(0)
            raise ValueError("execution observation rejected")

        async def shutdown(engine):
            monitor, task = monitors[0]
            self.assertTrue(task.done())
            self.assertIsNone(monitor._task)
            self.assertIsNone(self.worker["engine_health_monitor"])
            engine.is_running = False

        self.worker.update(
            observe_worker_execution=observe,
            shutdown_rejected_engine=shutdown,
        )
        with self.assertRaisesRegex(ValueError, "execution observation rejected"):
            await self.fake_load(FakeEngine(), compilation=True)
        self.assertIsNone(self.worker["engine"])
        self.assertIsNone(self.worker["generation_multiplexer"])
        self.assertEqual(self.messages, [])

    async def test_worker_shutdown_and_eof_stop_monitor(self):
        for request in ({"id": 1, "op": "shutdown"}, None):
            with self.subTest(request=request):
                await self.fake_load(FakeEngine())
                monitor = self.worker["engine_health_monitor"]
                task = monitor._task
                self.worker["request_queue"].put(request)
                if request is None:
                    await self.worker["run_worker"]()
                else:
                    with self.assertRaises(SystemExit):
                        await self.worker["run_worker"]()
                self.assertTrue(task.done())
                self.assertIsNone(monitor._task)
                self.assertIsNone(self.worker["engine_health_monitor"])
        self.assertEqual(self.messages, [])


if __name__ == "__main__":
    unittest.main()
