import ast
import contextlib
import io
import json
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch


class HybridPrefillAlignmentTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).resolve().parents[1] / "src" / "vllm_worker.py"
        node = next(node for node in ast.parse(source.read_text()).body
                    if isinstance(node, ast.FunctionDef)
                    and node.name == "align_hybrid_prefill_args")
        scope = {"json": json, "sys": sys}
        exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), "exec"), scope)
        self.align = scope["align_hybrid_prefill_args"]
        self.block = 1056
        self.calls = []
        self.cache = types.SimpleNamespace(
            cache_dtype="auto", user_specified_block_size=False, block_size=16)
        self.scheduler = types.SimpleNamespace(
            max_num_batched_tokens=512, long_prefill_token_threshold=0)
        self.config = types.SimpleNamespace(
            model_config=types.SimpleNamespace(
                is_hybrid=True, dtype="bfloat16", use_mla=False,
                get_head_size=lambda: 256, get_num_attention_heads=lambda _: 16),
            cache_config=self.cache, scheduler_config=self.scheduler,
            parallel_config=object())
        self.args = types.SimpleNamespace(
            mamba_cache_mode="align", max_num_batched_tokens=512,
            long_prefill_token_threshold=0,
            create_engine_config=lambda: self.config)

        def upstream_alignment(config, backend):
            self.calls.append((config, backend))
            config.cache_config.block_size = self.block

        modules = {
            "vllm.config.cache": types.SimpleNamespace(
                CacheConfig=types.SimpleNamespace(DEFAULT_BLOCK_SIZE=16)),
            "vllm.config.vllm": types.SimpleNamespace(
                set_current_vllm_config=lambda _: contextlib.nullcontext()),
            "vllm.platforms": types.SimpleNamespace(current_platform=types.SimpleNamespace(
                _align_hybrid_block_size=upstream_alignment)),
            "vllm.v1.attention.selector": types.SimpleNamespace(
                get_attn_backend=lambda **_: types.SimpleNamespace(
                    get_preferred_block_size=lambda _: 16)),
        }
        self.patch = patch.dict(sys.modules, modules)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def run_alignment(self):
        with contextlib.redirect_stderr(io.StringIO()) as diagnostic:
            self.align(self.args)
        return diagnostic.getvalue()

    def test_small_prefill_limit_uses_upstream_cache_requirement(self):
        message = json.loads(self.run_alignment())
        self.assertEqual(self.args.max_num_batched_tokens, 1056)
        self.assertEqual(self.args.long_prefill_token_threshold, 0)
        self.assertEqual(message["changes"]["max_num_batched_tokens"],
                         {"requested": 512, "effective": 1056})
        self.assertEqual(len(self.calls), 1)

    def test_sufficient_batch_limit_is_preserved(self):
        self.scheduler.max_num_batched_tokens = self.args.max_num_batched_tokens = 8192
        self.assertEqual(self.run_alignment(), "")
        self.assertEqual(self.args.max_num_batched_tokens, 8192)

    def test_positive_long_prefill_threshold_must_fit_one_cache_block(self):
        self.scheduler.long_prefill_token_threshold = 128
        self.run_alignment()
        self.assertEqual(self.args.long_prefill_token_threshold, 1056)

    def test_dense_model_does_not_change_scheduler(self):
        self.config.model_config.is_hybrid = False
        self.assertEqual(self.run_alignment(), "")
        self.assertEqual(self.args.max_num_batched_tokens, 512)
        self.assertEqual(self.calls, [])

    def test_unresolved_cache_block_fails_before_engine_creation(self):
        self.block = 0
        with self.assertRaisesRegex(ValueError, "positive hybrid cache block"):
            self.run_alignment()


if __name__ == "__main__":
    unittest.main()
