import ast
import asyncio
import math
import pathlib
import threading
import unittest


ENGINE_SRC = pathlib.Path(__file__).resolve().parents[1] / "src"
WORKERS = ("vllm_worker.py", "mlx_worker.py")


def load_cancellation_scope(worker_name):
    worker_path = ENGINE_SRC / worker_name
    tree = ast.parse(worker_path.read_text(), worker_path.name)
    retained = []
    for node in tree.body:
        if isinstance(node, ast.Assign):
            names = {
                target.id
                for target in node.targets
                if isinstance(target, ast.Name)
            }
            if names & {
                "cancelled_requests",
                "active_request_ids",
                "cancelled_requests_lock",
                "completed_request_id",
                "engine",
            }:
                retained.append(node)
        elif isinstance(node, ast.ClassDef) and node.name in {
            "RequestCancelled",
            "GenerationMultiplexer",
        }:
            retained.append(node)
        elif isinstance(node, ast.FunctionDef) and node.name in {
            "register_request",
            "mark_cancelled",
            "finish_request",
            "request_cancelled",
            "check_cancelled",
            "positive_int",
            "load_generation_capacity",
            "runtime_kv_cache_info",
        }:
            retained.append(node)

    namespace = {"asyncio": asyncio, "math": math, "threading": threading}
    module = ast.Module(body=retained, type_ignores=[])
    exec(compile(module, worker_path.name, "exec"), namespace)
    return namespace


class WorkerCancellationCleanupTests(unittest.TestCase):
    def test_completed_cancellations_are_removed_from_a_snapshot(self):
        for worker_name in WORKERS:
            with self.subTest(worker=worker_name):
                worker = load_cancellation_scope(worker_name)
                worker["cancelled_requests"].update({1, 2, 4})

                worker["finish_request"](2)

                self.assertEqual(worker["completed_request_id"], 2)
                self.assertEqual(worker["cancelled_requests"], {4})

                worker["finish_request"](4)
                self.assertEqual(worker["cancelled_requests"], set())

    def test_cleanup_remains_safe_when_nothing_was_cancelled(self):
        for worker_name in WORKERS:
            with self.subTest(worker=worker_name):
                worker = load_cancellation_scope(worker_name)
                worker["finish_request"](1)
                worker["finish_request"](2)
                self.assertEqual(worker["completed_request_id"], 2)
                self.assertEqual(worker["cancelled_requests"], set())

    def test_out_of_order_completion_keeps_active_cancellation(self):
        worker = load_cancellation_scope("vllm_worker.py")
        worker["register_request"](1)
        worker["register_request"](2)
        worker["mark_cancelled"](1)

        worker["finish_request"](2)

        self.assertEqual(worker["completed_request_id"], 2)
        self.assertEqual(worker["cancelled_requests"], {1})
        worker["finish_request"](1)
        self.assertEqual(worker["cancelled_requests"], set())


class VllmWorkerConcurrencyTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.worker = load_cancellation_scope("vllm_worker.py")
        self.messages = []
        self.aborted = []

    def multiplexer(self, capacity, generate, abort=None, abort_timeout=2.0):
        if abort is None:
            async def abort(request_id):
                self.aborted.append(request_id)

        return self.worker["GenerationMultiplexer"](
            capacity,
            generate,
            abort,
            self.messages.append,
            self.worker["finish_request"],
            abort_timeout=abort_timeout,
        )

    async def wait_until(self, predicate):
        for _ in range(100):
            if predicate():
                return
            await asyncio.sleep(0.001)
        self.fail("timed out waiting for concurrent worker state")

    async def test_capacity_is_read_from_load_payload(self):
        capacity = self.worker["load_generation_capacity"]({"max_batch_size": 7})
        self.assertEqual(capacity, 7)

        async def generate(request_id, payload):
            return payload

        multiplexer = self.multiplexer(capacity, generate)
        self.assertEqual(multiplexer.capacity, 7)

    async def test_generations_overlap_up_to_load_capacity(self):
        release = asyncio.Event()
        started = set()
        active = 0
        peak_active = 0

        async def generate(request_id, payload):
            nonlocal active, peak_active
            active += 1
            peak_active = max(peak_active, active)
            started.add(request_id)
            await release.wait()
            active -= 1
            return {"request": request_id, "payload": payload}

        multiplexer = self.multiplexer(2, generate)
        multiplexer.submit(1, {"prompt": "one"})
        multiplexer.submit(2, {"prompt": "two"})

        await self.wait_until(lambda: started == {1, 2})
        self.assertEqual(peak_active, 2)
        release.set()
        await multiplexer.drain()
        self.assertEqual({message["id"] for message in self.messages}, {1, 2})

    async def test_capacity_one_preserves_serial_generation(self):
        first_release = asyncio.Event()
        started = []

        async def generate(request_id, payload):
            started.append(request_id)
            if request_id == 1:
                await first_release.wait()
            return payload

        multiplexer = self.multiplexer(1, generate)
        multiplexer.submit(1, {})
        multiplexer.submit(2, {})

        await self.wait_until(lambda: started == [1])
        await asyncio.sleep(0)
        self.assertEqual(started, [1])
        first_release.set()
        await multiplexer.drain()
        self.assertEqual(started, [1, 2])

    async def test_out_of_order_completion_keeps_response_ids_exact(self):
        releases = {1: asyncio.Event(), 2: asyncio.Event()}
        started = set()

        async def generate(request_id, payload):
            started.add(request_id)
            await releases[request_id].wait()
            return {"value": payload["value"]}

        multiplexer = self.multiplexer(2, generate)
        multiplexer.submit(1, {"value": "first"})
        multiplexer.submit(2, {"value": "second"})
        await self.wait_until(lambda: started == {1, 2})

        releases[2].set()
        await self.wait_until(lambda: len(self.messages) == 1)
        self.assertEqual(self.messages[0]["id"], 2)
        self.assertEqual(self.messages[0]["result"], {"value": "second"})

        releases[1].set()
        await multiplexer.drain()
        self.assertEqual([message["id"] for message in self.messages], [2, 1])
        self.assertEqual(self.messages[1]["result"], {"value": "first"})

    async def test_cancellation_is_isolated_to_target_request(self):
        releases = {1: asyncio.Event(), 2: asyncio.Event()}
        started = set()

        async def generate(request_id, payload):
            started.add(request_id)
            multiplexer.engine_started(request_id)
            await releases[request_id].wait()
            return {"request": request_id}

        async def abort(request_id):
            self.aborted.append(request_id)
            releases[request_id].set()

        multiplexer = self.multiplexer(2, generate, abort=abort)
        multiplexer.submit(1, {})
        multiplexer.submit(2, {})
        await self.wait_until(lambda: started == {1, 2})

        self.worker["mark_cancelled"](1)
        self.assertTrue(await multiplexer.cancel(1))
        await self.wait_until(lambda: len(self.messages) == 1)
        self.assertEqual(self.aborted, [1])
        self.assertEqual(
            self.messages[0],
            {
                "id": 1,
                "type": "response",
                "ok": False,
                "cancelled": True,
                "error": "engine request cancelled",
            },
        )

        releases[2].set()
        await multiplexer.drain()
        self.assertEqual(self.messages[1]["id"], 2)
        self.assertTrue(self.messages[1]["ok"])

    async def test_cancelling_preprocessing_holds_capacity_until_thread_returns(self):
        preprocessing_started = threading.Event()
        release_preprocessing = threading.Event()
        sibling_finished = asyncio.Event()

        async def generate(request_id, payload):
            if request_id == 1:
                preprocessing_started.set()
                await asyncio.to_thread(release_preprocessing.wait)
            sibling_finished.set()
            return {"request": request_id}

        multiplexer = self.multiplexer(1, generate)
        multiplexer.submit(1, {})
        await asyncio.to_thread(preprocessing_started.wait)
        multiplexer.submit(2, {})

        self.worker["mark_cancelled"](1)
        self.assertTrue(await multiplexer.cancel(1))
        with self.assertRaises(asyncio.TimeoutError):
            await asyncio.wait_for(sibling_finished.wait(), timeout=0.05)

        self.assertEqual(self.aborted, [])
        self.assertEqual(self.messages, [])
        release_preprocessing.set()
        await asyncio.wait_for(sibling_finished.wait(), timeout=0.2)
        await multiplexer.drain()

        cancelled = next(item for item in self.messages if item["id"] == 1)
        sibling = next(item for item in self.messages if item["id"] == 2)
        self.assertTrue(cancelled["cancelled"])
        self.assertTrue(sibling["ok"])

    async def test_hung_engine_abort_is_bounded_and_reported(self):
        started = asyncio.Event()
        release = asyncio.Event()

        async def generate(request_id, payload):
            started.set()
            multiplexer.engine_started(request_id)
            await release.wait()

        async def abort(request_id):
            self.aborted.append(request_id)
            await asyncio.Event().wait()

        multiplexer = self.multiplexer(
            1, generate, abort=abort, abort_timeout=0.01
        )
        multiplexer.submit(7, {})
        await started.wait()

        self.worker["mark_cancelled"](7)
        self.assertTrue(await asyncio.wait_for(multiplexer.cancel(7), timeout=0.2))
        self.assertEqual(self.messages, [])
        release.set()
        await asyncio.wait_for(multiplexer.drain(), timeout=0.2)

        self.assertEqual(self.aborted, [7])
        self.assertEqual(len(self.messages), 1)
        self.assertEqual(self.messages[0]["id"], 7)
        self.assertTrue(self.messages[0]["cancelled"])
        self.assertTrue(self.messages[0]["abort_failed"])

    async def test_abort_failure_survives_a_following_generation_error(self):
        started = asyncio.Event()
        release = asyncio.Event()

        async def generate(request_id, payload):
            started.set()
            multiplexer.engine_started(request_id)
            await release.wait()
            raise RuntimeError("engine iterator failed after abort failure")

        async def abort(request_id):
            self.aborted.append(request_id)
            raise RuntimeError("abort failed")

        multiplexer = self.multiplexer(1, generate, abort=abort)
        multiplexer.submit(8, {})
        await started.wait()

        self.worker["mark_cancelled"](8)
        self.assertTrue(await multiplexer.cancel(8))
        release.set()
        await multiplexer.drain()

        self.assertEqual(self.aborted, [8])
        self.assertEqual(len(self.messages), 1)
        self.assertEqual(self.messages[0]["id"], 8)
        self.assertTrue(self.messages[0]["cancelled"])
        self.assertTrue(self.messages[0]["abort_failed"])
        self.assertEqual(
            self.messages[0]["error"], "engine iterator failed after abort failure"
        )

    async def test_runtime_kv_cache_info_uses_profiled_vllm_values(self):
        class CacheConfig:
            kv_cache_size_tokens = 732_776
            kv_cache_max_concurrency = 2.7953

        class Config:
            cache_config = CacheConfig()

        class Engine:
            vllm_config = Config()

        self.worker["engine"] = Engine()
        self.assertEqual(
            self.worker["runtime_kv_cache_info"](),
            {"size_tokens": 732_776, "max_concurrency": 2.7953},
        )


if __name__ == "__main__":
    unittest.main()
