import base64
import contextlib
import json
import math
import os
import pathlib
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile


_PROTOCOL_STDOUT = sys.stdout
_MAX_INPUT_AUDIO_BYTES = 64 * 1024 * 1024
_MAX_OUTPUT_AUDIO_BYTES = 512 * 1024 * 1024
_MAX_AUDIO_DURATION_SECONDS = 600.0
_MAX_CANONICAL_AUDIO_RATE = 48_000
_MAX_HELPER_CAPTION_CHARS = 4096
_GIB = 1024**3
_CPU_OFFLOAD_THRESHOLD_BYTES = 20 * _GIB
_DIT_OFFLOAD_THRESHOLD_BYTES = 12 * _GIB
_ACE_STEP_SOURCE_COMMIT = "dce621408bee8c31b4fcf4811682eb9359e1bc94"
_MEMORY_CALIBRATION = "acestep/gpu_config.py:v0.1.8-tier-vram-calibration"
_CONTENT_TYPES = {
    "flac": "audio/flac",
    "mp3": "audio/mpeg",
    "opus": "audio/ogg",
    "aac": "audio/aac",
    "wav": "audio/wav",
    "wav32": "audio/wav",
}
_FFMPEG_AUDIO_FORMATS = {"mp3", "opus", "aac"}
_INPUT_SUFFIXES = {
    "audio/aac": ".aac",
    "audio/flac": ".flac",
    "audio/m4a": ".m4a",
    "audio/mp4": ".m4a",
    "audio/mpeg": ".mp3",
    "audio/mp3": ".mp3",
    "audio/ogg": ".ogg",
    "audio/opus": ".opus",
    "audio/wav": ".wav",
    "audio/x-wav": ".wav",
}

_dit_handler = None
_llm_handler = None
_model_root = None
_worker_cache = None


def _reply(message_id, result=None, error=None):
    response = {"id": message_id, "ok": error is None}
    if error is None:
        response["result"] = result
    else:
        response["error"] = str(error)
    _PROTOCOL_STDOUT.write(json.dumps(response, separators=(",", ":")) + "\n")
    _PROTOCOL_STDOUT.flush()


def _absolute_local_path(path):
    candidate = pathlib.Path(path)
    if not candidate.is_absolute():
        raise ValueError("local path must be absolute")
    return pathlib.Path(os.path.abspath(candidate))


def _is_reparse_point(path):
    attributes = getattr(os.lstat(path), "st_file_attributes", 0)
    return bool(attributes & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0))


def _require_local_dir(path, label):
    resolved = _absolute_local_path(path)
    if not resolved.is_dir() or _is_reparse_point(resolved):
        raise ValueError(f"{label} is not a directory")
    return resolved


def _require_local_file_under(path, root, label):
    resolved = _absolute_local_path(path)
    if not resolved.is_relative_to(root):
        raise RuntimeError(f"{label} is outside the request directory")
    relative = resolved.relative_to(root)
    if not relative.parts or any(":" in part for part in relative.parts):
        raise RuntimeError(f"{label} is not a regular local path")
    current = root
    for part in relative.parts:
        current /= part
        if _is_reparse_point(current):
            raise RuntimeError(f"{label} traverses a reparse point")
    if not resolved.is_file():
        raise RuntimeError(f"{label} is not a regular file")
    return resolved


@contextlib.contextmanager
def _temporary_request_directory(root):
    if os.name != "nt":
        with tempfile.TemporaryDirectory(prefix="request-", dir=root) as temporary:
            yield pathlib.Path(temporary)
        return

    # CPython's Windows 0o700 mkdir uses a protected DACL that excludes the
    # AppContainer SID. The cache root is already scoped to this launch, so let
    # each request directory inherit that exact ACL instead.
    for _ in range(100):
        temporary = root / f"request-{secrets.token_hex(8)}"
        try:
            temporary.mkdir()
            break
        except FileExistsError:
            continue
    else:
        raise FileExistsError("could not allocate a unique request directory")
    try:
        yield temporary
    finally:
        shutil.rmtree(temporary)


def _contains_safetensors(path):
    return any(candidate.is_file() for candidate in path.rglob("*.safetensors"))


def _forbid_download(*_args, **_kwargs):
    raise RuntimeError("ACE-Step runtime downloads are disabled")


def _validate_audio_byte_length(length, maximum, label):
    if not isinstance(length, int) or length <= 0 or length > maximum:
        raise ValueError(f"{label} audio byte length is out of bounds")


def _ffmpeg_program():
    return shutil.which("ffmpeg")


def _can_transcode_with_ffmpeg(audio_format):
    return audio_format in _FFMPEG_AUDIO_FORMATS and _ffmpeg_program() is not None


