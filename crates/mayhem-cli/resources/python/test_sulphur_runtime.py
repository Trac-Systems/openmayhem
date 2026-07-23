import ast
import base64
import importlib.util
import hashlib
import inspect
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock


MODULE_PATH = pathlib.Path(__file__).with_name("sulphur_runtime.py")
SPEC = importlib.util.spec_from_file_location("mayhem_test_sulphur_runtime", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REPOSITORY_ROOT = MODULE_PATH.parents[4]
WORKER_PATH = REPOSITORY_ROOT / "crates/mayhem-engine/src/sulphur_worker.py"
WORKER_SPEC = importlib.util.spec_from_file_location("mayhem_test_sulphur_worker", WORKER_PATH)
WORKER = importlib.util.module_from_spec(WORKER_SPEC)
assert WORKER_SPEC.loader is not None
sys.modules[WORKER_SPEC.name] = WORKER
WORKER_SPEC.loader.exec_module(WORKER)

PNG_1X1 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)


class FakeTokenizer:
    def encode(self, value, add_special_tokens=True):
        assert add_special_tokens is True
        return list(range(len(value.strip().split()) + 1))


class SulphurRuntimeValidationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        cache = pathlib.Path(self.temporary.name)
        (cache / "inputs").mkdir()
        self.runtime = MODULE._Runtime(
            model_root=cache / "model",
            cache_root=cache,
            artifact_path=cache / "model.gguf",
            pipeline_root=cache / "pipeline",
            distillation_lora=cache / "distill.safetensors",
            latent_upsampler_root=cache / "upsampler",
            device="cuda:0",
            torch=None,
            t2v_pipeline=None,
            i2v_pipeline=None,
            upsample_pipeline=None,
            encode_video=None,
            video_condition_type=None,
            cfgpp_scheduler_type=None,
            lcm_scheduler_type=None,
            tokenizer=FakeTokenizer(),
            prompt_enhancer=None,
        )
        self.request = {
            "enhance_prompt": False,
            "frame_rate": 24.0,
            "height": 512,
            "images": [],
            "negative_prompt": "",
            "num_frames": 121,
            "prompt": "A bell rings while rain falls on a copper roof.",
            "seed": 42,
            "width": 768,
        }
        WORKER.runtime_adapter = MODULE
        WORKER.runtime = self.runtime
        WORKER.input_root = cache / "inputs"

    def test_valid_distilled_t2v_request_reports_exact_controls(self):
        evidence = MODULE.validate_video(self.runtime, dict(self.request))
        self.assertEqual(
            evidence,
            {"handled_controls": sorted(self.request), "valid": True},
        )

    def test_negative_prompt_is_a_real_cfgpp_control(self):
        request = dict(self.request, negative_prompt="silence")
        self.assertTrue(MODULE.validate_video(self.runtime, request)["valid"])
        request["negative_prompt"] = "x" * (MODULE._MAX_PROMPT_BYTES + 1)
        with self.assertRaisesRegex(ValueError, "negative_prompt"):
            MODULE.validate_video(self.runtime, request)

    def test_prompt_over_token_limit_is_rejected_instead_of_truncated(self):
        request = dict(
            self.request,
            prompt=" ".join(f"token-{index}" for index in range(MODULE._MAX_PROMPT_TOKENS)),
        )
        with self.assertRaisesRegex(ValueError, "truncation is forbidden"):
            MODULE.validate_video(self.runtime, request)

    def test_guidance_is_not_a_distilled_control(self):
        request = dict(self.request, guidance_scale=4.0)
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE.validate_video(self.runtime, request)

    def test_step_count_is_not_a_request_control(self):
        request = dict(self.request, step_count=11)
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE.validate_video(self.runtime, request)

    def test_fractional_frame_rate_matches_worker_domain(self):
        request = dict(self.request, frame_rate=23.976)
        self.assertTrue(MODULE.validate_video(self.runtime, request)["valid"])

    def test_i2v_accepts_only_materialized_cache_input(self):
        image = self.runtime.cache_root / "inputs" / "frame.png"
        image.write_bytes(PNG_1X1)
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "frame_index": 0,
                    "path": str(image),
                    "strength": 1.0,
                }
            ],
        )
        evidence = MODULE.validate_video(self.runtime, request)
        self.assertEqual(evidence["handled_controls"], sorted(self.request))

    def test_i2v_rejects_arbitrary_filesystem_path(self):
        image = self.runtime.cache_root / "outside.png"
        image.write_bytes(PNG_1X1)
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "frame_index": 0,
                    "path": str(image),
                    "strength": 1.0,
                }
            ],
        )
        with self.assertRaisesRegex(ValueError, "escaped its bounded root"):
            MODULE.validate_video(self.runtime, request)

    def test_prompt_enhancement_is_reserved_for_verified_host_compositor(self):
        request = dict(self.request, enhance_prompt=True)
        with self.assertRaisesRegex(ValueError, "verified llama.cpp host compositor"):
            MODULE.validate_video(self.runtime, request)

    def test_unknown_request_control_fails_closed(self):
        request = dict(self.request, arbitrary_steps=99)
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE.validate_video(self.runtime, request)

    def test_description_matches_real_worker_contract(self):
        description = MODULE.describe(self.runtime)
        self.assertEqual(description["stage_1_denoise_intervals"], 8)
        self.assertEqual(description["stage_2_denoise_intervals"], 3)
        self.assertEqual(WORKER._validate_description(description, "gguf"), description)

    def test_video_condition_uses_documented_diffusers_module(self):
        imports = [
            node
            for node in ast.walk(ast.parse(MODULE_PATH.read_text(encoding="utf-8")))
            if isinstance(node, ast.ImportFrom)
            and any(alias.name == "LTX2VideoCondition" for alias in node.names)
        ]
        self.assertEqual(len(imports), 1)
        self.assertEqual(
            imports[0].module,
            "diffusers.pipelines.ltx2.pipeline_ltx2_condition",
        )

    def test_creator_stage_schedules_exclude_terminal_zero(self):
        stage_1 = MODULE._creator_stage_1_sigmas(97, 256, 384, 8, 32)
        expected = (
            1.0,
            0.958819062254,
            0.907115837915,
            0.840266754513,
            0.750474547799,
            0.623481079521,
            0.43012698966,
            0.1,
        )
        for actual, target in zip(stage_1, expected):
            self.assertAlmostEqual(actual, target, places=11)
        self.assertEqual(MODULE._STAGE_2_SIGMAS, (0.85, 0.725, 0.4219))
        self.assertNotIn(0.0, stage_1)
        self.assertNotIn(0.0, MODULE._STAGE_2_SIGMAS)

    def test_cfgpp_flow_coefficients_include_cfg1_unconditional_derivative(self):
        alpha_s, alpha_t, sigma_down, sigma_up = MODULE._cfgpp_ancestral_coefficients(
            0.75, 0.5
        )
        self.assertEqual(alpha_s, 0.25)
        self.assertEqual(alpha_t, 0.5)
        self.assertGreater(sigma_up, 0.0)
        self.assertGreaterEqual(sigma_down, 0.0)
        self.assertAlmostEqual(sigma_down * sigma_down + sigma_up * sigma_up, 1.0)

    def test_sampler_noise_uses_a_distinct_deterministic_seed_domain(self):
        for stage in ("stage-1", "stage-2"):
            initial_seed = MODULE._stage_seed(7, stage)
            sampler_seed = MODULE._sampler_seed(7, stage)
            self.assertNotEqual(initial_seed, sampler_seed)
            self.assertEqual(sampler_seed, MODULE._sampler_seed(7, stage))

    def test_creator_lora_scaling_removes_peft_rank_normalization(self):
        class Layer:
            def __init__(self, rank):
                self.r = {"mayhem_distilled": rank}
                self.lora_alpha = {"mayhem_distilled": 1}
                self.scaling = {"mayhem_distilled": 1 / rank}
                self.use_rslora = {"mayhem_distilled": False}

        layers = [Layer(1), Layer(36), Layer(72)]
        model = types.SimpleNamespace(modules=lambda: iter(layers))
        MODULE._restore_creator_lora_scaling(model, "mayhem_distilled", 3)
        for layer in layers:
            self.assertEqual(
                layer.lora_alpha["mayhem_distilled"],
                layer.r["mayhem_distilled"],
            )
            self.assertEqual(layer.scaling["mayhem_distilled"], 1.0)

    def test_creator_lora_inventory_requires_complete_pairs(self):
        names = {
            "one.lora_A.weight",
            "one.lora_B.weight",
            "two.lora_A.weight",
            "two.lora_B.weight",
        }
        self.assertEqual(MODULE._lora_pair_count(names), 2)
        with self.assertRaisesRegex(RuntimeError, "incomplete A/B pairs"):
            MODULE._lora_pair_count(names - {"two.lora_B.weight"})

    def test_custom_scheduler_advertises_diffusers_sigma_and_timestep_support(self):
        class SchedulerBase:
            def set_timesteps(
                self,
                num_inference_steps=None,
                device=None,
                sigmas=None,
                mu=None,
                timesteps=None,
            ):
                pass

        cfgpp_type, lcm_type = MODULE._scheduler_types(
            types.SimpleNamespace(),
            SchedulerBase,
            object,
        )
        for scheduler_type in (cfgpp_type, lcm_type):
            parameters = inspect.signature(scheduler_type.set_timesteps).parameters
            self.assertIn("sigmas", parameters)
            self.assertIn("timesteps", parameters)

    def test_text_encoder_must_remain_exact_prequantized_nf4(self):
        quantization = {
            "quant_method": "bitsandbytes",
            "load_in_4bit": True,
            "bnb_4bit_compute_dtype": "bfloat16",
            "bnb_4bit_quant_storage": "uint8",
            "bnb_4bit_quant_type": "nf4",
            "bnb_4bit_use_double_quant": True,
        }
        pipeline = types.SimpleNamespace(
            text_encoder=types.SimpleNamespace(
                is_loaded_in_4bit=True,
                config=types.SimpleNamespace(quantization_config=quantization),
            )
        )
        MODULE._require_prequantized_text_encoder(pipeline)
        pipeline.text_encoder.is_loaded_in_4bit = False
        with self.assertRaisesRegex(RuntimeError, "serialized 4-bit"):
            MODULE._require_prequantized_text_encoder(pipeline)

    def test_real_worker_accepts_adapter_validation_result(self):
        evidence = WORKER._validate_request(dict(self.request))
        self.assertEqual(
            evidence,
            {"handled_controls": sorted(self.request), "valid": True},
        )

    def test_real_worker_materializes_bounded_inline_i2v_image(self):
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "data_base64": base64.b64encode(PNG_1X1).decode("ascii"),
                    "frame_index": 0,
                    "strength": 1.0,
                }
            ],
        )
        evidence = WORKER._validate_request(request)
        self.assertTrue(evidence["valid"])
        self.assertEqual(list(WORKER.input_root.iterdir()), [])

    def test_real_worker_accepts_exact_adapter_generation_result(self):
        class FakeValue:
            def __init__(self, shape=None, ndim=None):
                self.shape = shape
                self.ndim = ndim

            def numel(self):
                return 242

            def detach(self):
                return self

            def float(self):
                return self

            def cpu(self):
                return self

            def abs(self):
                return self

            def max(self):
                return self

            def item(self):
                return 0.5

        class FakePipeline:
            transformer_spatial_patch_size = 1
            transformer_temporal_patch_size = 1
            vae_temporal_compression_ratio = 8
            vae_spatial_compression_ratio = 32
            vocoder = types.SimpleNamespace(
                config=types.SimpleNamespace(output_sampling_rate=24)
            )

            def __init__(self):
                self.scheduler = FakeScheduler()
                self.calls = []
                self.adapters = []

            def set_adapters(self, name, weight):
                if FakeTorch.inference_depth <= 0:
                    raise RuntimeError("adapter activation escaped inference mode")
                self.adapters.append((name, weight))

            def __call__(self, **kwargs):
                self.calls.append(kwargs)
                if kwargs["output_type"] == "latent":
                    return object(), object()
                return [FakeValue(shape=(121, 512, 768, 3))], [FakeValue(shape=(2, 121), ndim=2)]

        class FakeScheduler:
            configured_seeds = []

            def __init__(self):
                self.config = {}
                self.seed = None

            def configure_seed(self, seed):
                self.seed = seed
                self.configured_seeds.append(seed)

        class FakeCfgppScheduler(FakeScheduler):
            pass

        class FakeLcmScheduler(FakeScheduler):
            pass

        class FakeCfgppSchedulerType:
            @classmethod
            def from_config(cls, config, **kwargs):
                return FakeCfgppScheduler()

        class FakeLcmSchedulerType:
            @classmethod
            def from_config(cls, config, **kwargs):
                return FakeLcmScheduler()

        class FakeTorch:
            inference_depth = 0
            generator_seeds = []

            class Generator:
                def __init__(self, device):
                    self.device = device

                def manual_seed(self, seed):
                    self.seed = seed
                    FakeTorch.generator_seeds.append(seed)
                    return self

            class InferenceMode:
                def __enter__(self):
                    FakeTorch.inference_depth += 1

                def __exit__(self, exc_type, exc_value, traceback):
                    FakeTorch.inference_depth -= 1

            @classmethod
            def inference_mode(cls):
                return cls.InferenceMode()

        pipeline = FakePipeline()
        self.runtime.torch = FakeTorch
        self.runtime.t2v_pipeline = pipeline
        self.runtime.i2v_pipeline = pipeline
        self.runtime.upsample_pipeline = lambda **kwargs: [object()]
        self.runtime.cfgpp_scheduler_type = FakeCfgppSchedulerType
        self.runtime.lcm_scheduler_type = FakeLcmSchedulerType
        self.runtime.video_condition_type = lambda **kwargs: kwargs
        self.runtime.encode_video = lambda *args, **kwargs: pathlib.Path(
            kwargs["output_path"]
        ).write_bytes(b"bounded-mp4-fixture")
        WORKER.output_root = self.runtime.cache_root / "outputs"
        WORKER.output_root.mkdir()
        original_probe = WORKER._probe_media
        WORKER._probe_media = lambda output, request: {
            "video_duration_seconds": 121 / 24,
            "audio_duration_seconds": 121 / 24,
            "duration_delta_seconds": 0.0,
            "fps": 24.0,
            "video_packet_count": 121,
            "audio_packet_count": 1,
            "timestamps_monotonic": True,
            "audio_peak_s16": 1,
            "ffprobe_decodable": True,
            "ffmpeg_audio_decodable": True,
        }
        self.addCleanup(setattr, WORKER, "_probe_media", original_probe)
        output = WORKER.output_root / "result.mp4"
        result = WORKER._generate({"output_path": str(output), "request": self.request})
        self.assertEqual(
            set(result),
            {
                "duration_seconds",
                "frame_count",
                "handled_controls",
                "media_evidence",
                "output_bytes",
                "output_path",
                "stage_1_denoise_intervals",
                "stage_2_denoise_intervals",
            },
        )
        self.assertNotIn("step_count", result)
        self.assertEqual(
            pipeline.adapters,
            [
                ("mayhem_distilled", 0.7),
                ("mayhem_distilled", 0.5),
            ],
        )
        self.assertEqual(pipeline.calls[0]["negative_prompt"], "")
        self.assertTrue(pipeline.calls[0]["use_cross_timestep"])
        self.assertEqual(pipeline.calls[1]["sigmas"], [0.85, 0.725, 0.4219])
        self.assertTrue(pipeline.calls[1]["use_cross_timestep"])
        self.assertIsInstance(pipeline.scheduler, FakeLcmScheduler)
        self.assertEqual(
            FakeTorch.generator_seeds[:2],
            [
                MODULE._stage_seed(self.request["seed"], "stage-1"),
                MODULE._stage_seed(self.request["seed"], "stage-2"),
            ],
        )
        self.assertEqual(
            FakeScheduler.configured_seeds[:2],
            [
                MODULE._sampler_seed(self.request["seed"], "stage-1"),
                MODULE._sampler_seed(self.request["seed"], "stage-2"),
            ],
        )
        self.assertTrue(
            set(FakeTorch.generator_seeds[:2]).isdisjoint(
                FakeScheduler.configured_seeds[:2]
            )
        )

        i2v_request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "data_base64": base64.b64encode(PNG_1X1).decode("ascii"),
                    "frame_index": 0,
                    "strength": 1.0,
                }
            ],
            negative_prompt="watermark",
        )
        output = WORKER.output_root / "i2v.mp4"
        with mock.patch.object(MODULE, "_load_image", return_value=object()):
            WORKER._generate({"output_path": str(output), "request": i2v_request})
        self.assertIsInstance(pipeline.scheduler, FakeCfgppScheduler)
        self.assertEqual(pipeline.calls[-1]["negative_prompt"], "watermark")


class SulphurRuntimeManifestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name) / "model"
        files = {
            "sulphur.gguf": b"gguf",
            "distill.safetensors": b"lora",
            "pipeline/model_index.json": b"{}",
            "pipeline/transformer/config.json": b"{}",
            "pipeline/latent_upsampler/config.json": b"{}",
        }
        inventory = {}
        for relative, content in files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            inventory[relative] = {
                "sha256": hashlib.sha256(content).hexdigest(),
                "size": len(content),
            }
        manifest = {
            "schema": MODULE._MANIFEST_SCHEMA,
            "ltx_runtime_commit": MODULE._LTX_RUNTIME_COMMIT,
            "sulphur_source_commit": MODULE._SULPHUR_SOURCE_COMMIT,
            "diffusers_version": MODULE._DIFFUSERS_VERSION,
            "diffusers_commit": MODULE._DIFFUSERS_COMMIT,
            "distillation_mode": MODULE._DISTILLATION_MODE,
            "roles": {
                "transformer_gguf": "sulphur.gguf",
                "distillation_lora": "distill.safetensors",
                "pipeline_root": "pipeline",
                "latent_upsampler": "pipeline/latent_upsampler",
            },
            "prompt_enhancer": None,
            "files": inventory,
        }
        (self.root / MODULE._MANIFEST_NAME).write_text(json.dumps(manifest), encoding="utf-8")

    def test_complete_hash_inventory_is_accepted(self):
        result = MODULE._load_and_verify_manifest(self.root, self.root / "sulphur.gguf")
        self.assertEqual(result["distillation_mode"], MODULE._DISTILLATION_MODE)

    def test_exact_prompt_enhancer_pair_is_accepted_as_inventory(self):
        manifest_path = self.root / MODULE._MANIFEST_NAME
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        for index, relative in enumerate(MODULE._PROMPT_ENHANCER_ASSETS):
            content = f"enhancer-{index}".encode()
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            manifest["files"][relative] = {
                "sha256": hashlib.sha256(content).hexdigest(),
                "size": len(content),
            }
        manifest["prompt_enhancer"] = {
            "asset_paths": list(MODULE._PROMPT_ENHANCER_ASSETS),
            "system_prompt": "",
        }
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        result = MODULE._load_and_verify_manifest(self.root, self.root / "sulphur.gguf")
        self.assertEqual(
            result["_resolved"]["prompt_enhancer"]["asset_paths"],
            list(MODULE._PROMPT_ENHANCER_ASSETS),
        )

    def test_unlisted_sidecar_is_rejected(self):
        (self.root / "unexpected.bin").write_bytes(b"extra")
        with self.assertRaisesRegex(ValueError, "inventory differs from disk"):
            MODULE._load_and_verify_manifest(self.root, self.root / "sulphur.gguf")

    def test_changed_sidecar_is_rejected(self):
        (self.root / "distill.safetensors").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "unexpected size"):
            MODULE._load_and_verify_manifest(self.root, self.root / "sulphur.gguf")


if __name__ == "__main__":
    unittest.main()
