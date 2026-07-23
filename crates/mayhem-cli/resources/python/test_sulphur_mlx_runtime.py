import hashlib
import importlib.util
import json
import os
import pathlib
import sys
import tempfile
import unittest
from typing import NamedTuple
from unittest import mock


MODULE_PATH = pathlib.Path(__file__).with_name("sulphur_mlx_runtime.py")
SPEC = importlib.util.spec_from_file_location("mayhem_test_sulphur_mlx_runtime", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REPOSITORY_ROOT = MODULE_PATH.parents[4]
WORKER_PATH = REPOSITORY_ROOT / "crates/mayhem-engine/src/sulphur_worker.py"
WORKER_SPEC = importlib.util.spec_from_file_location(
    "mayhem_test_sulphur_mlx_worker", WORKER_PATH
)
WORKER = importlib.util.module_from_spec(WORKER_SPEC)
assert WORKER_SPEC.loader is not None
sys.modules[WORKER_SPEC.name] = WORKER
WORKER_SPEC.loader.exec_module(WORKER)

PNG_1X1 = (
    b"\x89PNG\r\n\x1a\n"
    b"\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
)


class FakePipeline:
    instances = []

    def __init__(self, **kwargs):
        self.init_kwargs = kwargs
        self.calls = []
        self.extra_sidecar = False
        type(self).instances.append(self)

    def generate_and_save(self, **kwargs):
        self.calls.append(kwargs)
        output = pathlib.Path(kwargs["output_path"])
        output.write_bytes(b"joint-av-mp4")
        if self.extra_sidecar:
            output.with_suffix(".json").write_text("{}", encoding="utf-8")
        return str(output)


class FakeImageConditioningInput(NamedTuple):
    path: str
    frame_idx: int
    strength: float
    crf: int


class FakeTokenizer:
    def encode(self, value, add_special_tokens=True):
        assert add_special_tokens is True
        return list(range(len(value.strip().split()) + 1))


class SulphurMlxFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = pathlib.Path(self.temporary.name)
        self.root = self.base / "bundle"
        self.sulphur = self.root / "sulphur"
        self.gemma = self.root / "gemma"
        self.cache = self.base / "cache"
        self.inputs = self.cache / "inputs"
        self.outputs = self.cache / "outputs"
        self.sulphur.mkdir(parents=True)
        self.gemma.mkdir()
        self.inputs.mkdir(parents=True)
        self.outputs.mkdir()
        self.contents = {
            "sulphur/transformer-distilled.safetensors": b"mlxbits-sulphur",
            "sulphur/split_manifest.json": b"{}",
            "sulphur/vae_decoder.safetensors": b"video-vae",
            "sulphur/audio_vae.safetensors": b"audio-vae",
            "sulphur/vocoder.safetensors": b"vocoder",
            "gemma/model-00001-of-00002.safetensors": b"gemma-one",
            "gemma/model-00002-of-00002.safetensors": b"gemma-two",
            "gemma/config.json": b"{}",
            "gemma/tokenizer.json": b"{}",
        }
        for relative, content in self.contents.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)

        self.model_sha = hashlib.sha256(
            self.contents["sulphur/transformer-distilled.safetensors"]
        ).hexdigest()
        self.gemma_pins = []
        for name in (
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ):
            content = self.contents[f"gemma/{name}"]
            self.gemma_pins.append((name, len(content), hashlib.sha256(content).hexdigest()))
        self.constant_patches = [
            mock.patch.object(
                MODULE,
                "_MODEL_PRIMARY_SIZE",
                len(self.contents["sulphur/transformer-distilled.safetensors"]),
            ),
            mock.patch.object(MODULE, "_MODEL_PRIMARY_SHA256", self.model_sha),
            mock.patch.object(MODULE, "_GEMMA_ARTIFACTS", tuple(self.gemma_pins)),
        ]
        for patcher in self.constant_patches:
            patcher.start()
            self.addCleanup(patcher.stop)
        self.write_manifest()

        self.request = {
            "enhance_prompt": False,
            "frame_rate": 24.0,
            "height": 512,
            "images": [],
            "negative_prompt": "",
            "num_frames": 17,
            "prompt": "A bell rings as sulfur crystals glow.",
            "seed": 42,
            "width": 768,
        }

    def manifest_value(self):
        inventory = {
            relative: {
                "size": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
            for relative, content in self.contents.items()
        }
        return {
            "schema": MODULE._MANIFEST_SCHEMA,
            "runtime": {
                "repository": MODULE._MLX_RUNTIME_REPOSITORY,
                "revision": MODULE._MLX_RUNTIME_COMMIT,
                "package": "ltx-2-mlx",
                "version": MODULE._MLX_RUNTIME_VERSION,
                "lockfile_sha256": MODULE._MLX_RUNTIME_LOCK_SHA256,
            },
            "source": {
                "repository": MODULE._SULPHUR_SOURCE_REPOSITORY,
                "revision": MODULE._SULPHUR_SOURCE_COMMIT,
            },
            "model": {
                "repository": MODULE._MODEL_REPOSITORY,
                "revision": MODULE._MODEL_REVISION,
                "primary_artifact": "sulphur/transformer-distilled.safetensors",
                "primary_artifact_sha256": MODULE._MODEL_PRIMARY_SHA256,
                "primary_artifact_size": MODULE._MODEL_PRIMARY_SIZE,
                "quantization_bits": 4,
                "quantization_group_size": 64,
            },
            "gemma": {
                "repository": MODULE._GEMMA_REPOSITORY,
                "revision": MODULE._GEMMA_REVISION,
                "artifacts": [
                    {
                        "path": f"gemma/{name}",
                        "size": size,
                        "sha256": sha256,
                    }
                    for name, size, sha256 in MODULE._GEMMA_ARTIFACTS
                ],
            },
            "roles": {"sulphur_root": "sulphur", "gemma_root": "gemma"},
            "prompt_enhancer": None,
            "files": inventory,
        }

    def write_manifest(self, mutate=None):
        manifest = self.manifest_value()
        if mutate is not None:
            mutate(manifest)
        (self.root / MODULE._MANIFEST_NAME).write_text(
            json.dumps(manifest, sort_keys=True),
            encoding="utf-8",
        )

    def load_runtime(self):
        FakePipeline.instances.clear()
        with (
            mock.patch.object(MODULE, "_require_apple_silicon"),
            mock.patch.object(MODULE, "_verify_runtime_packages"),
            mock.patch.object(MODULE, "_load_tokenizer", return_value=FakeTokenizer()),
            mock.patch.object(
                MODULE,
                "_import_pipeline",
                return_value=(
                    FakePipeline,
                    FakeImageConditioningInput,
                    MODULE._STAGE_1_SIGMAS,
                    MODULE._STAGE_2_SIGMAS,
                ),
            ),
        ):
            runtime = MODULE.load(
                model_root=str(self.root),
                artifact_path=str(self.sulphur),
                backend="mlx",
                cache_root=str(self.cache),
            )
        return runtime, FakePipeline.instances[-1]


class SulphurMlxManifestTests(SulphurMlxFixture):
    def test_complete_exact_inventory_is_accepted_before_imports(self):
        runtime, pipeline = self.load_runtime()
        self.assertEqual(runtime.sulphur_root, self.sulphur.resolve())
        self.assertEqual(runtime.gemma_root, self.gemma.resolve())
        self.assertEqual(pipeline.init_kwargs["model_dir"], str(self.sulphur.resolve()))
        self.assertEqual(pipeline.init_kwargs["gemma_model_id"], str(self.gemma.resolve()))
        self.assertTrue(pipeline.init_kwargs["low_memory"])
        self.assertTrue(pipeline.init_kwargs["low_ram_streaming"])

    def test_unlisted_file_fails_before_heavy_import(self):
        (self.sulphur / "unlisted.bin").write_bytes(b"unexpected")
        importer = mock.Mock()
        with (
            mock.patch.object(MODULE, "_require_apple_silicon"),
            mock.patch.object(MODULE, "_verify_runtime_packages"),
            mock.patch.object(MODULE, "_import_pipeline", importer),
        ):
            with self.assertRaisesRegex(ValueError, "inventory differs from disk"):
                MODULE.load(
                    model_root=str(self.root),
                    artifact_path=str(self.sulphur),
                    backend="mlx",
                    cache_root=str(self.cache),
                )
        importer.assert_not_called()

    def test_changed_file_hash_fails_closed(self):
        (self.sulphur / "audio_vae.safetensors").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "unexpected size"):
            MODULE._load_and_verify_manifest(self.root, self.sulphur)

    def test_wrong_mlxbits_revision_is_rejected(self):
        self.write_manifest(lambda value: value["model"].update(revision="main"))
        with self.assertRaisesRegex(ValueError, "model pin is not canonical"):
            MODULE._load_and_verify_manifest(self.root, self.sulphur)

    def test_wrong_gemma_revision_is_rejected(self):
        self.write_manifest(lambda value: value["gemma"].update(revision="main"))
        with self.assertRaisesRegex(ValueError, "Gemma pin is not canonical"):
            MODULE._load_and_verify_manifest(self.root, self.sulphur)

    def test_artifact_role_must_match_supplied_local_directory(self):
        with self.assertRaisesRegex(ValueError, "artifact path does not match"):
            MODULE._load_and_verify_manifest(self.root, self.gemma)

    def test_prompt_enhancer_assets_are_not_guessed(self):
        self.write_manifest(lambda value: value.update(prompt_enhancer={"repository": "mutable"}))
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE._load_and_verify_manifest(self.root, self.sulphur)

    def test_exact_prompt_enhancer_pair_is_accepted_as_inventory(self):
        for index, relative in enumerate(MODULE._PROMPT_ENHANCER_ASSETS):
            content = f"enhancer-{index}".encode()
            self.contents[relative] = content
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        self.write_manifest(
            lambda value: value.update(
                prompt_enhancer={
                    "asset_paths": list(MODULE._PROMPT_ENHANCER_ASSETS),
                    "system_prompt": "",
                }
            )
        )
        result = MODULE._load_and_verify_manifest(self.root, self.sulphur)
        self.assertIn("manifest_sha256", result)

    def test_only_mlx_backend_is_accepted(self):
        with self.assertRaisesRegex(ValueError, "only backend mlx"):
            MODULE.load(
                model_root=str(self.root),
                artifact_path=str(self.sulphur),
                backend="gguf",
                cache_root=str(self.cache),
            )


