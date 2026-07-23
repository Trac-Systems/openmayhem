"""Offline Apple MLX runtime adapter for the pinned Sulphur-2 artifact."""

from __future__ import annotations

import contextlib
import hashlib
import importlib.metadata
import json
import math
import os
import pathlib
import platform
import re
import shutil
import stat
import subprocess
import tempfile
from dataclasses import dataclass
from typing import Any


MAYHEM_SULPHUR_API_VERSION = 1

_MANIFEST_NAME = "mayhem-sulphur-mlx-runtime.json"
_MANIFEST_SCHEMA = "openmayhem.sulphur.mlx.v1"
_MLX_RUNTIME_REPOSITORY = "https://github.com/dgrauet/ltx-2-mlx"
_MLX_RUNTIME_COMMIT = "e1838a855bfd1640135c424c96cb27a0c0ad150e"
_MLX_RUNTIME_VERSION = "0.14.19"
_MLX_RUNTIME_LOCK_SHA256 = "0dd7510a74a99f13b621b4d4a42ad2b0a07b05f4fe98544313e308c189d3eb54"
_LTX_REFERENCE_RUNTIME_COMMIT = "9377758131b1ffde4b7f766804590a6617bf2ab9"
_SULPHUR_SOURCE_REPOSITORY = "SulphurAI/Sulphur-2-base"
_SULPHUR_SOURCE_COMMIT = "875e886e556b955d21149316fd631cc121db6cc1"
_MODEL_REPOSITORY = "MLXBits/sulphur-2-distill-mlx-q4"
_MODEL_REVISION = "d210a0937cac3464ef80c74806e886beddf19a8e"
_MODEL_PRIMARY_NAME = "transformer-distilled.safetensors"
_MODEL_PRIMARY_SIZE = 10_698_311_363
_MODEL_PRIMARY_SHA256 = "7eb719bb8ee018bac999323fc82084c222d17133a75cdb77060e5097af0ec8b0"
_GEMMA_REPOSITORY = "mlx-community/gemma-3-12b-it-4bit"
_GEMMA_REVISION = "86cc6a8dedbc456dd0e4af01a9d09f396f77e558"
_GEMMA_ARTIFACTS = (
    (
        "model-00001-of-00002.safetensors",
        5_367_455_313,
        "995cbd05b7bfd8f5ab5307b476eb5496b5ec3f5256a9dd26366236ce8816c93f",
    ),
    (
        "model-00002-of-00002.safetensors",
        2_661_219_935,
        "8b7af7eb5ff32109fc65cbcd0af5b8016ac0de46df17f40705f043f899495333",
    ),
)
_PIPELINES_TREE_SHA256 = "6392e4251c283b6161cef3a527867be50f44a7e63ab97dc31da581ecac9943b6"
_PIPELINES_TREE_FILES = 31
_CORE_TREE_SHA256 = "74782d2aae3ac559caeaaece735ba590f2eefe09ba219729ffefbaa0749e2209"
_CORE_TREE_FILES = 66
_DISTILLATION_MODE = "native_distilled_artifact"
_PROMPT_ENHANCER_ASSETS = (
    "prompt_enhancer_uncensored/prompt_enhancer_uncensored-q8_0.gguf",
    "prompt_enhancer_uncensored/mmproj-prompt_enhancer_uncensored.gguf",
)

_STAGE_1_SIGMAS = (1.0, 0.99375, 0.9875, 0.98125, 0.975, 0.909375, 0.725, 0.421875, 0.0)
_STAGE_2_SIGMAS = (0.909375, 0.725, 0.421875, 0.0)
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
_MAX_MANIFEST_BYTES = 8 * 1024 * 1024
_MAX_PROMPT_BYTES = 32 * 1024
_MAX_PROMPT_TOKENS = 1024
_MAX_IMAGE_BYTES = 32 * 1024 * 1024
_MAX_OUTPUT_BYTES = 1024 * 1024 * 1024
_HASH_CHUNK_BYTES = 8 * 1024 * 1024
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

