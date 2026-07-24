"""Offline CUDA/GGUF runtime adapter for Sulphur-2.

The adapter intentionally treats the public GGUF as the development transformer,
not as a native distilled checkpoint.  It loads that component through the
official Diffusers LTX-2 loader, applies the pinned distillation LoRA, and uses
the fixed two-stage schedule from LTX-2 commit
9377758131b1ffde4b7f766804590a6617bf2ab9.

Every model file must be listed in ``mayhem-sulphur-runtime.json`` under the
model root.  The signed catalog remains the authority for the manifest itself;
this adapter verifies the manifest's complete local hash inventory before any
model code is loaded.  No Hub lookup or fallback checkpoint is permitted.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import io
import json
import math
import os
import pathlib
import re
import stat
import tempfile
from dataclasses import dataclass
from typing import Any


MAYHEM_SULPHUR_API_VERSION = 1

_MANIFEST_NAME = "mayhem-sulphur-runtime.json"
_MANIFEST_SCHEMA = "openmayhem.sulphur.cuda-gguf.v1"
_LTX_RUNTIME_COMMIT = "9377758131b1ffde4b7f766804590a6617bf2ab9"
_SULPHUR_SOURCE_COMMIT = "875e886e556b955d21149316fd631cc121db6cc1"
_DIFFUSERS_VERSION = "0.39.0"
_DIFFUSERS_COMMIT = "cc92165331e1b20afc1a47e03f63e8f3a930f8cc"
_DISTILLATION_MODE = "dev_transformer_plus_pinned_distill_lora"
_PROMPT_ENHANCER_ASSETS = (
    "prompt_enhancer_uncensored/prompt_enhancer_uncensored-q8_0.gguf",
    "prompt_enhancer_uncensored/mmproj-prompt_enhancer_uncensored.gguf",
)

_STAGE_1_STEPS = 8
_STAGE_1_MAX_SHIFT = 4.0
_STAGE_1_BASE_SHIFT = 1.5
_STAGE_1_TERMINAL = 0.1
_STAGE_1_LORA_WEIGHT = 0.7
_STAGE_2_SIGMAS = (0.85, 0.725, 0.4219)
_STAGE_2_LORA_WEIGHT = 0.5
_LTX_SCHEDULER_BASE_TOKENS = 1024
_LTX_SCHEDULER_MAX_TOKENS = 4096

_MAX_PROMPT_BYTES = 32 * 1024
_MAX_PROMPT_TOKENS = 1024
_MAX_INPUT_IMAGE_BYTES = 32 * 1024 * 1024
_MAX_IMAGE_PIXELS = 16 * 1024 * 1024
_MAX_CONDITION_IMAGES = 16
_MAX_MANIFEST_BYTES = 8 * 1024 * 1024
_HASH_CHUNK_BYTES = 8 * 1024 * 1024
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_CUDA_DEVICE_RE = re.compile(r"^cuda:(0|[1-9][0-9]*)$")
_WINDOWS = os.name == "nt"

_REQUIRED_DISTRIBUTIONS = {
    "accelerate": "1.12.0",
    "av": "16.1.0",
    "bitsandbytes": "0.49.1",
    "diffusers": _DIFFUSERS_VERSION,
    "gguf": "0.19.0",
    "huggingface-hub": "0.36.0",
    "numpy": "2.2.6",
    "peft": "0.18.1",
    "Pillow": "12.1.0",
    "safetensors": "0.8.0",
    "torch": "2.9.1",
    "torchvision": "0.24.1",
    "tqdm": "4.67.1",
    "tokenizers": "0.22.2",
    "transformers": "4.57.6",
}

_REQUEST_FIELDS = frozenset(
    {
        "enhance_prompt",
        "frame_rate",
        "height",
        "images",
        "negative_prompt",
        "num_frames",
        "prompt",
        "seed",
        "width",
    }
)
_IMAGE_FIELDS = frozenset(
    {
        "content_type",
        "crf",
        "frame_index",
        "path",
        "strength",
    }
)


@dataclass
class _Runtime:
    model_root: pathlib.Path
    cache_root: pathlib.Path
    artifact_path: pathlib.Path
    pipeline_root: pathlib.Path
    distillation_lora: pathlib.Path
    latent_upsampler_root: pathlib.Path
    device: str
    torch: Any
    t2v_pipeline: Any
    i2v_pipeline: Any
    upsample_pipeline: Any
    encode_video: Any
    video_condition_type: Any
    cfgpp_scheduler_type: Any
    lcm_scheduler_type: Any
    tokenizer: Any
    prompt_enhancer: dict[str, Any] | None


def _require_exact_object(value: Any, fields: set[str] | frozenset[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != set(fields):
        raise ValueError(f"{label} has unknown or missing fields")
    return value


def _absolute_local_path(
    value: str | os.PathLike[str],
    label: str,
) -> pathlib.Path:
    path = pathlib.Path(value)
    if not path.is_absolute():
        raise ValueError(f"{label} must be absolute")
    return pathlib.Path(os.path.abspath(path))


def _is_link_or_reparse(path: pathlib.Path, metadata: os.stat_result | None = None) -> bool:
    metadata = metadata or os.lstat(path)
    return stat.S_ISLNK(metadata.st_mode) or bool(
        getattr(metadata, "st_file_attributes", 0)
        & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    )


def _existing_real_path(
    value: str | os.PathLike[str],
    label: str,
) -> tuple[pathlib.Path, os.stat_result]:
    source = pathlib.Path(value)
    if _WINDOWS:
        path = _absolute_local_path(source, label)
        metadata = os.lstat(path)
        if _is_link_or_reparse(path, metadata):
            raise ValueError(f"{label} must not be a symlink or reparse point")
        return path, metadata
    if source.is_symlink():
        raise ValueError(f"{label} must not be a symlink")
    path = source.resolve(strict=True)
    return path, path.stat()


def _real_directory(value: str | os.PathLike[str], label: str) -> pathlib.Path:
    path, metadata = _existing_real_path(value, label)
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError(f"{label} must be a real directory")
    return path


def _real_file(value: str | os.PathLike[str], label: str) -> pathlib.Path:
    path, metadata = _existing_real_path(value, label)
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"{label} must be a real regular file")
    return path


def _ensure_private_directory(
    value: str | os.PathLike[str],
    label: str,
) -> pathlib.Path:
    path = pathlib.Path(value)
    if _WINDOWS:
        path.mkdir(parents=True, exist_ok=True)
    else:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(path, 0o700)
    return _real_directory(path, label)


def _contained(path: pathlib.Path, root: pathlib.Path, label: str) -> pathlib.Path:
    try:
        path.relative_to(root)
    except ValueError as error:
        raise ValueError(f"{label} escaped its bounded root") from error
    return path


def _relative_manifest_path(value: Any, label: str) -> pathlib.PurePosixPath:
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError(f"{label} must be a non-empty POSIX relative path")
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"{label} must be a normalized relative path")
    return path


def _resolve_manifest_path(root: pathlib.Path, value: Any, label: str, directory: bool) -> pathlib.Path:
    relative = _relative_manifest_path(value, label)
    current = root
    for part in relative.parts:
        current = current / part
        if _is_link_or_reparse(current):
            raise ValueError(f"{label} contains a symlink or reparse point")
    resolved = (
        _real_directory(current, label)
        if directory
        else _real_file(current, label)
    )
    _contained(resolved, root, label)
    return resolved


def _sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(_HASH_CHUNK_BYTES):
            digest.update(chunk)
    return digest.hexdigest()


def _load_and_verify_manifest(model_root: pathlib.Path, artifact_path: pathlib.Path) -> dict[str, Any]:
    model_root = _real_directory(model_root, "Sulphur model root")
    artifact_path = _real_file(artifact_path, "Sulphur GGUF artifact")
    _contained(artifact_path, model_root, "Sulphur GGUF artifact")
    manifest_path = _real_file(model_root / _MANIFEST_NAME, "Sulphur runtime manifest")
    if manifest_path.stat().st_size > _MAX_MANIFEST_BYTES:
        raise ValueError("Sulphur runtime manifest exceeds its size bound")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("Sulphur runtime manifest is not valid UTF-8 JSON") from error
    manifest = _require_exact_object(
        manifest,
        {
            "schema",
            "ltx_runtime_commit",
            "sulphur_source_commit",
            "diffusers_version",
            "diffusers_commit",
            "distillation_mode",
            "roles",
            "prompt_enhancer",
            "files",
        },
        "Sulphur runtime manifest",
    )
    expected_scalars = {
        "schema": _MANIFEST_SCHEMA,
        "ltx_runtime_commit": _LTX_RUNTIME_COMMIT,
        "sulphur_source_commit": _SULPHUR_SOURCE_COMMIT,
        "diffusers_version": _DIFFUSERS_VERSION,
        "diffusers_commit": _DIFFUSERS_COMMIT,
        "distillation_mode": _DISTILLATION_MODE,
    }
    for field, expected in expected_scalars.items():
        if manifest[field] != expected:
            raise ValueError(f"Sulphur runtime manifest has an unexpected {field}")

    roles = _require_exact_object(
        manifest["roles"],
        {"transformer_gguf", "distillation_lora", "pipeline_root", "latent_upsampler"},
        "Sulphur runtime roles",
    )
    files = manifest["files"]
    if not isinstance(files, dict) or not files:
        raise ValueError("Sulphur runtime files must be a non-empty object")

    normalized_entries: dict[str, dict[str, Any]] = {}
    for relative_text, entry in files.items():
        relative = _relative_manifest_path(relative_text, "Sulphur runtime file").as_posix()
        if relative == _MANIFEST_NAME or relative in normalized_entries:
            raise ValueError("Sulphur runtime file inventory contains a duplicate or self-entry")
        entry = _require_exact_object(entry, {"sha256", "size"}, f"Sulphur file {relative}")
        if not isinstance(entry["size"], int) or isinstance(entry["size"], bool) or entry["size"] < 0:
            raise ValueError(f"Sulphur file {relative} has an invalid size")
        if not isinstance(entry["sha256"], str) or not _SHA256_RE.fullmatch(entry["sha256"]):
            raise ValueError(f"Sulphur file {relative} has an invalid SHA-256")
        normalized_entries[relative] = entry

    actual_files: set[str] = set()
    for item in model_root.rglob("*"):
        if item.is_symlink():
            raise ValueError("Sulphur model root contains a symlink")
        if item.is_file():
            relative = item.relative_to(model_root).as_posix()
            if relative != _MANIFEST_NAME:
                actual_files.add(relative)
    if actual_files != set(normalized_entries):
        missing = sorted(set(normalized_entries) - actual_files)
        extra = sorted(actual_files - set(normalized_entries))
        raise ValueError(f"Sulphur file inventory differs from disk (missing={missing}, extra={extra})")

    for relative, entry in normalized_entries.items():
        path = _resolve_manifest_path(model_root, relative, f"Sulphur file {relative}", directory=False)
        if path.stat().st_size != entry["size"]:
            raise ValueError(f"Sulphur file {relative} has an unexpected size")
        if _sha256_file(path) != entry["sha256"]:
            raise ValueError(f"Sulphur file {relative} failed SHA-256 verification")

    transformer = _resolve_manifest_path(
        model_root, roles["transformer_gguf"], "Sulphur GGUF transformer", directory=False
    )
    if transformer != artifact_path or transformer.suffix.lower() != ".gguf":
        raise ValueError("Sulphur artifact is not the manifest-pinned GGUF transformer")
    lora = _resolve_manifest_path(
        model_root, roles["distillation_lora"], "Sulphur distillation LoRA", directory=False
    )
    if lora.suffix.lower() != ".safetensors":
        raise ValueError("Sulphur distillation LoRA must be a safetensors file")
    pipeline_root = _resolve_manifest_path(
        model_root, roles["pipeline_root"], "Sulphur Diffusers pipeline root", directory=True
    )
    upsampler = _resolve_manifest_path(
        model_root, roles["latent_upsampler"], "Sulphur latent upsampler", directory=True
    )
    _contained(upsampler, pipeline_root, "Sulphur latent upsampler")
    for required in (
        pipeline_root / "model_index.json",
        pipeline_root / "transformer" / "config.json",
        upsampler / "config.json",
    ):
        relative = required.relative_to(model_root).as_posix()
        if relative not in normalized_entries:
            raise ValueError(f"Sulphur runtime inventory is missing required component {relative}")

    prompt_enhancer = manifest["prompt_enhancer"]
    if prompt_enhancer is not None:
        prompt_enhancer = _require_exact_object(
            prompt_enhancer,
            {"asset_paths", "system_prompt"},
            "Sulphur prompt enhancer",
        )
        if not isinstance(prompt_enhancer["system_prompt"], str):
            raise ValueError("Sulphur prompt enhancer system_prompt must be a string")
        if len(prompt_enhancer["system_prompt"].encode("utf-8")) > _MAX_PROMPT_BYTES:
            raise ValueError("Sulphur prompt enhancer system_prompt exceeds its bound")
        if prompt_enhancer["system_prompt"]:
            raise ValueError("Sulphur prompt enhancer source declares no system prompt")
        assets = prompt_enhancer["asset_paths"]
        if assets != list(_PROMPT_ENHANCER_ASSETS):
            raise ValueError("Sulphur prompt enhancer assets are not canonical")
        seen: set[str] = set()
        for index, asset in enumerate(assets):
            relative = _relative_manifest_path(asset, f"Sulphur prompt enhancer asset {index}").as_posix()
            if relative in seen or relative not in normalized_entries:
                raise ValueError("Sulphur prompt enhancer contains a duplicate or unpinned asset")
            seen.add(relative)

    manifest["_resolved"] = {
        "distillation_lora": lora,
        "pipeline_root": pipeline_root,
        "latent_upsampler": upsampler,
        "prompt_enhancer": prompt_enhancer,
    }
    return manifest


def _verify_distribution_versions() -> None:
    for distribution, expected in _REQUIRED_DISTRIBUTIONS.items():
        try:
            actual = importlib.metadata.version(distribution)
        except importlib.metadata.PackageNotFoundError as error:
            raise RuntimeError(f"Sulphur runtime dependency {distribution} is not installed") from error
        if actual.split("+", 1)[0] != expected:
            raise RuntimeError(
                f"Sulphur runtime dependency {distribution} must be {expected}, found {actual}"
            )


def _configure_offline_cache(cache_root: pathlib.Path) -> None:
    hub_root = cache_root / "huggingface"
    hub_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    values = {
        "DIFFUSERS_OFFLINE": "1",
        "HF_DATASETS_OFFLINE": "1",
        "HF_HUB_DISABLE_TELEMETRY": "1",
        "HF_HUB_OFFLINE": "1",
        "TRANSFORMERS_OFFLINE": "1",
        # Optional GGUF CUDA kernels alter numerics and are not part of this pin.
        "DIFFUSERS_GGUF_CUDA_KERNELS": "false",
        "HF_HOME": str(hub_root),
        "HF_HUB_CACHE": str(hub_root / "hub"),
        "TRANSFORMERS_CACHE": str(hub_root / "transformers"),
    }
    for key, value in values.items():
        os.environ[key] = value


def _require_prequantized_text_encoder(pipeline: Any) -> None:
    text_encoder = getattr(pipeline, "text_encoder", None)
    if text_encoder is None or getattr(text_encoder, "is_loaded_in_4bit", False) is not True:
        raise RuntimeError("Sulphur text encoder did not remain serialized 4-bit")
    config = getattr(text_encoder, "config", None)
    quantization = getattr(config, "quantization_config", None)
    if hasattr(quantization, "to_dict"):
        quantization = quantization.to_dict()
    expected = {
        "quant_method": "bitsandbytes",
        "load_in_4bit": True,
        "bnb_4bit_compute_dtype": "bfloat16",
        "bnb_4bit_quant_storage": "uint8",
        "bnb_4bit_quant_type": "nf4",
        "bnb_4bit_use_double_quant": True,
    }
    if not isinstance(quantization, dict) or any(
        quantization.get(field) != value for field, value in expected.items()
    ):
        raise RuntimeError("Sulphur text encoder quantization differs from the pinned NF4 profile")


def _creator_stage_1_sigmas(
    num_frames: int,
    height: int,
    width: int,
    temporal_compression_ratio: int,
    spatial_compression_ratio: int,
) -> tuple[float, ...]:
    """Reproduce the creator workflow's LTXVScheduler for the stage-1 latent."""
    latent_frames = (num_frames - 1) // temporal_compression_ratio + 1
    latent_height = height // spatial_compression_ratio
    latent_width = width // spatial_compression_ratio
    if min(latent_frames, latent_height, latent_width) <= 0:
        raise ValueError("Sulphur stage-1 latent geometry is invalid")
    tokens = latent_frames * latent_height * latent_width
    slope = (_STAGE_1_MAX_SHIFT - _STAGE_1_BASE_SHIFT) / (
        _LTX_SCHEDULER_MAX_TOKENS - _LTX_SCHEDULER_BASE_TOKENS
    )
    intercept = _STAGE_1_BASE_SHIFT - slope * _LTX_SCHEDULER_BASE_TOKENS
    sigma_shift = tokens * slope + intercept
    shift_exp = math.exp(sigma_shift)
    raw_sigmas = [1.0 - index / _STAGE_1_STEPS for index in range(_STAGE_1_STEPS + 1)]
    shifted = [
        shift_exp / (shift_exp + (1.0 / sigma - 1.0)) if sigma else 0.0
        for sigma in raw_sigmas
    ]
    nonzero = shifted[:-1]
    stretch_scale = (1.0 - nonzero[-1]) / (1.0 - _STAGE_1_TERMINAL)
    if not math.isfinite(stretch_scale) or stretch_scale <= 0:
        raise ValueError("Sulphur stage-1 scheduler stretch is invalid")
    return tuple(1.0 - (1.0 - sigma) / stretch_scale for sigma in nonzero)


