import ast
import importlib.util
from pathlib import Path
import unittest
import tempfile
import subprocess
import sys
import os
import hashlib
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("runtime", Path(__file__).with_name("vllm-prefix-runtime.py"))
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)

class PrefixRuntimeTests(unittest.TestCase):
    def test_unknown_source_is_rejected_without_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            module = root / "vllm/v1/core/single_type_kv_cache_manager.py"
            module.parent.mkdir(parents=True)
            original = "# unrecognized vendor runtime\n"
            module.write_text(original)
            result = subprocess.run([sys.executable, runtime.__file__, "--site-packages", str(root),
                "--backup", str(root / "backup"), "--apply"], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Unrecognized vLLM cache manager", result.stderr)
            self.assertEqual(module.read_text(), original)
            self.assertFalse((root / "backup").exists())

    def test_reviewed_install_is_idempotent_and_retains_original(self):
        source = os.environ.get("MAYHEM_VLLM_PREFIX_SOURCE")
        if not source:
            self.skipTest("requires inventoried complete vLLM module")
        original = Path(source).read_bytes()
        self.assertEqual(hashlib.sha256(original).hexdigest(), runtime.ORIGINAL_SHA256)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            module = root / "vllm/v1/core/single_type_kv_cache_manager.py"
            module.parent.mkdir(parents=True)
            module.write_bytes(original)
            command = [sys.executable, runtime.__file__, "--site-packages", str(root),
                "--backup", str(root / "backup"), "--apply"]
            subprocess.run(command, capture_output=True, check=True)
            self.assertEqual(hashlib.sha256(module.read_bytes()).hexdigest(), runtime.PATCHED_SHA256)
            self.assertEqual((root / "backup" / (runtime.ORIGINAL_SHA256 + ".py")).read_bytes(), original)
            after = module.stat().st_mtime_ns
            subprocess.run(command, capture_output=True, check=True)
            self.assertEqual(module.stat().st_mtime_ns, after)

    def test_mamba_lookup_excludes_speculative_trailing_state(self):
        # Exercise the deployed lookup itself when supplied by the inventory.
        source_path = __import__("os").environ.get("MAYHEM_MAMBA_SOURCE")
        if not source_path:
            self.skipTest("run with inventoried MambaManager source")
        source = Path(source_path).read_text()
        patched = runtime.patched_source(source)
        self.assertEqual(runtime.patched_source(patched), patched)
        cls = next(n for n in ast.parse(patched).body if isinstance(n, ast.ClassDef) and n.name == "MambaManager")
        method = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "find_longest_cache_hit")
        method.decorator_list = []
        module = ast.Module(body=[ast.ImportFrom(module="__future__", names=[ast.alias(name="annotations")], level=0), method], type_ignores=[])
        ns = {"MambaSpec": SimpleNamespace}
        exec(compile(ast.fix_missing_locations(module), "<native-lookup>", "exec"), ns)
        lookup = ns["find_longest_cache_hit"]
        pool = SimpleNamespace(null_block=None, get_cached_block=lambda h, ids: [h])
        def hit(tokens, speculative):
            return lookup(None, list(range(10, 20)), tokens, [0], pool,
                          SimpleNamespace(block_size=16), speculative, 16)
        self.assertEqual(hit(64, False)[0][-1], 13)
        self.assertEqual(hit(64, True)[0][-1], 12)
        self.assertEqual(hit(16, True), ([],))
        self.assertEqual(hit(0, True), ([],))

if __name__ == "__main__":
    unittest.main()