class SulphurMlxGenerationTests(SulphurMlxFixture):
    def setUp(self):
        super().setUp()
        self.runtime, self.pipeline = self.load_runtime()
        self.probe = mock.patch.object(
            MODULE,
            "_probe_joint_av",
            return_value={
                "audio_duration_seconds": 17 / 24,
                "audio_sample_rate": 48_000,
                "audio_channels": 2,
                "frame_count": 17,
                "frame_rate": 24.0,
                "video_duration_seconds": 17 / 24,
            },
        )
        self.probe.start()
        self.addCleanup(self.probe.stop)

    def test_t2v_maps_exact_controls_and_fixed_schedule(self):
        output = self.outputs / "t2v.mp4"
        result = MODULE.generate_video(self.runtime, dict(self.request), str(output))
        call = self.pipeline.calls[-1]
        self.assertEqual(
            call,
            {
                "prompt": self.request["prompt"],
                "output_path": str(output.resolve()),
                "height": 512,
                "width": 768,
                "num_frames": 17,
                "frame_rate": 24.0,
                "seed": 42,
                "stage1_steps": 8,
                "stage2_steps": 3,
            },
        )
        self.assertEqual(
            result,
            {
                "duration_seconds": 17 / 24,
                "frame_count": 17,
                "handled_controls": sorted(self.request),
                "stage_1_denoise_intervals": 8,
                "stage_2_denoise_intervals": 3,
            },
        )

    def test_i2v_maps_ordered_conditioning_images_with_exact_controls(self):
        start = self.inputs / "start.png"
        middle = self.inputs / "middle.png"
        end = self.inputs / "end.jpg"
        start.write_bytes(PNG_1X1)
        middle.write_bytes(PNG_1X1)
        end.write_bytes(b"\xff\xd8\xff\xe0")
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 0,
                    "frame_index": 0,
                    "path": str(start),
                    "strength": 1,
                },
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "frame_index": 8,
                    "path": str(middle),
                    "strength": 0.625,
                },
                {
                    "content_type": "image/jpeg",
                    "crf": 51,
                    "frame_index": 16,
                    "path": str(end),
                    "strength": 0,
                },
            ],
        )
        MODULE.generate_video(self.runtime, request, str(self.outputs / "i2v.mp4"))
        self.assertNotIn("image", self.pipeline.calls[-1])
        self.assertEqual(
            self.pipeline.calls[-1]["images"],
            [
                FakeImageConditioningInput(str(start.resolve()), 0, 1.0, 0),
                FakeImageConditioningInput(str(middle.resolve()), 8, 0.625, 33),
                FakeImageConditioningInput(str(end.resolve()), 16, 0.0, 51),
            ],
        )

    def test_i2v_rejects_path_outside_worker_cache(self):
        image = self.base / "outside.png"
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

    def test_i2v_rejects_condition_frame_outside_output(self):
        image = self.inputs / "condition.png"
        image.write_bytes(PNG_1X1)
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "crf": 33,
                    "frame_index": 17,
                    "path": str(image),
                    "strength": 1.0,
                }
            ],
        )
        with self.assertRaisesRegex(ValueError, "frame_index must identify an output frame"):
            MODULE.validate_video(self.runtime, request)

    def test_i2v_rejects_non_finite_or_out_of_range_strength(self):
        image = self.inputs / "condition.png"
        image.write_bytes(PNG_1X1)
        for strength in (-0.01, 1.01, float("nan"), True):
            with self.subTest(strength=strength):
                request = dict(
                    self.request,
                    images=[
                        {
                            "content_type": "image/png",
                            "crf": 33,
                            "frame_index": 0,
                            "path": str(image),
                            "strength": strength,
                        }
                    ],
                )
                with self.assertRaisesRegex(
                    ValueError, "strength must be finite and between 0 and 1"
                ):
                    MODULE.validate_video(self.runtime, request)

    def test_i2v_rejects_invalid_crf(self):
        image = self.inputs / "condition.png"
        image.write_bytes(PNG_1X1)
        for crf in (-1, 52, 33.0, True):
            with self.subTest(crf=crf):
                request = dict(
                    self.request,
                    images=[
                        {
                            "content_type": "image/png",
                            "crf": crf,
                            "frame_index": 0,
                            "path": str(image),
                            "strength": 1.0,
                        }
                    ],
                )
                with self.assertRaisesRegex(
                    ValueError, "crf must be an integer between 0 and 51"
                ):
                    MODULE.validate_video(self.runtime, request)

    def test_i2v_requires_the_exact_conditioning_schema(self):
        image = self.inputs / "condition.png"
        image.write_bytes(PNG_1X1)
        request = dict(
            self.request,
            images=[
                {
                    "content_type": "image/png",
                    "path": str(image),
                }
            ],
        )
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE.validate_video(self.runtime, request)

    def test_prompt_enhancement_is_rejected_without_signed_assets(self):
        with self.assertRaisesRegex(ValueError, "no signed canonical assets"):
            MODULE.validate_video(self.runtime, dict(self.request, enhance_prompt=True))

    def test_prompt_over_token_limit_is_rejected_instead_of_truncated(self):
        request = dict(
            self.request,
            prompt=" ".join(f"token-{index}" for index in range(MODULE._MAX_PROMPT_TOKENS)),
        )
        with self.assertRaisesRegex(ValueError, "truncation is forbidden"):
            MODULE.validate_video(self.runtime, request)

    def test_step_override_is_not_a_supported_control(self):
        with self.assertRaisesRegex(ValueError, "unknown or missing fields"):
            MODULE.validate_video(self.runtime, dict(self.request, steps=99))

    def test_video_only_or_invalid_output_is_removed(self):
        output = self.outputs / "video-only.mp4"
        with mock.patch.object(
            MODULE,
            "_probe_joint_av",
            side_effect=RuntimeError("video-only"),
        ):
            with self.assertRaisesRegex(RuntimeError, "video-only"):
                MODULE.generate_video(self.runtime, dict(self.request), str(output))
        self.assertFalse(output.exists())

    def test_unexpected_output_sidecar_fails_closed(self):
        self.pipeline.extra_sidecar = True
        output = self.outputs / "sidecar.mp4"
        with self.assertRaisesRegex(RuntimeError, "unexpected output sidecar"):
            MODULE.generate_video(self.runtime, dict(self.request), str(output))
        self.assertEqual(list(self.outputs.iterdir()), [])

    def test_description_binds_runtime_commit_and_manifest(self):
        description = MODULE.describe(self.runtime)
        self.assertEqual(description["backend"], "mlx")
        self.assertEqual(description["stage_1_denoise_intervals"], 8)
        self.assertEqual(description["stage_2_denoise_intervals"], 3)
        self.assertIn(MODULE._MLX_RUNTIME_COMMIT, description["runtime_version"])
        self.assertIn(self.runtime.manifest_sha256, description["runtime_version"])
        self.assertEqual(WORKER._validate_description(description, "mlx"), description)