def _stage_seed(seed: int, stage: str) -> int:
    digest = hashlib.sha256(f"openmayhem.sulphur.seed.v1:{stage}:{seed}".encode("ascii")).digest()
    return int.from_bytes(digest[:8], "big") & ((1 << 63) - 1)


def _sampler_seed(seed: int, stage: str) -> int:
    return _stage_seed(seed, f"{stage}-sampler")


def _lora_pair_count(names: Any) -> int:
    names = set(names)
    suffix_a = ".lora_A.weight"
    suffix_b = ".lora_B.weight"
    pairs_a = {name[: -len(suffix_a)] for name in names if name.endswith(suffix_a)}
    pairs_b = {name[: -len(suffix_b)] for name in names if name.endswith(suffix_b)}
    if not pairs_a or pairs_a != pairs_b:
        raise RuntimeError("the pinned Sulphur distillation LoRA has incomplete A/B pairs")
    return len(pairs_a)


def _restore_creator_lora_scaling(
    model: Any,
    adapter_name: str,
    expected_layers: int,
) -> None:
    adjusted = 0
    for module in model.modules():
        ranks = getattr(module, "r", None)
        if not isinstance(ranks, dict) or adapter_name not in ranks:
            continue
        alphas = getattr(module, "lora_alpha", None)
        scalings = getattr(module, "scaling", None)
        use_rslora = getattr(module, "use_rslora", None)
        if not isinstance(alphas, dict) or not isinstance(scalings, dict):
            raise RuntimeError("the pinned PEFT LoRA layer has no mutable scaling state")
        if isinstance(use_rslora, dict) and use_rslora.get(adapter_name, False):
            raise RuntimeError("the pinned Sulphur distillation LoRA unexpectedly uses rsLoRA")
        rank = ranks[adapter_name]
        if isinstance(rank, bool) or not isinstance(rank, int) or rank <= 0:
            raise RuntimeError("the pinned Sulphur distillation LoRA has an invalid rank")
        # The creator and Comfy paths apply strength * B(Ax), without alpha/r.
        alphas[adapter_name] = rank
        scalings[adapter_name] = 1.0
        adjusted += 1
    if adjusted != expected_layers:
        raise RuntimeError(
            "the pinned Sulphur distillation LoRA layer count does not match "
            f"the loaded adapter ({adjusted} != {expected_layers})"
        )