_EXTERNAL_DISTRIBUTIONS = {
    "annotated-doc": "0.0.4",
    "anyio": "4.12.1",
    "certifi": "2026.2.25",
    "click": "8.3.1",
    "colorama": "0.4.6",
    "filelock": "3.25.2",
    "fsspec": "2026.2.0",
    "h11": "0.16.0",
    "hf-xet": "1.4.2",
    "httpcore": "1.0.9",
    "httpx": "0.28.1",
    "huggingface-hub": "1.7.1",
    "idna": "3.11",
    "jinja2": "3.1.6",
    "markdown-it-py": "4.0.0",
    "markupsafe": "3.0.3",
    "mdurl": "0.1.2",
    "mlx": "0.31.1",
    "mlx-arsenal": "0.2.4",
    "mlx-lm": "0.31.1",
    "mlx-metal": "0.31.1",
    "numpy": "2.4.3",
    "packaging": "26.0",
    "pillow": "12.1.1",
    "protobuf": "6.33.6",
    "pygments": "2.19.2",
    "pyyaml": "6.0.3",
    "regex": "2026.2.28",
    "rich": "14.3.3",
    "safetensors": "0.7.0",
    "sentencepiece": "0.2.1",
    "shellingham": "1.5.4",
    "tokenizers": "0.22.2",
    "tqdm": "4.67.3",
    "transformers": "5.3.0",
    "typer": "0.24.1",
    "typing-extensions": "4.15.0",
}
_EMBEDDED_DISTRIBUTIONS = {
    "ltx-core-mlx": "0.14.19",
    "ltx-pipelines-mlx": "0.14.19",
}
_REQUIRED_DISTRIBUTIONS = {
    **_EXTERNAL_DISTRIBUTIONS,
    **_EMBEDDED_DISTRIBUTIONS,
}
_OFFLINE_ENV = {
    "DIFFUSERS_OFFLINE": "1",
    "HF_DATASETS_OFFLINE": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "HF_HUB_OFFLINE": "1",
    "TRANSFORMERS_OFFLINE": "1",
}


@dataclass
class _Runtime:
    model_root: pathlib.Path
    cache_root: pathlib.Path
    artifact_path: pathlib.Path
    sulphur_root: pathlib.Path
    gemma_root: pathlib.Path
    pipeline: Any
    image_conditioning_class: Any
    tokenizer: Any
    manifest_sha256: str


def _exact_object(value: Any, fields: set[str] | frozenset[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != set(fields):
        raise ValueError(f"{label} has unknown or missing fields")
    return value


def _real_directory(value: str | os.PathLike[str], label: str) -> pathlib.Path:
    source = pathlib.Path(value)
    if source.is_symlink():
        raise ValueError(f"{label} must not be a symlink")
    path = source.resolve(strict=True)
    if not path.is_dir():
        raise ValueError(f"{label} must be a real directory")
    return path


def _real_file(value: str | os.PathLike[str], label: str) -> pathlib.Path:
    source = pathlib.Path(value)
    if source.is_symlink():
        raise ValueError(f"{label} must not be a symlink")
    path = source.resolve(strict=True)
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"{label} must be a real regular file")
    return path


def _contained(path: pathlib.Path, root: pathlib.Path, label: str) -> pathlib.Path:
    try:
        path.relative_to(root)
    except ValueError as error:
        raise ValueError(f"{label} escaped its bounded root") from error
    return path


def _relative_path(value: Any, label: str) -> pathlib.PurePosixPath:
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError(f"{label} must be a non-empty POSIX relative path")
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"{label} must be a normalized relative path")
    return path


def _manifest_path(
    root: pathlib.Path,
    value: Any,
    label: str,
    *,
    directory: bool,
) -> pathlib.Path:
    relative = _relative_path(value, label)
    current = root
    for part in relative.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError(f"{label} contains a symlink")
    resolved = current.resolve(strict=True)
    _contained(resolved, root, label)
    if directory and not resolved.is_dir():
        raise ValueError(f"{label} must resolve to a directory")
    if not directory and not stat.S_ISREG(resolved.stat().st_mode):
        raise ValueError(f"{label} must resolve to a regular file")
    return resolved