class SulphurMlxOfflineTests(SulphurMlxFixture):
    def test_load_forces_offline_mode_before_pipeline_import(self):
        observed = {}

        def importer():
            observed.update({name: os.environ.get(name) for name in MODULE._OFFLINE_ENV})
            return (
                FakePipeline,
                FakeImageConditioningInput,
                MODULE._STAGE_1_SIGMAS,
                MODULE._STAGE_2_SIGMAS,
            )

        with (
            mock.patch.object(MODULE, "_require_apple_silicon"),
            mock.patch.object(MODULE, "_verify_runtime_packages"),
            mock.patch.object(MODULE, "_load_tokenizer", return_value=FakeTokenizer()),
            mock.patch.object(MODULE, "_import_pipeline", side_effect=importer),
            mock.patch.dict(os.environ, {name: "0" for name in MODULE._OFFLINE_ENV}),
        ):
            MODULE.load(
                model_root=str(self.root),
                artifact_path=str(self.sulphur),
                backend="mlx",
                cache_root=str(self.cache),
            )
        self.assertEqual(observed, MODULE._OFFLINE_ENV)

    def test_requirements_and_embedded_wheels_are_the_exact_audited_runtime_closure(self):
        requirements_path = MODULE_PATH.with_name("sulphur-mlx-runtime-requirements.txt")
        actual = {}
        for line in requirements_path.read_text(encoding="ascii").splitlines():
            self.assertEqual(line.count("=="), 1)
            name, version = line.split("==")
            actual[name] = version
        self.assertEqual(actual, MODULE._EXTERNAL_DISTRIBUTIONS)
        self.assertEqual(
            MODULE._REQUIRED_DISTRIBUTIONS,
            {
                **MODULE._EXTERNAL_DISTRIBUTIONS,
                **MODULE._EMBEDDED_DISTRIBUTIONS,
            },
        )

    def test_schedule_drift_is_rejected(self):
        with (
            mock.patch.object(MODULE, "_require_apple_silicon"),
            mock.patch.object(MODULE, "_verify_runtime_packages"),
            mock.patch.object(
                MODULE,
                "_import_pipeline",
                return_value=(
                    FakePipeline,
                    FakeImageConditioningInput,
                    (1.0, 0.0),
                    MODULE._STAGE_2_SIGMAS,
                ),
            ),
        ):
            with self.assertRaisesRegex(RuntimeError, "8\\+3 schedule"):
                MODULE.load(
                    model_root=str(self.root),
                    artifact_path=str(self.sulphur),
                    backend="mlx",
                    cache_root=str(self.cache),
                )


if __name__ == "__main__":
    unittest.main()