def _samples_for_soundfile(tensor):
    with contextlib.redirect_stdout(sys.stderr):
        import numpy

        samples = tensor.detach().cpu().float().numpy()
        if (
            samples.ndim != 2
            or samples.shape[0] <= 0
            or samples.shape[0] > 8
            or samples.shape[1] <= 0
            or not numpy.isfinite(samples).all()
        ):
            raise RuntimeError("ACE-Step returned an invalid generated audio tensor")
        return numpy.ascontiguousarray(samples.T, dtype=numpy.float32)


def _validate_generated_audio(path, data, audio_format, expected_duration):
    signature_matches = {
        "flac": lambda: data.startswith(b"fLaC"),
        "mp3": lambda: data.startswith(b"ID3")
        or (len(data) >= 2 and data[0] == 0xFF and data[1] & 0xE0 == 0xE0),
        "opus": lambda: data.startswith(b"OggS") and b"OpusHead" in data[:4096],
        "aac": lambda: len(data) >= 2
        and data[0] == 0xFF
        and data[1] & 0xF6 == 0xF0,
        "wav": lambda: data.startswith(b"RIFF") and data[8:12] == b"WAVE",
        "wav32": lambda: data.startswith(b"RIFF") and data[8:12] == b"WAVE",
    }[audio_format]()
    if not signature_matches:
        raise RuntimeError(
            f"ACE-Step returned bytes that do not match requested {audio_format}"
        )
    try:
        encoded_duration, _ = _decoded_audio_metrics(path, "generated")
    except Exception as exc:
        raise RuntimeError(
            f"ACE-Step returned undecodable {audio_format} audio"
        ) from exc
    if (
        not math.isfinite(encoded_duration)
        or encoded_duration <= 0.0
        or encoded_duration > _MAX_AUDIO_DURATION_SECONDS
        or abs(encoded_duration - expected_duration) > 0.25
    ):
        raise RuntimeError(
            "ACE-Step encoded audio duration does not match generated tensor"
        )
    if audio_format == "wav32":
        with contextlib.redirect_stdout(sys.stderr):
            import soundfile

            try:
                subtype = soundfile.info(str(path)).subtype
            except Exception as exc:
                raise RuntimeError("ACE-Step returned an unreadable wav32 file") from exc
        if subtype != "FLOAT":
            raise RuntimeError(
                "ACE-Step wav32 export fell back instead of producing 32-bit float WAV"
            )


def _canonicalize_generated_wave(path, tensor, sample_rate, audio_format):
    if audio_format not in ("wav", "wav32"):
        return
    with contextlib.redirect_stdout(sys.stderr):
        import soundfile

        samples = _samples_for_soundfile(tensor)
        temporary = path.with_name(f".{path.name}.mayhem-canonical.wav")
        try:
            soundfile.write(
                str(temporary),
                samples,
                sample_rate,
                format="WAV",
                subtype="PCM_16" if audio_format == "wav" else "FLOAT",
            )
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def _transcode_generated_audio(path, tensor, sample_rate, audio_format, config):
    if audio_format not in _FFMPEG_AUDIO_FORMATS:
        return path
    ffmpeg = _ffmpeg_program()
    if ffmpeg is None:
        return path
    with contextlib.redirect_stdout(sys.stderr):
        import soundfile

        samples = _samples_for_soundfile(tensor)
        source = path.with_name(f".{path.stem}.mayhem-source.wav")
        target = path.with_suffix(f".{audio_format}")
        temporary = target.with_name(f".{target.name}.mayhem-transcode")
        try:
            soundfile.write(str(source), samples, sample_rate, format="WAV", subtype="PCM_16")
            command = [
                ffmpeg,
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                str(source),
            ]
            if audio_format == "mp3":
                bitrate = str(config.get("mp3_bitrate") or "128k").strip().lower()
                if bitrate not in {"128k", "192k", "256k", "320k"}:
                    bitrate = "128k"
                target_sample_rate = int(config.get("mp3_sample_rate") or 48_000)
                if target_sample_rate not in (44_100, 48_000):
                    target_sample_rate = 48_000
                command.extend([
                    "-codec:a",
                    "libmp3lame",
                    "-ar",
                    str(target_sample_rate),
                    "-b:a",
                    bitrate,
                    "-f",
                    "mp3",
                ])
            elif audio_format == "opus":
                command.extend(["-codec:a", "libopus", "-f", "opus"])
            elif audio_format == "aac":
                command.extend(["-codec:a", "aac", "-f", "adts"])
            duration = float(tensor.shape[-1]) / float(sample_rate or 1)
            timeout = max(120, min(900, int(duration * 3.0) + 30))
            command.append(str(temporary))
            subprocess.run(command, check=True, capture_output=True, timeout=timeout)
            temporary.replace(target)
            return target
        except FileNotFoundError as exc:
            raise RuntimeError("ffmpeg executable not found. Install ffmpeg or add it to PATH.") from exc
        except subprocess.TimeoutExpired as exc:
            raise RuntimeError(f"ffmpeg {audio_format} export timed out") from exc
        except subprocess.CalledProcessError as exc:
            stderr = exc.stderr.decode("utf-8", errors="ignore") if exc.stderr else str(exc)
            raise RuntimeError(f"ffmpeg {audio_format} export failed: {stderr}") from exc
        finally:
            source.unlink(missing_ok=True)
            temporary.unlink(missing_ok=True)


