import ast
import base64
import contextlib
import errno
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


def load_file_scope(base_dir):
    source = Path(__file__).resolve().parents[1] / "src" / "comfyui_worker.py"
    tree = ast.parse(source.read_text(), source.name)
    names = {"safe_input_file_path", "materialized_input_files", "resolve_output_file",
             "input_transfer_root", "restore_input_transfer", "recover_input_transfers",
             "input_transfer_lock", "staged_input_files"}
    functions = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
    namespace = {"base_dir": base_dir, "base64": base64, "contextlib": contextlib,
                 "tempfile": tempfile, "Path": Path, "json": json,
                 "os": os, "time": time, "errno": errno}
    exec(compile(ast.Module(body=functions, type_ignores=[]), source.name, "exec"), namespace)
    return namespace


def reference(data, filename="refs/frame.png"):
    return {"filename": filename, "data_base64": base64.b64encode(data).decode("ascii")}


class ComfyReferenceFileTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.scope = load_file_scope(self.root)
        self.path = self.root / "input" / "refs" / "frame.png"

    def test_consumer_reads_each_request_bytes_and_upload_is_removed(self):
        for content in (b"first image content", b"different image content"):
            with self.scope["materialized_input_files"]({"input_files": [reference(content)]}):
                self.assertEqual(self.scope["safe_input_file_path"]("refs/frame.png").read_bytes(), content)
            self.assertFalse(self.path.exists())

    def test_existing_provider_file_is_restored_after_success_or_execution_error(self):
        self.path.parent.mkdir(parents=True)
        self.path.write_bytes(b"provider fixture")
        for fail in (False, True):
            try:
                with self.scope["materialized_input_files"]({"input_files": [reference(b"user image")]}):
                    self.assertEqual(self.path.read_bytes(), b"user image")
                    if fail:
                        raise RuntimeError("generation failed")
            except RuntimeError:
                self.assertTrue(fail)
            self.assertEqual(self.path.read_bytes(), b"provider fixture")

    def test_later_invalid_file_cleans_earlier_upload_and_restores_fixture(self):
        self.path.parent.mkdir(parents=True)
        self.path.write_bytes(b"provider fixture")
        payload = {"input_files": [reference(b"user image"), reference(b"second", "refs/new.png"),
                                    {"filename": "refs/broken.png", "data_base64": "!invalid!"}]}
        with self.assertRaises(ValueError):
            with self.scope["materialized_input_files"](payload):
                self.fail("Invalid upload must never enter generation")
        self.assertEqual(self.path.read_bytes(), b"provider fixture")
        self.assertFalse((self.path.parent / "new.png").exists())

    def test_same_file_cannot_be_supplied_twice(self):
        with self.assertRaisesRegex(ValueError, "duplicate"):
            with self.scope["materialized_input_files"]({"input_files": [reference(b"one"), reference(b"two")]}):
                self.fail("Duplicate filename must be rejected")
        self.assertFalse(self.path.exists())

    def test_next_worker_recovers_files_after_process_termination(self):
        self.path.parent.mkdir(parents=True)
        self.path.write_bytes(b"provider fixture")
        child = """
import os, runpy, sys
from pathlib import Path
helpers = runpy.run_path(sys.argv[1])
scope = helpers['load_file_scope'](Path(sys.argv[2]))
files = [helpers['reference'](b'user image'), helpers['reference'](b'private upload', 'refs/new.png')]
with scope['materialized_input_files']({'input_files': files}):
    os._exit(0)
"""
        subprocess.run([sys.executable, "-c", child, str(Path(__file__).resolve()), str(self.root)], check=True)
        self.assertEqual(self.path.read_bytes(), b"user image")
        # A new worker must restore provider bytes before the next graph runs,
        # including a graph with no new attachments at all.
        scope = load_file_scope(self.root)
        with scope["materialized_input_files"]({}):
            self.assertEqual(self.path.read_bytes(), b"provider fixture")
            self.assertFalse((self.path.parent / "new.png").exists())
        self.assertEqual([p.name for p in scope["input_transfer_root"]().iterdir()], [".lock"])

    def test_second_worker_waits_for_active_reference_owner(self):
        self.path.parent.mkdir(parents=True)
        self.path.write_bytes(b"provider fixture")
        child = """
import runpy, sys
from pathlib import Path
helpers = runpy.run_path(sys.argv[1])
root = Path(sys.argv[2])
scope = helpers['load_file_scope'](root)
(root / 'waiting').write_text('ready')
with scope['materialized_input_files']({'input_files': [helpers['reference'](b'second request')]}):
    (root / 'entered').write_bytes((root / 'input/refs/frame.png').read_bytes())
"""
        process = None
        try:
            with self.scope["materialized_input_files"]({"input_files": [reference(b"first request")]}):
                process = subprocess.Popen([sys.executable, "-c", child, str(Path(__file__).resolve()), str(self.root)])
                deadline = time.monotonic() + 5
                while not (self.root / "waiting").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue((self.root / "waiting").exists())
                time.sleep(0.15)
                self.assertFalse((self.root / "entered").exists())
                self.assertEqual(self.path.read_bytes(), b"first request")
            self.assertEqual(process.wait(timeout=5), 0)
            self.assertEqual((self.root / "entered").read_bytes(), b"second request")
            self.assertEqual(self.path.read_bytes(), b"provider fixture")
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)

    def test_symlink_to_sibling_with_same_prefix_cannot_escape_input_root(self):
        (self.root / "input").mkdir()
        sibling = self.root / "input-private"
        sibling.mkdir()
        (sibling / "file.png").write_bytes(b"private")
        (self.root / "input" / "alias").symlink_to(sibling, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "escaped"):
            self.scope["safe_input_file_path"]("alias/file.png")
        with self.assertRaisesRegex(RuntimeError, "escaped"):
            self.scope["resolve_output_file"]({"type": "input", "subfolder": "alias", "filename": "file.png"})


if __name__ == "__main__":
    unittest.main()