def _cfgpp_ancestral_coefficients(
    sigma: float,
    sigma_next: float,
) -> tuple[float, float, float, float]:
    if not (0.0 < sigma <= 1.0 and 0.0 < sigma_next < sigma):
        raise ValueError("Sulphur CFG++ sigmas must descend strictly inside the flow domain")
    alpha_s = 1.0 - sigma
    alpha_t = 1.0 - sigma_next
    sigma_to = sigma_next / alpha_t
    if alpha_s <= 1e-12:
        return alpha_s, alpha_t, 0.0, sigma_to
    sigma_from = sigma / alpha_s
    variance = max(
        0.0,
        sigma_to
        * sigma_to
        * (sigma_from * sigma_from - sigma_to * sigma_to)
        / (sigma_from * sigma_from),
    )
    sigma_up = min(sigma_to, math.sqrt(variance))
    sigma_down = math.sqrt(max(0.0, sigma_to * sigma_to - sigma_up * sigma_up))
    return alpha_s, alpha_t, sigma_down, sigma_up


def _scheduler_types(
    torch: Any,
    scheduler_base: Any,
    scheduler_output: Any,
) -> tuple[Any, Any]:
    class _DeterministicFlowScheduler(scheduler_base):
        _mayhem_force_cfg = False

        def __init__(self, *args: Any, **kwargs: Any) -> None:
            super().__init__(*args, **kwargs)
            self._mayhem_seed = 0
            self._mayhem_stream = 0

        def __deepcopy__(self, memo: dict[int, Any]) -> Any:
            clone = type(self).from_config(self.config)
            clone._mayhem_seed = self._mayhem_seed
            clone._mayhem_stream = self._mayhem_stream + 1
            memo[id(self)] = clone
            return clone

        def configure_seed(self, seed: int) -> None:
            self._mayhem_seed = seed
            self._mayhem_stream = 0

        def _noise_like(self, sample: Any, step_index: int) -> Any:
            stream_seed = (
                self._mayhem_seed
                + self._mayhem_stream * 0x1F123BB5
                + step_index * 0x6A09E667
            ) & ((1 << 63) - 1)
            generator = torch.Generator(device=sample.device).manual_seed(stream_seed)
            return torch.randn(
                sample.size(),
                dtype=sample.dtype,
                layout=sample.layout,
                device=sample.device,
                generator=generator,
            )

        @staticmethod
        def _finish(prev_sample: Any, model_output: Any, return_dict: bool) -> Any:
            prev_sample = prev_sample.to(model_output.dtype)
            if not return_dict:
                return (prev_sample,)
            return scheduler_output(prev_sample=prev_sample)

    class _EulerAncestralCfgPpFlowScheduler(_DeterministicFlowScheduler):
        _mayhem_force_cfg = True

        def __init__(self, *args: Any, **kwargs: Any) -> None:
            super().__init__(*args, **kwargs)
            self._mayhem_x0: list[Any] = []

        def set_timesteps(
            self,
            num_inference_steps: int | None = None,
            device: Any = None,
            sigmas: list[float] | None = None,
            mu: float | None = None,
            timesteps: list[float] | None = None,
        ) -> None:
            super().set_timesteps(
                num_inference_steps=num_inference_steps,
                device=device,
                sigmas=sigmas,
                mu=mu,
                timesteps=timesteps,
            )
            self._mayhem_x0 = []

        def capture_x0(self, value: Any) -> None:
            if len(self._mayhem_x0) >= 2:
                raise RuntimeError("Sulphur CFG++ captured an unexpected extra denoised prediction")
            self._mayhem_x0.append(value)

        def step(
            self,
            model_output: Any,
            timestep: Any,
            sample: Any,
            return_dict: bool = True,
            **kwargs: Any,
        ) -> Any:
            if self.step_index is None:
                self._init_step_index(timestep)
            if len(self._mayhem_x0) != 2:
                raise RuntimeError("Sulphur CFG++ requires conditional and unconditional predictions")
            step_index = int(self.step_index)
            sigma = float(self.sigmas[step_index])
            sigma_next = float(self.sigmas[step_index + 1])
            sample_f = sample.to(torch.float32)
            denoised = sample_f - sigma * model_output.to(torch.float32)
            uncond_denoised = self._mayhem_x0[1].to(torch.float32)
            self._mayhem_x0 = []

            if sigma_next == 0.0:
                prev_sample = denoised
            else:
                alpha_s, alpha_t, sigma_down, sigma_up = _cfgpp_ancestral_coefficients(
                    sigma,
                    sigma_next,
                )
                derivative = (sample_f - alpha_s * uncond_denoised) / sigma
                prev_sample = alpha_t * denoised + alpha_t * sigma_down * derivative
                if sigma_up > 0:
                    prev_sample = (
                        prev_sample
                        + alpha_t
                        * sigma_up
                        * self._noise_like(sample_f, step_index)
                    )

            self._step_index += 1
            return self._finish(prev_sample, model_output, return_dict)

    class _LcmFlowScheduler(_DeterministicFlowScheduler):
        def step(
            self,
            model_output: Any,
            timestep: Any,
            sample: Any,
            return_dict: bool = True,
            **kwargs: Any,
        ) -> Any:
            if self.step_index is None:
                self._init_step_index(timestep)
            step_index = int(self.step_index)
            sigma = float(self.sigmas[step_index])
            sigma_next = float(self.sigmas[step_index + 1])
            sample_f = sample.to(torch.float32)
            denoised = sample_f - sigma * model_output.to(torch.float32)
            if sigma_next > 0:
                prev_sample = (
                    (1.0 - sigma_next) * denoised
                    + sigma_next * self._noise_like(sample_f, step_index)
                )
            else:
                prev_sample = denoised
            self._step_index += 1
            return self._finish(prev_sample, model_output, return_dict)

    return _EulerAncestralCfgPpFlowScheduler, _LcmFlowScheduler