def _accelerator_memory(torch, device_kind):
    accelerator = getattr(torch, device_kind)
    memory = getattr(accelerator, "mem_get_info", None)
    if memory is None:
        raise RuntimeError(
            f"ACE-Step cannot measure load-time free memory for {device_kind}"
        )
    free_bytes, total_bytes = memory()
    free_bytes = int(free_bytes)
    total_bytes = int(total_bytes)
    if free_bytes <= 0 or total_bytes <= 0 or free_bytes > total_bytes:
        raise RuntimeError(
            f"ACE-Step returned invalid {device_kind} free-memory information"
        )
    return free_bytes, total_bytes


def _select_execution_config(torch):
    if torch.cuda.is_available():
        device_kind = "cuda"
        free_bytes, total_bytes = _accelerator_memory(torch, device_kind)
    elif (
        hasattr(torch, "xpu")
        and torch.xpu.is_available()
        and hasattr(torch.xpu, "mem_get_info")
    ):
        device_kind = "xpu"
        free_bytes, total_bytes = _accelerator_memory(torch, device_kind)
    elif (
        hasattr(torch.backends, "mps")
        and torch.backends.mps.is_available()
    ):
        # The pinned runtime explicitly disables torchao and CPU offload on MPS.
        return {
            "device_kind": "mps",
            "free_memory_bytes": None,
            "total_memory_bytes": None,
            "offload_to_cpu": False,
            "offload_dit_to_cpu": False,
            "quantization": None,
            "selection_basis": "pinned-v0.1.8-mps-policy",
            "memory_calibration": _MEMORY_CALIBRATION,
            "source_commit": _ACE_STEP_SOURCE_COMMIT,
        }
    else:
        return {
            "device_kind": "cpu",
            "free_memory_bytes": None,
            "total_memory_bytes": None,
            "offload_to_cpu": False,
            "offload_dit_to_cpu": False,
            "quantization": None,
            "selection_basis": "pinned-v0.1.8-cpu-policy",
            "memory_calibration": _MEMORY_CALIBRATION,
            "source_commit": _ACE_STEP_SOURCE_COMMIT,
        }

    offload_to_cpu = free_bytes < _CPU_OFFLOAD_THRESHOLD_BYTES
    offload_dit_to_cpu = free_bytes < _DIT_OFFLOAD_THRESHOLD_BYTES
    return {
        "device_kind": device_kind,
        "free_memory_bytes": free_bytes,
        "total_memory_bytes": total_bytes,
        "offload_to_cpu": offload_to_cpu,
        "offload_dit_to_cpu": offload_dit_to_cpu,
        "quantization": "int8_weight_only" if offload_to_cpu else None,
        "selection_basis": "load-time-free-accelerator-memory",
        "memory_calibration": _MEMORY_CALIBRATION,
        "source_commit": _ACE_STEP_SOURCE_COMMIT,
    }


def _verify_effective_execution_config(handler, selected):
    effective = getattr(handler, "last_init_params", None)
    if not isinstance(effective, dict):
        raise RuntimeError("ACE-Step loader did not report effective load settings")
    effective_device = str(effective.get("device", "")).split(":", 1)[0]
    expected = {
        "device_kind": selected["device_kind"],
        "offload_to_cpu": selected["offload_to_cpu"],
        "offload_dit_to_cpu": selected["offload_dit_to_cpu"],
        "quantization": selected["quantization"],
    }
    actual = {
        "device_kind": effective_device,
        "offload_to_cpu": effective.get("offload_to_cpu"),
        "offload_dit_to_cpu": effective.get("offload_dit_to_cpu"),
        "quantization": effective.get("quantization"),
    }
    if actual != expected:
        raise RuntimeError(
            "ACE-Step loader changed the selected execution configuration"
        )


def _verify_effective_lm_execution_config(handler, selected):
    effective = getattr(handler, "_last_initialize_config", None)
    if not isinstance(effective, dict):
        raise RuntimeError("ACE-Step LM loader did not report effective load settings")
    effective_device = str(effective.get("device", "")).split(":", 1)[0]
    if (
        effective.get("backend") != "pt"
        or effective_device != selected["device_kind"]
        or effective.get("offload_to_cpu") != selected["offload_to_cpu"]
    ):
        raise RuntimeError(
            "ACE-Step LM loader changed the selected execution configuration"
        )