def _sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(_HASH_CHUNK_BYTES):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_file_inventory(root: pathlib.Path, files: Any) -> dict[str, dict[str, Any]]:
    if not isinstance(files, dict) or not files:
        raise ValueError("Sulphur MLX files must be a non-empty object")
    normalized: dict[str, dict[str, Any]] = {}
    for relative_text, value in files.items():
        relative = _relative_path(relative_text, "Sulphur MLX file").as_posix()
        if relative == _MANIFEST_NAME or relative in normalized:
            raise ValueError("Sulphur MLX inventory contains a duplicate or self-entry")
        entry = _exact_object(value, {"sha256", "size"}, f"Sulphur MLX file {relative}")
        if not isinstance(entry["size"], int) or isinstance(entry["size"], bool) or entry["size"] < 0:
            raise ValueError(f"Sulphur MLX file {relative} has an invalid size")
        if not isinstance(entry["sha256"], str) or not _SHA256_RE.fullmatch(entry["sha256"]):
            raise ValueError(f"Sulphur MLX file {relative} has an invalid SHA-256")
        normalized[relative] = entry

    actual: set[str] = set()
    for item in root.rglob("*"):
        if item.is_symlink():
            raise ValueError("Sulphur MLX model root contains a symlink")
        if item.is_file():
            relative = item.relative_to(root).as_posix()
            if relative != _MANIFEST_NAME:
                actual.add(relative)
    if actual != set(normalized):
        raise ValueError("Sulphur MLX inventory differs from disk")

    for relative, entry in normalized.items():
        path = _manifest_path(root, relative, f"Sulphur MLX file {relative}", directory=False)
        if path.stat().st_size != entry["size"]:
            raise ValueError(f"Sulphur MLX file {relative} has an unexpected size")
        if _sha256_file(path) != entry["sha256"]:
            raise ValueError(f"Sulphur MLX file {relative} has an unexpected SHA-256")
    return normalized