def _load_diffusers_runtime(
    artifact_path: pathlib.Path,
    pipeline_root: pathlib.Path,
    lora_path: pathlib.Path,
    upsampler_root: pathlib.Path,
    device: str,
) -> tuple[Any, Any, Any, Any, Any]:
    try:
        import torch
        from diffusers import (
            FlowMatchEulerDiscreteScheduler,
            GGUFQuantizationConfig,
            LTX2ConditionPipeline,
            LTX2VideoTransformer3DModel,
        )
        from diffusers.loaders import single_file_model
        from diffusers.pipelines.ltx2.latent_upsampler import LTX2LatentUpsamplerModel
        from diffusers.pipelines.ltx2.pipeline_ltx2_condition import LTX2VideoCondition
        from diffusers.pipelines.ltx2.pipeline_ltx2_latent_upsample import LTX2LatentUpsamplePipeline
        from diffusers.schedulers.scheduling_flow_match_euler_discrete import (
            FlowMatchEulerDiscreteSchedulerOutput,
        )
        from diffusers.utils import encode_video
        from safetensors import safe_open
    except Exception as error:
        raise RuntimeError("importing the pinned Diffusers LTX-2 runtime failed") from error

    if not torch.cuda.is_available():
        raise RuntimeError("Sulphur CUDA/GGUF requires an available CUDA device")
    device_index = int(device.partition(":")[2])
    if device_index >= torch.cuda.device_count():
        raise RuntimeError(f"Sulphur CUDA device {device} does not exist")
    if not torch.cuda.is_bf16_supported(device_index):
        raise RuntimeError("Sulphur CUDA/GGUF requires bfloat16-capable CUDA hardware")

    dtype = torch.bfloat16
    try:
        cfgpp_scheduler_type, lcm_scheduler_type = _scheduler_types(
            torch,
            FlowMatchEulerDiscreteScheduler,
            FlowMatchEulerDiscreteSchedulerOutput,
        )

        class _CreatorSamplerPipelineMixin:
            @property
            def do_classifier_free_guidance(self) -> bool:
                return bool(
                    super().do_classifier_free_guidance
                    or getattr(self.scheduler, "_mayhem_force_cfg", False)
                )

            def convert_velocity_to_x0(
                self,
                sample: Any,
                denoised_output: Any,
                step_idx: int,
                scheduler: Any | None = None,
            ) -> Any:
                if scheduler is None:
                    scheduler = self.scheduler
                value = super().convert_velocity_to_x0(
                    sample,
                    denoised_output,
                    step_idx,
                    scheduler,
                )
                capture = getattr(scheduler, "capture_x0", None)
                if capture is not None:
                    capture(value)
                return value

        class _MayhemLTX2ConditionPipeline(
            _CreatorSamplerPipelineMixin,
            LTX2ConditionPipeline,
        ):
            pass

        mapping = single_file_model.SINGLE_FILE_LOADABLE_CLASSES.get(
            "LTX2VideoTransformer3DModel"
        )
        if not isinstance(mapping, dict) or not callable(mapping.get("checkpoint_mapping_fn")):
            raise RuntimeError("the pinned Diffusers LTX-2 single-file mapping is unavailable")
        original_mapping = mapping["checkpoint_mapping_fn"]

        def corrected_mapping(checkpoint: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
            converted = original_mapping(checkpoint, **kwargs)
            renamed = 0
            for old_prefix, new_prefix in (
                ("prompt_adaln_single.", "prompt_adaln."),
                ("audio_prompt_adaln_single.", "audio_prompt_adaln."),
            ):
                for old_name in [name for name in converted if name.startswith(old_prefix)]:
                    new_name = new_prefix + old_name.removeprefix(old_prefix)
                    if new_name in converted:
                        raise RuntimeError(
                            f"the pinned Diffusers LTX-2 mapping collides at {new_name}"
                        )
                    converted[new_name] = converted.pop(old_name)
                    renamed += 1
            if renamed != 12:
                raise RuntimeError(
                    "the pinned Sulphur GGUF did not expose its 12 prompt AdaLN tensors"
                )
            return converted

        mapping["checkpoint_mapping_fn"] = corrected_mapping
        try:
            transformer = LTX2VideoTransformer3DModel.from_single_file(
                str(artifact_path),
                config=str(pipeline_root),
                subfolder="transformer",
                quantization_config=GGUFQuantizationConfig(compute_dtype=dtype),
                torch_dtype=dtype,
                local_files_only=True,
                low_cpu_mem_usage=True,
            )
        finally:
            mapping["checkpoint_mapping_fn"] = original_mapping
        unresolved = [
            name
            for name, parameter in transformer.named_parameters()
            if parameter.device.type == "meta"
        ]
        if unresolved:
            raise RuntimeError(
                "the pinned Sulphur GGUF left unmaterialized tensors: "
                + ", ".join(unresolved)
            )
        condition_pipeline = _MayhemLTX2ConditionPipeline.from_pretrained(
            str(pipeline_root),
            transformer=transformer,
            torch_dtype=dtype,
            local_files_only=True,
            low_cpu_mem_usage=True,
        )
        _require_prequantized_text_encoder(condition_pipeline)
        with safe_open(lora_path, framework="pt", device="cpu") as lora_file:
            expected_lora_layers = _lora_pair_count(lora_file.keys())
        condition_pipeline.load_lora_weights(
            str(lora_path.parent),
            weight_name=lora_path.name,
            adapter_name="mayhem_distilled",
            local_files_only=True,
        )
        _restore_creator_lora_scaling(
            condition_pipeline.transformer,
            "mayhem_distilled",
            expected_lora_layers,
        )
        condition_pipeline.set_adapters("mayhem_distilled", _STAGE_1_LORA_WEIGHT)
        condition_pipeline.scheduler = cfgpp_scheduler_type.from_config(
            condition_pipeline.scheduler.config,
            use_dynamic_shifting=False,
            shift_terminal=None,
        )
        condition_pipeline.vae.enable_tiling()
        latent_upsampler = LTX2LatentUpsamplerModel.from_pretrained(
            str(upsampler_root),
            torch_dtype=dtype,
            local_files_only=True,
            low_cpu_mem_usage=True,
        )
        upsample = LTX2LatentUpsamplePipeline(
            vae=condition_pipeline.vae,
            latent_upsampler=latent_upsampler,
        )
        condition_pipeline.enable_model_cpu_offload(device=device)
        upsample.enable_model_cpu_offload(device=device)
    except Exception as error:
        raise RuntimeError(
            "Diffusers 0.39.0 could not safely load the manifest-pinned Sulphur GGUF, "
            "fixed distillation LoRA, and local LTX-2 sidecars; no fallback checkpoint "
            f"or loader was attempted: {type(error).__name__}: {error}"
        ) from error
    return (
        torch,
        condition_pipeline,
        condition_pipeline,
        upsample,
        encode_video,
        LTX2VideoCondition,
        cfgpp_scheduler_type,
        lcm_scheduler_type,
    )


def load(model_root: str, artifact_path: str, backend: str, cache_root: str) -> _Runtime:
    if backend != "gguf":
        raise ValueError("this Sulphur runtime adapter supports only the CUDA/GGUF backend")
    root = _real_directory(model_root, "Sulphur model root")
    artifact = _real_file(artifact_path, "Sulphur GGUF artifact")
    _contained(artifact, root, "Sulphur GGUF artifact")
    cache = _real_directory(cache_root, "Sulphur cache root")
    if cache == root or root in cache.parents or cache in root.parents:
        raise ValueError("Sulphur cache root and immutable model root must be disjoint")

    device = os.environ.get("MAYHEM_SULPHUR_CUDA_DEVICE", "cuda:0")
    if not _CUDA_DEVICE_RE.fullmatch(device):
        raise ValueError("MAYHEM_SULPHUR_CUDA_DEVICE must be cuda:N")
    _configure_offline_cache(cache)
    _verify_distribution_versions()
    manifest = _load_and_verify_manifest(root, artifact)
    resolved = manifest["_resolved"]
    (
        torch,
        t2v,
        i2v,
        upsample,
        encode_video,
        video_condition_type,
        cfgpp_scheduler_type,
        lcm_scheduler_type,
    ) = _load_diffusers_runtime(
        artifact,
        resolved["pipeline_root"],
        resolved["distillation_lora"],
        resolved["latent_upsampler"],
        device,
    )
    return _Runtime(
        model_root=root,
        cache_root=cache,
        artifact_path=artifact,
        pipeline_root=resolved["pipeline_root"],
        distillation_lora=resolved["distillation_lora"],
        latent_upsampler_root=resolved["latent_upsampler"],
        device=device,
        torch=torch,
        t2v_pipeline=t2v,
        i2v_pipeline=i2v,
        upsample_pipeline=upsample,
        encode_video=encode_video,
        video_condition_type=video_condition_type,
        cfgpp_scheduler_type=cfgpp_scheduler_type,
        lcm_scheduler_type=lcm_scheduler_type,
        tokenizer=t2v.tokenizer,
        prompt_enhancer=resolved["prompt_enhancer"],
    )


def describe(runtime: _Runtime) -> dict[str, Any]:
    if not isinstance(runtime, _Runtime):
        raise TypeError("Sulphur runtime is invalid")
    return {
        "api_version": MAYHEM_SULPHUR_API_VERSION,
        "runtime_name": "diffusers-ltx2-cuda-gguf",
        "runtime_version": _DIFFUSERS_VERSION,
        "backend": "gguf",
        "distilled": True,
        "joint_audio_video": True,
        "prompt_enhancer": False,
        "ltx_runtime_commit": _LTX_RUNTIME_COMMIT,
        "sulphur_source_commit": _SULPHUR_SOURCE_COMMIT,
        "distillation_mode": _DISTILLATION_MODE,
        "stage_1_denoise_intervals": _STAGE_1_STEPS,
        "stage_2_denoise_intervals": len(_STAGE_2_SIGMAS),
    }


def _request_fields(request: Any) -> dict[str, Any]:
    return _require_exact_object(request, _REQUEST_FIELDS, "Sulphur request")


def _require_prompt_within_token_limit(tokenizer: Any, value: str, label: str) -> None:
    if tokenizer is None or not callable(getattr(tokenizer, "encode", None)):
        raise RuntimeError("Sulphur tokenizer is unavailable for no-truncation validation")
    try:
        token_ids = tokenizer.encode(value.strip(), add_special_tokens=True)
    except Exception as error:
        raise RuntimeError(f"Sulphur {label} tokenization failed") from error
    if not isinstance(token_ids, (list, tuple)) or any(
        not isinstance(token_id, int) or isinstance(token_id, bool)
        for token_id in token_ids
    ):
        raise RuntimeError(f"Sulphur {label} tokenizer returned an invalid token sequence")
    if len(token_ids) > _MAX_PROMPT_TOKENS:
        raise ValueError(
            f"Sulphur {label} contains {len(token_ids)} tokens; "
            f"the exact model limit is {_MAX_PROMPT_TOKENS} and truncation is forbidden"
        )


def _materialized_images(
    runtime: _Runtime,
    images: Any,
    num_frames: int,
) -> list[dict[str, Any]]:
    if not isinstance(images, list) or len(images) > _MAX_CONDITION_IMAGES:
        raise ValueError(
            f"Sulphur images must contain at most {_MAX_CONDITION_IMAGES} ordered conditions"
        )
    if not images:
        return []
    inputs_root = _real_directory(runtime.cache_root / "inputs", "Sulphur input image directory")
    materialized = []
    for index, raw_image in enumerate(images):
        label = f"Sulphur materialized image {index}"
        image_value = _require_exact_object(raw_image, _IMAGE_FIELDS, label)
        content_type = image_value["content_type"]
        if content_type not in ("image/png", "image/jpeg"):
            raise ValueError(f"{label} has an unsupported content type")
        frame_index = image_value["frame_index"]
        if (
            not isinstance(frame_index, int)
            or isinstance(frame_index, bool)
            or not 0 <= frame_index < num_frames
        ):
            raise ValueError(f"{label} frame_index must identify an output frame")
        strength = image_value["strength"]
        if (
            not isinstance(strength, (int, float))
            or isinstance(strength, bool)
            or not math.isfinite(float(strength))
            or not 0.0 <= float(strength) <= 1.0
        ):
            raise ValueError(f"{label} strength must be finite and between 0 and 1")
        crf = image_value["crf"]
        if (
            not isinstance(crf, int)
            or isinstance(crf, bool)
            or not 0 <= crf <= 51
        ):
            raise ValueError(f"{label} crf must be an integer between 0 and 51")
        value = image_value["path"]
        if not isinstance(value, str) or not value:
            raise ValueError(f"{label} path must be non-empty text")
        image = _real_file(value, f"{label} input file")
        _contained(image, inputs_root, f"{label} input file")
        expected_suffix = ".png" if content_type == "image/png" else ".jpg"
        if image.suffix.lower() != expected_suffix:
            raise ValueError(f"{label} suffix does not match its content type")
        if image.stat().st_size <= 0 or image.stat().st_size > _MAX_INPUT_IMAGE_BYTES:
            raise ValueError(f"{label} is outside its size bound")
        with image.open("rb") as stream:
            signature = stream.read(8)
        signature_ok = (
            content_type == "image/png" and signature == b"\x89PNG\r\n\x1a\n"
        ) or (content_type == "image/jpeg" and signature.startswith(b"\xff\xd8\xff"))
        if not signature_ok:
            raise ValueError(f"{label} bytes do not match its content type")
        materialized.append(
            {
                "crf": crf,
                "frame_index": frame_index,
                "path": image,
                "strength": float(strength),
            }
        )
    return materialized


def validate_video(runtime: _Runtime, request: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(runtime, _Runtime):
        raise TypeError("Sulphur runtime is invalid")
    request = _request_fields(request)
    prompt = request["prompt"]
    if (
        not isinstance(prompt, str)
        or not prompt.strip()
        or not 0 < len(prompt.encode("utf-8")) <= _MAX_PROMPT_BYTES
    ):
        raise ValueError("Sulphur prompt must contain 1 to 32768 UTF-8 bytes")
    _require_prompt_within_token_limit(runtime.tokenizer, prompt, "prompt")
    negative_prompt = request["negative_prompt"]
    if (
        not isinstance(negative_prompt, str)
        or len(negative_prompt.encode("utf-8")) > _MAX_PROMPT_BYTES
    ):
        raise ValueError("Sulphur negative_prompt must contain at most 32768 UTF-8 bytes")
    if negative_prompt:
        _require_prompt_within_token_limit(
            runtime.tokenizer,
            negative_prompt,
            "negative_prompt",
        )
    for field in ("width", "height", "num_frames", "seed"):
        if not isinstance(request[field], int) or isinstance(request[field], bool):
            raise ValueError(f"Sulphur {field} must be an integer")
    if not 256 <= request["width"] <= 2048 or request["width"] % 64:
        raise ValueError("Sulphur width must be a multiple of 64 between 256 and 2048")
    if not 256 <= request["height"] <= 2048 or request["height"] % 64:
        raise ValueError("Sulphur height must be a multiple of 64 between 256 and 2048")
    frames = request["num_frames"]
    if not 1 <= frames <= 513 or (frames - 1) % 8:
        raise ValueError("Sulphur num_frames must be an 8k+1 value between 1 and 513")
    fps = request["frame_rate"]
    if not isinstance(fps, (int, float)) or isinstance(fps, bool) or not math.isfinite(fps):
        raise ValueError("Sulphur frame_rate must be finite")
    if not 1 <= fps <= 50:
        raise ValueError("Sulphur frame_rate must be between 1 and 50")
    if not 0 <= request["seed"] <= 0xFFFFFFFF:
        raise ValueError("Sulphur seed must fit in an unsigned 32-bit integer")
    if not isinstance(request["enhance_prompt"], bool):
        raise ValueError("Sulphur enhance_prompt must be boolean")
    if request["enhance_prompt"]:
        raise ValueError(
            "Sulphur prompt enhancement must be completed by the verified llama.cpp host compositor"
        )
    _materialized_images(runtime, request["images"], frames)
    return {
        "handled_controls": sorted(_REQUEST_FIELDS),
        "valid": True,
    }


def _load_image(path: pathlib.Path, crf: int) -> Any:
    try:
        import av
        import numpy as np
        from PIL import Image
    except ImportError as error:
        raise RuntimeError("Sulphur image-conditioning dependencies are unavailable") from error
    Image.MAX_IMAGE_PIXELS = _MAX_IMAGE_PIXELS
    try:
        with Image.open(path) as opened:
            opened.verify()
        with Image.open(path) as opened:
            image = opened.convert("RGB")
            image.load()
    except Exception as error:
        raise ValueError("Sulphur input image could not be decoded safely") from error
    if image.width * image.height > _MAX_IMAGE_PIXELS:
        raise ValueError("Sulphur input image exceeds its pixel bound")
    if crf:
        array = np.asarray(image)
        array = array[: (array.shape[0] // 2) * 2, : (array.shape[1] // 2) * 2]
        if min(array.shape[:2]) <= 0:
            raise ValueError("Sulphur input image is too small for H.264 conditioning")
        encoded = io.BytesIO()
        with av.open(encoded, "w", format="mp4") as container:
            stream = container.add_stream(
                "libx264",
                rate=1,
                options={"crf": str(crf), "preset": "veryfast"},
            )
            stream.height = array.shape[0]
            stream.width = array.shape[1]
            frame = av.VideoFrame.from_ndarray(array, format="rgb24").reformat(format="yuv420p")
            container.mux(stream.encode(frame))
            container.mux(stream.encode())
        encoded.seek(0)
        with av.open(encoded, "r") as container:
            decoded = next(container.decode(video=0)).to_ndarray(format="rgb24")
        image = Image.fromarray(decoded, mode="RGB")
    return image


def _validate_generated_tensors(video: Any, audio: Any, frames: int, sample_rate: int, fps: float) -> None:
    if getattr(video, "shape", None) is None or len(video.shape) != 4 or int(video.shape[0]) != frames:
        raise RuntimeError("Sulphur Diffusers output has an unexpected video shape")
    if getattr(audio, "ndim", None) != 2 or int(audio.numel()) == 0:
        raise RuntimeError("Sulphur Diffusers output has an unexpected audio shape")
    if int(audio.shape[0]) == 2:
        samples = int(audio.shape[1])
    elif int(audio.shape[1]) == 2:
        samples = int(audio.shape[0])
    else:
        raise RuntimeError("Sulphur Diffusers output must contain exactly two audio channels")
    peak = float(audio.detach().float().abs().max().cpu().item())
    if not math.isfinite(peak) or peak <= 1e-6:
        raise RuntimeError("Sulphur Diffusers output audio is silent")
    audio_duration = samples / sample_rate
    video_duration = frames / fps
    if abs(audio_duration - video_duration) > 1.0 / fps + 1e-6:
        raise RuntimeError("Sulphur Diffusers output audio/video duration delta exceeds one frame")


def generate_video(
    runtime: _Runtime,
    request: dict[str, Any],
    output_path: str,
) -> dict[str, Any]:
    evidence = validate_video(runtime, request)
    output = pathlib.Path(output_path)
    if not output.is_absolute() or output.suffix.lower() != ".mp4":
        raise ValueError("Sulphur output_path must be an absolute MP4 path")
    outputs_root = _ensure_private_directory(
        runtime.cache_root / "outputs",
        "Sulphur output directory",
    )
    resolved_output = (
        _absolute_local_path(output, "Sulphur output path")
        if _WINDOWS
        else output.resolve(strict=False)
    )
    if resolved_output.parent != outputs_root or output.exists() or output.is_symlink():
        raise ValueError("Sulphur output_path escaped its fresh bounded output directory")

    materialized_images = _materialized_images(
        runtime,
        request["images"],
        request["num_frames"],
    )
    loaded_conditions = [
        {
            **condition,
            "image": _load_image(condition["path"], condition["crf"]),
        }
        for condition in materialized_images
    ]
    pipeline = runtime.i2v_pipeline if loaded_conditions else runtime.t2v_pipeline
    prompt = request["prompt"]

    torch = runtime.torch
    stage_1_seed = _stage_seed(request["seed"], "stage-1")
    stage_2_seed = _stage_seed(request["seed"], "stage-2")
    stage_1_generator = torch.Generator(device=runtime.device).manual_seed(stage_1_seed)
    stage_2_generator = torch.Generator(device=runtime.device).manual_seed(stage_2_seed)
    stage_1_sigmas = _creator_stage_1_sigmas(
        request["num_frames"],
        request["height"] // 2,
        request["width"] // 2,
        int(pipeline.vae_temporal_compression_ratio),
        int(pipeline.vae_spatial_compression_ratio),
    )
    temporal_scale = int(pipeline.vae_temporal_compression_ratio)
    stage_1_conditions = [
        runtime.video_condition_type(
            frames=condition["image"],
            index=(
                0
                if condition["frame_index"] == 0
                else (condition["frame_index"] - 1) / temporal_scale + 1
            ),
            strength=condition["strength"] * 0.8,
        )
        for condition in loaded_conditions
    ]
    stage_2_conditions = [
        runtime.video_condition_type(
            frames=condition["image"],
            index=(
                0
                if condition["frame_index"] == 0
                else (condition["frame_index"] - 1) / temporal_scale + 1
            ),
            strength=condition["strength"],
        )
        for condition in loaded_conditions
    ]
    with torch.inference_mode():
        pipeline.set_adapters("mayhem_distilled", _STAGE_1_LORA_WEIGHT)
        pipeline.scheduler = runtime.cfgpp_scheduler_type.from_config(
            pipeline.scheduler.config,
            use_dynamic_shifting=False,
            shift_terminal=None,
        )
        pipeline.scheduler.configure_seed(_sampler_seed(request["seed"], "stage-1"))
        stage_1_kwargs = {
            "conditions": stage_1_conditions or None,
            "prompt": prompt,
            "negative_prompt": request["negative_prompt"],
            "height": request["height"] // 2,
            "width": request["width"] // 2,
            "num_frames": request["num_frames"],
            "frame_rate": float(request["frame_rate"]),
            "num_inference_steps": len(stage_1_sigmas),
            "sigmas": list(stage_1_sigmas),
            "guidance_scale": 1.0,
            "audio_guidance_scale": 1.0,
            "generator": stage_1_generator,
            "use_cross_timestep": True,
            "output_type": "latent",
            "return_dict": False,
        }
        video_latent, audio_latent = pipeline(**stage_1_kwargs)
        upscaled_video_latent = runtime.upsample_pipeline(
            latents=video_latent,
            height=request["height"] // 2,
            width=request["width"] // 2,
            num_frames=request["num_frames"],
            spatial_patch_size=pipeline.transformer_spatial_patch_size,
            temporal_patch_size=pipeline.transformer_temporal_patch_size,
            output_type="latent",
            return_dict=False,
        )[0]
        pipeline.set_adapters("mayhem_distilled", _STAGE_2_LORA_WEIGHT)
        stage_2_scheduler_type = (
            runtime.cfgpp_scheduler_type
            if loaded_conditions
            else runtime.lcm_scheduler_type
        )
        pipeline.scheduler = stage_2_scheduler_type.from_config(
            pipeline.scheduler.config,
            use_dynamic_shifting=False,
            shift_terminal=None,
        )
        pipeline.scheduler.configure_seed(_sampler_seed(request["seed"], "stage-2"))
        stage_2_kwargs = {
            "conditions": stage_2_conditions or None,
            "prompt": prompt,
            "height": request["height"],
            "width": request["width"],
            "num_frames": request["num_frames"],
            "frame_rate": float(request["frame_rate"]),
            "num_inference_steps": len(_STAGE_2_SIGMAS),
            "sigmas": list(_STAGE_2_SIGMAS),
            "noise_scale": _STAGE_2_SIGMAS[0],
            "latents": upscaled_video_latent,
            "audio_latents": audio_latent,
            "guidance_scale": 1.0,
            "audio_guidance_scale": 1.0,
            "generator": stage_2_generator,
            "use_cross_timestep": True,
            "output_type": "np",
            "return_dict": False,
        }
        if loaded_conditions:
            stage_2_kwargs["negative_prompt"] = request["negative_prompt"]
        video, audio = pipeline(**stage_2_kwargs)

    frames = video[0]
    waveform = audio[0].detach().float().cpu()
    sample_rate = int(runtime.t2v_pipeline.vocoder.config.output_sampling_rate)
    _validate_generated_tensors(
        frames,
        waveform,
        request["num_frames"],
        sample_rate,
        float(request["frame_rate"]),
    )

    temporary_path: pathlib.Path | None = None
    try:
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{output.stem}-",
            suffix=".part.mp4",
            dir=outputs_root,
        )
        os.close(descriptor)
        temporary_path = pathlib.Path(temporary_name)
        temporary_path.unlink()
        runtime.encode_video(
            frames,
            fps=float(request["frame_rate"]),
            audio=waveform,
            audio_sample_rate=sample_rate,
            output_path=str(temporary_path),
        )
        generated = _real_file(temporary_path, "Sulphur generated temporary MP4")
        if generated.stat().st_size <= 0:
            raise RuntimeError("Sulphur generated an empty MP4")
        os.replace(generated, output)
        temporary_path = None
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)

    return {
        "duration_seconds": request["num_frames"] / float(request["frame_rate"]),
        "frame_count": request["num_frames"],
        "handled_controls": evidence["handled_controls"],
        "stage_1_denoise_intervals": _STAGE_1_STEPS,
        "stage_2_denoise_intervals": len(_STAGE_2_SIGMAS),
    }