def _load(payload):
    global _dit_handler, _llm_handler, _model_root, _worker_cache

    if set(payload) != {"model_root", "source_root", "worker_cache"}:
        raise ValueError("load payload has unknown or missing fields")
    source_root = _require_local_dir(payload["source_root"], "ACE-Step source root")
    model_root = _require_local_dir(payload["model_root"], "ACE-Step model root")
    worker_cache = _require_local_dir(payload["worker_cache"], "ACE-Step worker cache")
    if not (source_root / "pyproject.toml").is_file() or not (source_root / "uv.lock").is_file():
        raise ValueError("ACE-Step source root is missing pyproject.toml or uv.lock")

    components = {
        "DiT": model_root / "acestep-v15-sft",
        "text encoder": model_root / "Qwen3-Embedding-0.6B",
        "language model": model_root / "acestep-5Hz-lm-1.7B",
        "VAE": model_root / "vae",
    }
    for label, component in components.items():
        resolved = _absolute_local_path(component)
        if (
            not resolved.is_relative_to(model_root)
            or not resolved.is_dir()
            or _is_reparse_point(resolved)
        ):
            raise ValueError(f"local ACE-Step {label} component is invalid")
        if not _contains_safetensors(resolved):
            raise ValueError(f"local ACE-Step {label} component has no safetensors weights")

    os.environ["ACESTEP_PROJECT_ROOT"] = str(source_root)
    os.environ["ACESTEP_CHECKPOINTS_DIR"] = str(model_root)
    os.environ["ACESTEP_DISABLE_TQDM"] = "1"
    if str(source_root) not in sys.path:
        sys.path.insert(0, str(source_root))

    with contextlib.redirect_stdout(sys.stderr):
        from acestep.core.generation.handler import init_service_downloads
        from acestep.handler import AceStepHandler
        from acestep.llm_inference import LLMHandler
        from acestep.models.sft.configuration_acestep_v15 import AceStepConfig
        from acestep.models.sft.modeling_acestep_v15_base import (
            AceStepConditionGenerationModel,
        )
        import torch
        from transformers import AutoConfig, AutoModel, AutoModelForCausalLM

        AutoConfig.register("acestep", AceStepConfig, exist_ok=True)
        AutoModel.register(
            AceStepConfig, AceStepConditionGenerationModel, exist_ok=True
        )

        auto_model_from_pretrained = AutoModel.from_pretrained
        auto_causal_from_pretrained = AutoModelForCausalLM.from_pretrained

        def local_auto_model_from_pretrained(*args, **kwargs):
            kwargs["trust_remote_code"] = False
            return auto_model_from_pretrained(*args, **kwargs)

        def local_auto_causal_from_pretrained(*args, **kwargs):
            kwargs["trust_remote_code"] = False
            return auto_causal_from_pretrained(*args, **kwargs)

        AutoModel.from_pretrained = local_auto_model_from_pretrained
        AutoModelForCausalLM.from_pretrained = local_auto_causal_from_pretrained

        init_service_downloads.check_main_model_exists = lambda _path: True
        init_service_downloads.check_model_exists = lambda _name, _path: True
        init_service_downloads.check_vae_exists = lambda _name, _path: True
        init_service_downloads.ensure_main_model = _forbid_download
        init_service_downloads.ensure_dit_model = _forbid_download
        init_service_downloads.ensure_vae_model = _forbid_download
        init_service_downloads.InitServiceDownloadsMixin._sync_model_code_if_needed = (
            staticmethod(lambda _name, _path: None)
        )

        execution_config = _select_execution_config(torch)
        dit_handler = AceStepHandler()
        status, ok = dit_handler.initialize_service(
            project_root=str(source_root),
            config_path="acestep-v15-sft",
            device="auto",
            use_flash_attention=False,
            compile_model=False,
            offload_to_cpu=execution_config["offload_to_cpu"],
            offload_dit_to_cpu=execution_config["offload_dit_to_cpu"],
            quantization=execution_config["quantization"],
            prefer_source=None,
            use_mlx_dit=False,
        )
        if not ok:
            raise RuntimeError(f"initializing local ACE-Step SFT model failed: {status}")
        _verify_effective_execution_config(dit_handler, execution_config)

        llm_handler = LLMHandler()
        status, ok = llm_handler.initialize(
            checkpoint_dir=str(model_root),
            lm_model_path="acestep-5Hz-lm-1.7B",
            backend="pt",
            device="auto",
            offload_to_cpu=execution_config["offload_to_cpu"],
        )
        if not ok:
            raise RuntimeError(f"initializing local ACE-Step language model failed: {status}")
        _verify_effective_lm_execution_config(llm_handler, execution_config)

    _dit_handler = dit_handler
    _llm_handler = llm_handler
    _model_root = model_root
    _worker_cache = worker_cache
    return {
        "n_ctx_train": 0,
        "n_vocab": 0,
        "execution_config": execution_config,
    }


