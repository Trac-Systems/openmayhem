import ast
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
                "cancelled_requests_lock",
                "completed_request_id",
            }:
                retained.append(node)
        elif isinstance(node, ast.ClassDef) and node.name == "RequestCancelled":
            retained.append(node)
        elif isinstance(node, ast.FunctionDef) and node.name in {
            "mark_cancelled",
            "finish_request",
            "request_cancelled",
            "check_cancelled",
        }:
            retained.append(node)

    namespace = {"threading": threading}
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


if __name__ == "__main__":
    unittest.main()