def _load_and_verify_manifest(
    model_root: pathlib.Path,
    artifact_path: pathlib.Path,
) -> dict[str, Any]:
    model_root = model_root.resolve(strict=True)
    artifact_path = artifact_path.resolve(strict=True)
    manifest_path = _real_file(model_root / _MANIFEST_NAME, "Sulphur MLX runtime manifest")
    if manifest_path.stat().st_size > _MAX_MANIFEST_BYTES:
        raise ValueError("Sulphur MLX runtime manifest exceeds its size bound")
    raw_manifest = manifest_path.read_bytes()
    try:
        manifest = json.loads(raw_manifest.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("Sulphur MLX runtime manifest is not valid UTF-8 JSON") from error
    manifest = _exact_object(
        manifest,
        {
            "schema",
            "runtime",
            "source",
            "model",
            "gemma",
            "roles",
            "prompt_enhancer",
            "files",
        },
        "Sulphur MLX runtime manifest",
    )
    if manifest["schema"] != _MANIFEST_SCHEMA:
        raise ValueError("Sulphur MLX runtime manifest has an unexpected schema")

    runtime = _exact_object(
        manifest["runtime"],
        {"repository", "revision", "package", "version", "lockfile_sha256"},
        "Sulphur MLX runtime pin",
    )
    if runtime != {
        "repository": _MLX_RUNTIME_REPOSITORY,
        "revision": _MLX_RUNTIME_COMMIT,
        "package": "ltx-2-mlx",
        "version": _MLX_RUNTIME_VERSION,
        "lockfile_sha256": _MLX_RUNTIME_LOCK_SHA256,
    }:
        raise ValueError("Sulphur MLX runtime pin is not canonical")

    source = _exact_object(
        manifest["source"],
        {"repository", "revision"},
        "Sulphur source pin",
    )
    if source != {
        "repository": _SULPHUR_SOURCE_REPOSITORY,
        "revision": _SULPHUR_SOURCE_COMMIT,
    }:
        raise ValueError("Sulphur source pin is not canonical")

    model = _exact_object(
        manifest["model"],
        {
            "repository",
            "revision",
            "primary_artifact",
            "primary_artifact_sha256",
            "primary_artifact_size",
            "quantization_bits",
            "quantization_group_size",
        },
        "Sulphur MLX model pin",
    )
    if (
        model["repository"] != _MODEL_REPOSITORY
        or model["revision"] != _MODEL_REVISION
        or model["primary_artifact_sha256"] != _MODEL_PRIMARY_SHA256
        or model["primary_artifact_size"] != _MODEL_PRIMARY_SIZE
        or model["quantization_bits"] != 4
        or model["quantization_group_size"] != 64
    ):
        raise ValueError("Sulphur MLX model pin is not canonical")

    gemma = _exact_object(
        manifest["gemma"],
        {"repository", "revision", "artifacts"},
        "Sulphur MLX Gemma pin",
    )
    if gemma["repository"] != _GEMMA_REPOSITORY or gemma["revision"] != _GEMMA_REVISION:
        raise ValueError("Sulphur MLX Gemma pin is not canonical")
    artifacts = gemma["artifacts"]
    if not isinstance(artifacts, list) or len(artifacts) != len(_GEMMA_ARTIFACTS):
        raise ValueError("Sulphur MLX Gemma artifact pins are incomplete")

    roles = _exact_object(
        manifest["roles"],
        {"sulphur_root", "gemma_root"},
        "Sulphur MLX roles",
    )
    sulphur_root = _manifest_path(
        model_root, roles["sulphur_root"], "Sulphur MLX artifact root", directory=True
    )
    gemma_root = _manifest_path(
        model_root, roles["gemma_root"], "Sulphur MLX Gemma root", directory=True
    )
    if sulphur_root == gemma_root or artifact_path != sulphur_root:
        raise ValueError("Sulphur MLX artifact path does not match its canonical role")

    primary_relative = _relative_path(
        model["primary_artifact"], "Sulphur MLX primary artifact"
    ).as_posix()
    primary = _manifest_path(
        model_root, primary_relative, "Sulphur MLX primary artifact", directory=False
    )
    _contained(primary, sulphur_root, "Sulphur MLX primary artifact")
    if primary.name != _MODEL_PRIMARY_NAME:
        raise ValueError("Sulphur MLX primary artifact has an unexpected name")

    expected_gemma: list[dict[str, Any]] = []
    for name, size, sha256 in _GEMMA_ARTIFACTS:
        path = gemma_root / name
        relative = path.relative_to(model_root).as_posix()
        expected_gemma.append({"path": relative, "size": size, "sha256": sha256})
    if artifacts != expected_gemma:
        raise ValueError("Sulphur MLX Gemma artifact pins are not canonical")
    files = _verify_file_inventory(model_root, manifest["files"])
    if files.get(primary_relative) != {
        "size": _MODEL_PRIMARY_SIZE,
        "sha256": _MODEL_PRIMARY_SHA256,
    }:
        raise ValueError("Sulphur MLX primary artifact inventory does not match its pin")
    for artifact in expected_gemma:
        if files.get(artifact["path"]) != {
            "size": artifact["size"],
            "sha256": artifact["sha256"],
        }:
            raise ValueError("Sulphur MLX Gemma inventory does not match its pin")
    prompt_enhancer = manifest["prompt_enhancer"]
    if prompt_enhancer is not None:
        prompt_enhancer = _exact_object(
            prompt_enhancer,
            {"asset_paths", "system_prompt"},
            "Sulphur MLX prompt enhancer",
        )
        if prompt_enhancer["asset_paths"] != list(_PROMPT_ENHANCER_ASSETS):
            raise ValueError("Sulphur MLX prompt enhancer assets are not canonical")
        if prompt_enhancer["system_prompt"] != "":
            raise ValueError("Sulphur MLX prompt enhancer source declares no system prompt")
        for relative in _PROMPT_ENHANCER_ASSETS:
            if relative not in files:
                raise ValueError("Sulphur MLX prompt enhancer inventory is incomplete")

    return {
        "sulphur_root": sulphur_root,
        "gemma_root": gemma_root,
        "manifest_sha256": hashlib.sha256(raw_manifest).hexdigest(),
    }


def _force_offline() -> None:
    for name, value in _OFFLINE_ENV.items():
        os.environ[name] = value


def _python_tree_digest(root: pathlib.Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    files = sorted(root.rglob("*.py"))
    for path in files:
        if path.is_symlink() or not stat.S_ISREG(path.stat().st_mode):
            raise RuntimeError("Pinned Sulphur MLX package tree contains an invalid file")
        relative = path.relative_to(root).as_posix().encode("utf-8")
        content = path.read_bytes()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        digest.update(len(content).to_bytes(8, "big"))
        digest.update(content)
    return len(files), digest.hexdigest()


def _verify_runtime_packages() -> None:
    for distribution, expected in _REQUIRED_DISTRIBUTIONS.items():
        try:
            actual = importlib.metadata.version(distribution)
        except importlib.metadata.PackageNotFoundError as error:
            raise RuntimeError(f"Pinned Sulphur MLX distribution is missing: {distribution}") from error
        if actual != expected:
            raise RuntimeError(
                f"Pinned Sulphur MLX distribution mismatch: {distribution}=={actual}, expected {expected}"
            )

    tree_pins = (
        (
            "ltx-pipelines-mlx",
            "ltx_pipelines_mlx",
            _PIPELINES_TREE_FILES,
            _PIPELINES_TREE_SHA256,
        ),
        ("ltx-core-mlx", "ltx_core_mlx", _CORE_TREE_FILES, _CORE_TREE_SHA256),
    )
    for distribution_name, module_name, expected_count, expected_digest in tree_pins:
        distribution = importlib.metadata.distribution(distribution_name)
        root = _real_directory(
            distribution.locate_file(module_name),
            f"Pinned {distribution_name} package root",
        )
        count, digest = _python_tree_digest(root)
        if count != expected_count or digest != expected_digest:
            raise RuntimeError(f"Pinned {distribution_name} source tree differs from the audited commit")


def _require_apple_silicon() -> None:
    if platform.system() != "Darwin" or platform.machine().lower() not in ("arm64", "aarch64"):
        raise RuntimeError("Sulphur MLX requires Apple Silicon on macOS")


def _import_pipeline() -> tuple[Any, Any, tuple[float, ...], tuple[float, ...]]:
    from ltx_pipelines_mlx.distilled import DistilledPipeline
    from ltx_pipelines_mlx.scheduler import DISTILLED_SIGMAS, STAGE_2_SIGMAS
    from ltx_pipelines_mlx.utils.args import ImageConditioningInput

    return (
        DistilledPipeline,
        ImageConditioningInput,
        tuple(DISTILLED_SIGMAS),
        tuple(STAGE_2_SIGMAS),
    )


def _load_tokenizer(gemma_root: pathlib.Path) -> Any:
    try:
        from transformers import AutoTokenizer

        return AutoTokenizer.from_pretrained(
            str(gemma_root),
            local_files_only=True,
            trust_remote_code=False,
        )
    except Exception as error:
        raise RuntimeError("Pinned local Sulphur MLX Gemma tokenizer could not be loaded") from error


def load(
    *,
    model_root: str,
    artifact_path: str,
    backend: str,
    cache_root: str,
) -> _Runtime:
    if backend != "mlx":
        raise ValueError("Sulphur MLX runtime accepts only backend mlx")
    root = _real_directory(model_root, "Sulphur MLX model root")
    artifact = _real_directory(artifact_path, "Sulphur MLX artifact path")
    cache = _real_directory(cache_root, "Sulphur MLX cache root")
    if root == cache or root in cache.parents:
        raise ValueError("Sulphur MLX cache root must not mutate the signed model root")

    verified = _load_and_verify_manifest(root, artifact)
    _force_offline()
    _require_apple_silicon()
    _verify_runtime_packages()
    (
        pipeline_class,
        image_conditioning_class,
        stage_1_sigmas,
        stage_2_sigmas,
    ) = _import_pipeline()
    if stage_1_sigmas != _STAGE_1_SIGMAS or stage_2_sigmas != _STAGE_2_SIGMAS:
        raise RuntimeError("Pinned Sulphur MLX denoise schedules differ from the audited 8+3 schedule")
    pipeline = pipeline_class(
        model_dir=str(verified["sulphur_root"]),
        gemma_model_id=str(verified["gemma_root"]),
        low_memory=True,
        low_ram_streaming=True,
    )
    tokenizer = _load_tokenizer(verified["gemma_root"])
    return _Runtime(
        model_root=root,
        cache_root=cache,
        artifact_path=artifact,
        sulphur_root=verified["sulphur_root"],
        gemma_root=verified["gemma_root"],
        pipeline=pipeline,
        image_conditioning_class=image_conditioning_class,
        tokenizer=tokenizer,
        manifest_sha256=verified["manifest_sha256"],
    )


def describe(runtime: _Runtime) -> dict[str, Any]:
    if not isinstance(runtime, _Runtime):
        raise TypeError("Sulphur MLX runtime is invalid")
    return {
        "api_version": MAYHEM_SULPHUR_API_VERSION,
        "runtime_name": "dgrauet/ltx-2-mlx",
        "runtime_version": (
            f"{_MLX_RUNTIME_VERSION}@{_MLX_RUNTIME_COMMIT};"
            f"manifest={runtime.manifest_sha256}"
        ),
        "backend": "mlx",
        "distilled": True,
        "joint_audio_video": True,
        "prompt_enhancer": False,
        "ltx_runtime_commit": _LTX_REFERENCE_RUNTIME_COMMIT,
        "sulphur_source_commit": _SULPHUR_SOURCE_COMMIT,
        "distillation_mode": _DISTILLATION_MODE,
        "stage_1_denoise_intervals": 8,
        "stage_2_denoise_intervals": 3,
    }


def _request(value: Any) -> dict[str, Any]:
    return _exact_object(value, _REQUEST_FIELDS, "Sulphur MLX generation request")


def _require_prompt_within_token_limit(tokenizer: Any, value: str) -> None:
    if tokenizer is None or not callable(getattr(tokenizer, "encode", None)):
        raise RuntimeError("Sulphur MLX tokenizer is unavailable for no-truncation validation")
    try:
        token_ids = tokenizer.encode(value.strip(), add_special_tokens=True)
    except Exception as error:
        raise RuntimeError("Sulphur MLX prompt tokenization failed") from error
    if not isinstance(token_ids, (list, tuple)) or any(
        not isinstance(token_id, int) or isinstance(token_id, bool)
        for token_id in token_ids
    ):
        raise RuntimeError("Sulphur MLX tokenizer returned an invalid token sequence")
    if len(token_ids) > _MAX_PROMPT_TOKENS:
        raise ValueError(
            f"Sulphur MLX prompt contains {len(token_ids)} tokens; "
            f"the exact model limit is {_MAX_PROMPT_TOKENS} and truncation is forbidden"
        )


def _materialized_images(
    runtime: _Runtime,
    images: Any,
    num_frames: int,
) -> list[Any]:
    if not isinstance(images, list):
        raise ValueError("Sulphur MLX images must be an ordered list")
    if not images:
        return []
    inputs_root = _real_directory(runtime.cache_root / "inputs", "Sulphur MLX input root")
    materialized = []
    for index, raw_image in enumerate(images):
        label = f"Sulphur MLX materialized image {index}"
        image = _exact_object(raw_image, _IMAGE_FIELDS, label)
        if image["content_type"] not in ("image/png", "image/jpeg"):
            raise ValueError(f"{label} has an unsupported content type")

        frame_index = image["frame_index"]
        if (
            not isinstance(frame_index, int)
            or isinstance(frame_index, bool)
            or not 0 <= frame_index < num_frames
        ):
            raise ValueError(f"{label} frame_index must identify an output frame")

        strength = image["strength"]
        if (
            not isinstance(strength, (int, float))
            or isinstance(strength, bool)
            or not math.isfinite(float(strength))
            or not 0.0 <= float(strength) <= 1.0
        ):
            raise ValueError(f"{label} strength must be finite and between 0 and 1")

        crf = image["crf"]
        if (
            not isinstance(crf, int)
            or isinstance(crf, bool)
            or not 0 <= crf <= 51
        ):
            raise ValueError(f"{label} crf must be an integer between 0 and 51")

        path = _real_file(image["path"], f"{label} input file")
        _contained(path, inputs_root, f"{label} input file")
        expected_suffix = ".png" if image["content_type"] == "image/png" else ".jpg"
        if path.suffix.lower() != expected_suffix:
            raise ValueError(f"{label} suffix does not match its content type")
        if not 0 < path.stat().st_size <= _MAX_IMAGE_BYTES:
            raise ValueError(f"{label} is outside its size bound")
        with path.open("rb") as handle:
            signature = handle.read(8)
        valid_signature = (
            image["content_type"] == "image/png" and signature == b"\x89PNG\r\n\x1a\n"
        ) or (
            image["content_type"] == "image/jpeg"
            and signature.startswith(b"\xff\xd8\xff")
        )
        if not valid_signature:
            raise ValueError(f"{label} bytes do not match its content type")

        materialized.append(
            runtime.image_conditioning_class(
                path=str(path),
                frame_idx=frame_index,
                strength=float(strength),
                crf=crf,
            )
        )
    return materialized


def validate_video(runtime: _Runtime, request: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(runtime, _Runtime):
        raise TypeError("Sulphur MLX runtime is invalid")
    request = _request(request)
    prompt = request["prompt"]
    if (
        not isinstance(prompt, str)
        or not prompt.strip()
        or not 0 < len(prompt.encode("utf-8")) <= _MAX_PROMPT_BYTES
    ):
        raise ValueError("Sulphur MLX prompt must contain 1 to 32768 UTF-8 bytes")
    _require_prompt_within_token_limit(runtime.tokenizer, prompt)
    negative_prompt = request["negative_prompt"]
    if not isinstance(negative_prompt, str):
        raise ValueError("Sulphur MLX negative_prompt must be text")
    if negative_prompt:
        raise ValueError(
            "the pinned native-distilled Sulphur MLX artifact does not expose negative prompting"
        )
    for field in ("width", "height", "num_frames", "seed"):
        if not isinstance(request[field], int) or isinstance(request[field], bool):
            raise ValueError(f"Sulphur MLX {field} must be an integer")
    if not 256 <= request["width"] <= 2048 or request["width"] % 64:
        raise ValueError("Sulphur MLX width must be a multiple of 64 between 256 and 2048")
    if not 256 <= request["height"] <= 2048 or request["height"] % 64:
        raise ValueError("Sulphur MLX height must be a multiple of 64 between 256 and 2048")
    frames = request["num_frames"]
    if not 1 <= frames <= 513 or (frames - 1) % 8:
        raise ValueError("Sulphur MLX num_frames must be an 8k+1 value between 1 and 513")
    frame_rate = request["frame_rate"]
    if (
        not isinstance(frame_rate, (int, float))
        or isinstance(frame_rate, bool)
        or not math.isfinite(float(frame_rate))
        or not 1.0 <= float(frame_rate) <= 50.0
    ):
        raise ValueError("Sulphur MLX frame_rate must be finite and between 1 and 50")
    if not 0 <= request["seed"] <= 0xFFFFFFFF:
        raise ValueError("Sulphur MLX seed must fit in an unsigned 32-bit integer")
    if not isinstance(request["enhance_prompt"], bool):
        raise ValueError("Sulphur MLX enhance_prompt must be boolean")
    if request["enhance_prompt"]:
        raise ValueError("Sulphur MLX prompt enhancement has no signed canonical assets")
    _materialized_images(runtime, request["images"], frames)
    return {"handled_controls": sorted(_REQUEST_FIELDS), "valid": True}


def _parse_rate(value: Any) -> float:
    if not isinstance(value, str) or "/" not in value:
        raise RuntimeError("Sulphur MLX ffprobe returned an invalid frame rate")
    numerator, denominator = value.split("/", 1)
    rate = float(numerator) / float(denominator)
    if not math.isfinite(rate) or rate <= 0:
        raise RuntimeError("Sulphur MLX ffprobe returned a non-positive frame rate")
    return rate


def _probe_joint_av(
    output: pathlib.Path,
    expected_frames: int,
    expected_fps: float,
) -> dict[str, Any]:
    ffprobe_name = shutil.which("ffprobe")
    if ffprobe_name is None:
        raise RuntimeError("Sulphur MLX ffprobe is unavailable")
    ffprobe = pathlib.Path(ffprobe_name).resolve(strict=True)
    completed = subprocess.run(
        [
            str(ffprobe),
            "-v",
            "error",
            "-count_frames",
            "-show_entries",
            "stream=codec_type,nb_read_frames,duration,avg_frame_rate,sample_rate,channels",
            "-of",
            "json",
            str(output),
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=30,
    )
    if completed.returncode != 0 or len(completed.stdout) > 1024 * 1024:
        raise RuntimeError("Sulphur MLX output is not a decodable joint A/V MP4")
    try:
        payload = json.loads(completed.stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("Sulphur MLX ffprobe output is invalid") from error
    streams = payload.get("streams") if isinstance(payload, dict) else None
    if not isinstance(streams, list):
        raise RuntimeError("Sulphur MLX output has no stream inventory")
    videos = [stream for stream in streams if stream.get("codec_type") == "video"]
    audios = [stream for stream in streams if stream.get("codec_type") == "audio"]
    if len(videos) != 1 or len(audios) != 1:
        raise RuntimeError("Sulphur MLX output must contain exactly one video and one audio stream")
    video = videos[0]
    audio = audios[0]
    try:
        frame_count = int(video["nb_read_frames"])
        video_duration = float(video["duration"])
        audio_duration = float(audio["duration"])
        frame_rate = _parse_rate(video["avg_frame_rate"])
        sample_rate = int(audio["sample_rate"])
        channels = int(audio["channels"])
    except (KeyError, TypeError, ValueError, ZeroDivisionError) as error:
        raise RuntimeError("Sulphur MLX output stream evidence is incomplete") from error
    tolerance = 1.0 / expected_fps + 1e-6
    if (
        frame_count != expected_frames
        or abs(frame_rate - expected_fps) > 1e-3
        or not math.isfinite(video_duration)
        or not math.isfinite(audio_duration)
        or video_duration <= 0
        or audio_duration <= 0
        or abs(video_duration - audio_duration) > tolerance
        or sample_rate != 48_000
        or channels != 2
    ):
        raise RuntimeError("Sulphur MLX output does not satisfy exact joint A/V evidence")
    return {
        "audio_duration_seconds": audio_duration,
        "audio_sample_rate": sample_rate,
        "audio_channels": channels,
        "frame_count": frame_count,
        "frame_rate": frame_rate,
        "video_duration_seconds": video_duration,
    }


def _remove_new_entries(before: set[pathlib.Path], root: pathlib.Path) -> None:
    for entry in set(root.iterdir()) - before:
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink(missing_ok=True)


@contextlib.contextmanager
def _bounded_temp_directory(cache_root: pathlib.Path):
    scratch_root = cache_root / "tmp"
    scratch_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(scratch_root, 0o700)
    previous = tempfile.tempdir
    with tempfile.TemporaryDirectory(prefix="sulphur-mlx-", dir=scratch_root) as temporary:
        tempfile.tempdir = temporary
        try:
            yield
        finally:
            tempfile.tempdir = previous


def generate_video(
    runtime: _Runtime,
    request: dict[str, Any],
    output_path: str,
) -> dict[str, Any]:
    evidence = validate_video(runtime, request)
    output = pathlib.Path(output_path)
    if not output.is_absolute() or output.suffix.lower() != ".mp4":
        raise ValueError("Sulphur MLX output_path must be an absolute MP4 path")
    outputs_root = runtime.cache_root / "outputs"
    outputs_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    outputs_root = _real_directory(outputs_root, "Sulphur MLX output root")
    resolved_output = output.resolve(strict=False)
    if resolved_output.parent != outputs_root or output.exists() or output.is_symlink():
        raise ValueError("Sulphur MLX output_path escaped its fresh bounded output directory")

    images = _materialized_images(
        runtime,
        request["images"],
        request["num_frames"],
    )
    kwargs = {
        "prompt": request["prompt"],
        "output_path": str(resolved_output),
        "height": request["height"],
        "width": request["width"],
        "num_frames": request["num_frames"],
        "frame_rate": float(request["frame_rate"]),
        "seed": request["seed"],
        "stage1_steps": 8,
        "stage2_steps": 3,
    }
    if images:
        kwargs["images"] = images

    before = set(outputs_root.iterdir())
    try:
        with _bounded_temp_directory(runtime.cache_root):
            generated = runtime.pipeline.generate_and_save(**kwargs)
        generated_path = pathlib.Path(generated).resolve(strict=True)
        if generated_path != resolved_output:
            raise RuntimeError("Sulphur MLX pipeline wrote a different output path")
        new_entries = set(outputs_root.iterdir()) - before
        if new_entries != {resolved_output}:
            raise RuntimeError("Sulphur MLX pipeline wrote an unexpected output sidecar")
        result_file = _real_file(resolved_output, "Sulphur MLX generated MP4")
        if not 0 < result_file.stat().st_size <= _MAX_OUTPUT_BYTES:
            raise RuntimeError("Sulphur MLX generated MP4 is outside its size bound")
        media = _probe_joint_av(
            result_file,
            request["num_frames"],
            float(request["frame_rate"]),
        )
    except BaseException:
        _remove_new_entries(before, outputs_root)
        raise

    return {
        "duration_seconds": media["video_duration_seconds"],
        "frame_count": media["frame_count"],
        "handled_controls": evidence["handled_controls"],
        "stage_1_denoise_intervals": 8,
        "stage_2_denoise_intervals": 3,
    }