def _write_audio(descriptor, directory, stem):
    if descriptor is None:
        return None, None
    if set(descriptor) != {"data_base64", "content_type"}:
        raise ValueError(f"{stem} audio descriptor has unknown or missing fields")
    content_type = descriptor["content_type"].lower()
    suffix = _INPUT_SUFFIXES.get(content_type)
    if suffix is None:
        raise ValueError(f"{stem} audio has unsupported content type")
    try:
        data = base64.b64decode(descriptor["data_base64"], validate=True)
    except Exception as exc:
        raise ValueError(f"{stem} audio is not valid base64") from exc
    _validate_audio_byte_length(
        len(data), _MAX_INPUT_AUDIO_BYTES, stem
    )
    signature_matches = {
        "audio/aac": lambda: len(data) >= 2
        and data[0] == 0xFF
        and data[1] & 0xF6 == 0xF0,
        "audio/flac": lambda: data.startswith(b"fLaC"),
        "audio/m4a": lambda: data[4:8] == b"ftyp",
        "audio/mp4": lambda: data[4:8] == b"ftyp",
        "audio/mpeg": lambda: data.startswith(b"ID3")
        or (len(data) >= 2 and data[0] == 0xFF and data[1] & 0xE0 == 0xE0),
        "audio/mp3": lambda: data.startswith(b"ID3")
        or (len(data) >= 2 and data[0] == 0xFF and data[1] & 0xE0 == 0xE0),
        "audio/ogg": lambda: data.startswith(b"OggS"),
        "audio/opus": lambda: data.startswith(b"OggS"),
        "audio/wav": lambda: data.startswith(b"RIFF") and data[8:12] == b"WAVE",
        "audio/x-wav": lambda: data.startswith(b"RIFF")
        and data[8:12] == b"WAVE",
    }[content_type]()
    if not signature_matches:
        raise ValueError(f"{stem} audio bytes do not match the declared content type")
    path = directory / f"{stem}{suffix}"
    path.write_bytes(data)
    try:
        canonical_path, duration = _canonicalize_input_audio(path, directory, stem)
    except Exception as exc:
        raise ValueError(f"{stem} audio cannot be decoded") from exc
    return str(canonical_path), duration


def _first_audio_stream(container, label):
    streams = [stream for stream in container.streams if stream.type == "audio"]
    if not streams:
        raise ValueError(f"{label} audio has no decodable audio stream")
    return streams[0]


def _decoded_audio_metrics(path, label):
    with contextlib.redirect_stdout(sys.stderr):
        import av
        import numpy

        duration = 0.0
        peak = 0.0
        with av.open(str(path), mode="r") as container:
            stream = _first_audio_stream(container, label)
            for frame in container.decode(stream):
                sample_rate = int(frame.sample_rate or 0)
                if sample_rate <= 0 or sample_rate > 384_000 or frame.samples <= 0:
                    raise ValueError(f"{label} audio has invalid decoded metadata")
                samples = frame.to_ndarray()
                if samples.size == 0 or not numpy.isfinite(samples).all():
                    raise ValueError(f"{label} audio decoded to invalid samples")
                duration += float(frame.samples) / float(sample_rate)
                if duration > _MAX_AUDIO_DURATION_SECONDS:
                    raise ValueError(f"{label} audio duration is out of bounds")
                peak = max(peak, float(numpy.max(numpy.abs(samples))))
    if duration <= 0.0 or peak < 1e-6:
        raise ValueError(f"{label} audio is empty or silent")
    return duration, peak


def _canonicalize_input_audio(path, directory, stem):
    with contextlib.redirect_stdout(sys.stderr):
        import av
        import numpy
        import soundfile

        canonical_path = directory / f"{stem}.canonical.wav"
        writer = None
        resampler = None
        sample_rate = None
        channels = None
        total_samples = 0
        peak = 0.0

        def write_frame(frame):
            nonlocal total_samples, peak
            samples = frame.to_ndarray()
            if (
                samples.ndim != 2
                or samples.shape[0] != channels
                or samples.shape[1] <= 0
                or not numpy.isfinite(samples).all()
            ):
                raise ValueError(f"{stem} audio decoded to invalid samples")
            next_total = total_samples + int(samples.shape[1])
            if next_total > int(sample_rate * _MAX_AUDIO_DURATION_SECONDS):
                raise ValueError(f"{stem} audio duration is out of bounds")
            writer.write(samples.T)
            total_samples = next_total
            peak = max(peak, float(numpy.max(numpy.abs(samples))))

        try:
            with av.open(str(path), mode="r") as container:
                stream = _first_audio_stream(container, stem)
                for frame in container.decode(stream):
                    if resampler is None:
                        source_rate = int(frame.sample_rate or 0)
                        source_channels = len(frame.layout.channels)
                        if (
                            source_rate <= 0
                            or source_rate > 384_000
                            or source_channels <= 0
                        ):
                            raise ValueError(
                                f"{stem} audio has invalid decoded metadata"
                            )
                        sample_rate = min(source_rate, _MAX_CANONICAL_AUDIO_RATE)
                        channels = 1 if source_channels == 1 else 2
                        layout = "mono" if channels == 1 else "stereo"
                        resampler = av.AudioResampler(
                            format="fltp", layout=layout, rate=sample_rate
                        )
                        writer = soundfile.SoundFile(
                            str(canonical_path),
                            mode="w",
                            samplerate=sample_rate,
                            channels=channels,
                            format="WAV",
                            subtype="PCM_16",
                        )
                    for converted in resampler.resample(frame):
                        write_frame(converted)
                if resampler is not None:
                    for converted in resampler.resample(None):
                        write_frame(converted)
        finally:
            if writer is not None:
                writer.close()

    if total_samples <= 0 or peak < 1e-6:
        canonical_path.unlink(missing_ok=True)
        raise ValueError(f"{stem} audio is empty or silent")
    duration = float(total_samples) / float(sample_rate)
    if (
        not math.isfinite(duration)
        or duration <= 0.0
        or duration > _MAX_AUDIO_DURATION_SECONDS
    ):
        canonical_path.unlink(missing_ok=True)
        raise ValueError(f"{stem} audio duration is out of bounds")
    return canonical_path, duration


def _bounded_helper_text(value, label, maximum, allow_empty=True):
    if not isinstance(value, str) or "\x00" in value:
        raise RuntimeError(f"ACE-Step helper returned invalid {label}")
    if (not allow_empty and not value) or len(value) > maximum:
        raise RuntimeError(
            f"ACE-Step helper returned out-of-bounds {label} "
            f"({len(value)} characters, maximum {maximum})"
        )
    return value


def _apply_helper_result(params_dict, result, include_instrumental):
    params_dict["caption"] = _bounded_helper_text(
        result.caption,
        "caption",
        _MAX_HELPER_CAPTION_CHARS,
        allow_empty=False,
    )
    params_dict["lyrics"] = _bounded_helper_text(result.lyrics, "lyrics", 4096)
    if include_instrumental:
        params_dict["instrumental"] = bool(result.instrumental)
    if result.bpm is not None:
        bpm = int(result.bpm)
        if bpm < 30 or bpm > 300:
            raise RuntimeError("ACE-Step helper returned out-of-bounds bpm")
        params_dict["bpm"] = bpm
    if result.duration is not None:
        duration = float(result.duration)
        if not math.isfinite(duration) or duration < 10.0 or duration > 600.0:
            raise RuntimeError("ACE-Step helper returned out-of-bounds duration")
        params_dict["duration"] = duration
    for result_name, param_name, maximum in (
        ("keyscale", "keyscale", 32),
        ("language", "vocal_language", 16),
        ("timesignature", "timesignature", 16),
    ):
        value = getattr(result, result_name)
        if value:
            params_dict[param_name] = _bounded_helper_text(
                value, result_name, maximum
            )


def _validate_preprocess_payload(preprocess):
    if set(preprocess) != {"sample_mode", "sample_query", "use_format"}:
        raise ValueError("ACE-Step preprocess payload has unknown or missing fields")
    sample_mode = preprocess["sample_mode"]
    sample_query = preprocess["sample_query"]
    use_format = preprocess["use_format"]
    if not isinstance(sample_mode, bool) or not isinstance(use_format, bool):
        raise ValueError("ACE-Step preprocess flags must be booleans")
    if sample_query is not None and (
        not isinstance(sample_query, str)
        or not sample_query
        or len(sample_query) > 512
        or "\x00" in sample_query
    ):
        raise ValueError("ACE-Step sample_query is invalid")
    return sample_mode, sample_query, use_format


def _preprocess_generation_params(params_dict, preprocess, helpers):
    sample_mode, sample_query, use_format = _validate_preprocess_payload(preprocess)

    create_sample, format_sample = helpers
    if sample_mode or sample_query is not None:
        sample_result = create_sample(
            llm_handler=_llm_handler,
            query=sample_query or "NO USER INPUT",
            instrumental=bool(params_dict["instrumental"]),
            vocal_language=(
                None
                if params_dict["vocal_language"] in ("", "unknown")
                else params_dict["vocal_language"]
            ),
            temperature=params_dict["lm_temperature"],
            top_k=params_dict["lm_top_k"] or None,
            top_p=(
                None
                if params_dict["lm_top_p"] >= 1.0
                else params_dict["lm_top_p"]
            ),
            use_constrained_decoding=params_dict["use_constrained_decoding"],
        )
        if not sample_result.success:
            raise RuntimeError(
                sample_result.error
                or sample_result.status_message
                or "ACE-Step create_sample failed"
            )
        _apply_helper_result(params_dict, sample_result, include_instrumental=True)
        params_dict["use_cot_metas"] = False

    if use_format:
        metadata = {}
        for param_name, metadata_name in (
            ("bpm", "bpm"),
            ("duration", "duration"),
            ("keyscale", "keyscale"),
            ("timesignature", "timesignature"),
            ("vocal_language", "language"),
        ):
            value = params_dict[param_name]
            if value not in (None, "", "unknown", -1, -1.0):
                metadata[metadata_name] = value
        format_result = format_sample(
            llm_handler=_llm_handler,
            caption=params_dict["caption"],
            lyrics=params_dict["lyrics"],
            user_metadata=metadata or None,
            temperature=params_dict["lm_temperature"],
            top_k=params_dict["lm_top_k"] or None,
            top_p=(
                None
                if params_dict["lm_top_p"] >= 1.0
                else params_dict["lm_top_p"]
            ),
            use_constrained_decoding=params_dict["use_constrained_decoding"],
        )
        if not format_result.success:
            raise RuntimeError(
                format_result.error
                or format_result.status_message
                or "ACE-Step format_sample failed"
            )
        _apply_helper_result(params_dict, format_result, include_instrumental=False)


def _effective_step_count(params_dict):
    steps = int(params_dict["inference_steps"])
    timesteps = params_dict.get("timesteps")
    if timesteps is not None:
        schedule = [float(value) for value in timesteps]
        steps = len(schedule) - 1
    else:
        shift = float(params_dict["shift"])
        schedule = []
        for index in range(steps + 1):
            timestep = 1.0 - (index / steps)
            schedule.append(
                shift * timestep / (1.0 + (shift - 1.0) * timestep)
            )
    cover_noise_strength = float(params_dict["cover_noise_strength"])
    if cover_noise_strength <= 0.0:
        return steps
    effective_noise = 1.0 - cover_noise_strength
    start = min(
        range(len(schedule) - 1),
        key=lambda index: abs(schedule[index] - effective_noise),
    )
    return len(schedule) - 1 - start


def _prepare_generation(payload, temporary, apply_preprocess):
    if set(payload) != {
        "params",
        "preprocess",
        "config",
        "source_audio",
        "reference_audio",
    }:
        raise ValueError("generate payload has unknown or missing fields")
    params_dict = payload["params"]
    preprocess = payload["preprocess"]
    config_dict = payload["config"]
    if (
        not isinstance(params_dict, dict)
        or not isinstance(preprocess, dict)
        or not isinstance(config_dict, dict)
    ):
        raise ValueError("generate params, preprocess, and config must be objects")

    from acestep.inference import (
        GenerationConfig,
        GenerationParams,
        create_sample,
        format_sample,
    )

    audio_format = config_dict.get("audio_format")
    content_type = _CONTENT_TYPES.get(audio_format)
    if content_type is None:
        raise ValueError("unsupported ACE-Step output format")

    temporary = _absolute_local_path(temporary)
    inputs = temporary / "inputs"
    outputs = temporary / "outputs"
    inputs.mkdir()
    outputs.mkdir()
    params_dict = dict(params_dict)
    source_path, source_duration = _write_audio(
        payload["source_audio"], inputs, "source"
    )
    reference_path, reference_duration = _write_audio(
        payload["reference_audio"], inputs, "reference"
    )
    params_dict["src_audio"] = source_path
    params_dict["reference_audio"] = reference_path
    task_type = params_dict["task_type"]
    source_basis_duration = source_duration
    if source_basis_duration is None and task_type in ("cover", "cover-nofsq"):
        codes = params_dict.get("audio_codes") or ""
        if isinstance(codes, list):
            codes = codes[0] if codes else ""
        if isinstance(codes, str) and codes:
            source_basis_duration = codes.count("<|audio_code_") / 5.0
    if source_basis_duration is not None and (
        task_type in ("cover", "cover-nofsq", "repaint")
        or params_dict.get("flow_edit_morph")
    ):
        requested_duration = float(params_dict["duration"])
        if requested_duration > 0.0 and abs(requested_duration - source_basis_duration) > 0.05:
            raise ValueError(
                "source-driven ACE-Step duration conflicts with decoded source duration"
            )
        params_dict["duration"] = source_basis_duration
        if (
            params_dict["fade_in_duration"] + params_dict["fade_out_duration"]
            > source_basis_duration
        ):
            raise ValueError(
                "fade durations exceed the decoded source duration"
            )
        if task_type == "repaint":
            if params_dict["repainting_start"] >= source_basis_duration:
                raise ValueError(
                    "repainting_start must be below decoded source duration"
                )
            repainting_end = params_dict["repainting_end"]
            if repainting_end > source_basis_duration:
                raise ValueError(
                    "repainting_end exceeds decoded source duration"
                )
    if apply_preprocess:
        with contextlib.redirect_stdout(sys.stderr):
            _preprocess_generation_params(
                params_dict, preprocess, (create_sample, format_sample)
            )
    else:
        _validate_preprocess_payload(preprocess)
    params = GenerationParams(**params_dict)
    config = GenerationConfig(**config_dict)
    effective_step_count = _effective_step_count(params_dict)
    return (
        params_dict,
        params,
        config,
        effective_step_count,
        content_type,
        outputs,
        source_duration,
        reference_duration,
    )


def _validate(payload):
    if _dit_handler is None or _model_root is None or _worker_cache is None:
        raise RuntimeError("ACE-Step model is not loaded")
    request_root = _worker_cache / "requests"
    request_root.mkdir(parents=True, exist_ok=True)
    with _temporary_request_directory(request_root) as temporary:
        (
            params_dict,
            _params,
            _config,
            effective_step_count,
            content_type,
            _outputs,
            source_duration,
            reference_duration,
        ) = _prepare_generation(payload, temporary, True)
        validated_params = dict(params_dict)
        validated_params["src_audio"] = payload["source_audio"] is not None
        validated_params["reference_audio"] = payload["reference_audio"] is not None
        return {
            "worker_operation": "ace-step-v0.1.8/validate-music-v2",
            "generation_params": validated_params,
            "generation_config": payload["config"],
            "preprocess": payload["preprocess"],
            "effective_step_count": effective_step_count,
            "content_type": content_type,
            "source_duration_seconds": source_duration,
            "reference_duration_seconds": reference_duration,
        }


def _generate(payload):
    if _dit_handler is None or _model_root is None or _worker_cache is None:
        raise RuntimeError("ACE-Step model is not loaded")
    from acestep.inference import generate_music

    request_root = _worker_cache / "requests"
    request_root.mkdir(parents=True, exist_ok=True)
    with _temporary_request_directory(request_root) as temporary:
        (
            params_dict,
            params,
            config,
            effective_step_count,
            content_type,
            outputs,
            _source_duration,
            _reference_duration,
        ) = _prepare_generation(payload, temporary, True)
        audio_format = payload["config"]["audio_format"]
        if _can_transcode_with_ffmpeg(audio_format):
            config.audio_format = "flac"
        with contextlib.redirect_stdout(sys.stderr):
            result = generate_music(
                _dit_handler,
                _llm_handler,
                params,
                config,
                save_dir=str(outputs),
            )
        if not result.success:
            raise RuntimeError(result.error or result.status_message or "generation failed")
        if not result.audios:
            raise RuntimeError("ACE-Step returned no generated audio")

        artifacts = []
        for audio in result.audios:
            output_path = _require_local_file_under(
                audio.get("path", ""), outputs, "ACE-Step output path"
            )
            tensor = audio.get("tensor")
            sample_rate = int(audio.get("sample_rate") or 0)
            if tensor is None or sample_rate <= 0:
                raise RuntimeError("ACE-Step returned audio without duration metadata")
            duration = float(tensor.shape[-1]) / float(sample_rate)
            if not math.isfinite(duration) or duration <= 0.0 or duration > 600.0:
                raise RuntimeError("generated ACE-Step audio duration is out of bounds")
            _canonicalize_generated_wave(
                output_path, tensor, sample_rate, audio_format
            )
            output_path = _transcode_generated_audio(
                output_path, tensor, sample_rate, audio_format, payload["config"]
            )
            data = output_path.read_bytes()
            try:
                _validate_audio_byte_length(
                    len(data), _MAX_OUTPUT_AUDIO_BYTES, "generated ACE-Step"
                )
            except ValueError as exc:
                raise RuntimeError(str(exc)) from exc
            _validate_generated_audio(output_path, data, audio_format, duration)
            artifacts.append(
                {
                    "data_base64": base64.b64encode(data).decode("ascii"),
                    "content_type": content_type,
                    "duration_seconds": duration,
                }
            )
    return {
        "artifacts": artifacts,
        "step_count": effective_step_count,
    }


def _dispatch(operation, payload):
    if operation == "load":
        return _load(payload)
    if operation == "validate_music":
        return _validate(payload)
    if operation == "generate_music":
        return _generate(payload)
    if operation == "shutdown":
        return {"shutdown": True}
    raise ValueError(f"unsupported operation: {operation}")


def main():
    for line in sys.stdin:
        try:
            message = json.loads(line)
            if not isinstance(message, dict) or set(message) != {"id", "op", "payload"}:
                raise ValueError("worker message has unknown or missing fields")
            message_id = message["id"]
            operation = message["op"]
            result = _dispatch(operation, message["payload"])
            _reply(message_id, result=result)
            if operation == "shutdown":
                return
        except Exception as exc:
            _reply(locals().get("message_id", 0), error=exc)


if __name__ == "__main__":
    main()
