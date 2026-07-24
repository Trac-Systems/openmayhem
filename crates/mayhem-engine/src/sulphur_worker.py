import importlib
import base64
import contextlib
import json
import math
import os
import pathlib
import secrets
import stat
import struct
import subprocess
import sys


_API_VERSION = 1
_MAX_PROTOCOL_LINE_BYTES = 48 * 1024 * 1024
_MAX_OUTPUT_BYTES = 1024 * 1024 * 1024
_MAX_CONDITIONING_IMAGE_BYTES = 32 * 1024 * 1024
_REQUIRED_ADAPTER_FUNCTIONS = ("load", "describe", "validate_video", "generate_video")
_WINDOWS = os.name == "nt"

runtime_adapter = None
runtime = None
runtime_description = None
output_root = None
input_root = None
ffmpeg_path = None
ffprobe_path = None


def _reply(message_id, result=None, error=None):
    response = {"id": message_id, "ok": error is None}
    if error is None:
        response["result"] = result
    else:
        response["error"] = error
    print(json.dumps(response, separators=(",", ":")), flush=True)


def _require_object(value, label):
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def _require_exact_fields(value, expected, label):
    value = _require_object(value, label)
    if set(value) != set(expected):
        raise ValueError(f"{label} has unknown or missing fields")
    return value


def _absolute_local_path(value, label):
    path = pathlib.Path(value)
    if not path.is_absolute():
        raise ValueError(f"{label} must be absolute")
    return pathlib.Path(os.path.abspath(path))


def _is_link_or_reparse(path, metadata=None):
    metadata = metadata or os.lstat(path)
    return stat.S_ISLNK(metadata.st_mode) or bool(
        getattr(metadata, "st_file_attributes", 0)
        & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    )


def _existing_real_path(value, label):
    supplied = pathlib.Path(value)
    if _WINDOWS:
        path = _absolute_local_path(supplied, label)
        metadata = os.lstat(path)
        if _is_link_or_reparse(path, metadata):
            raise ValueError(f"{label} must not be a symlink or reparse point")
        return path, metadata
    if supplied.is_symlink():
        raise ValueError(f"{label} must not be a symlink")
    path = supplied.resolve(strict=True)
    return path, path.stat()


def _require_real_directory(value, label):
    path, metadata = _existing_real_path(value, label)
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError(f"{label} must be a real directory")
    return path


def _require_real_file(value, label):
    path, metadata = _existing_real_path(value, label)
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"{label} must be a real regular file")
    return path


def _ensure_private_directory(path, label):
    path = pathlib.Path(path)
    if _WINDOWS:
        path.mkdir(parents=True, exist_ok=True)
    else:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(path, 0o700)
    return _require_real_directory(path, label)


def _safe_module_name(value):
    if not isinstance(value, str) or not value or len(value) > 128:
        raise ValueError("Sulphur runtime module name is invalid")
    if any(not part.isidentifier() for part in value.split(".")):
        raise ValueError("Sulphur runtime module name is invalid")
    return value


def _validate_description(value, expected_backend):
    value = _require_exact_fields(
        value,
        {
            "api_version",
            "runtime_name",
            "runtime_version",
            "backend",
            "distilled",
            "joint_audio_video",
            "prompt_enhancer",
            "ltx_runtime_commit",
            "sulphur_source_commit",
            "distillation_mode",
            "stage_1_denoise_intervals",
            "stage_2_denoise_intervals",
        },
        "Sulphur runtime description",
    )
    if value["api_version"] != _API_VERSION:
        raise RuntimeError("Sulphur runtime adapter API version does not match")
    if value["backend"] != expected_backend:
        raise RuntimeError("Sulphur runtime adapter selected a different backend")
    if value["distilled"] is not True:
        raise RuntimeError("Sulphur runtime adapter did not load the distilled pipeline")
    if value["joint_audio_video"] is not True:
        raise RuntimeError("Sulphur runtime adapter does not guarantee joint audio-video output")
    if not isinstance(value["prompt_enhancer"], bool):
        raise RuntimeError("Sulphur runtime adapter returned invalid prompt-enhancer capability")
    if value["ltx_runtime_commit"] != "9377758131b1ffde4b7f766804590a6617bf2ab9":
        raise RuntimeError("Sulphur runtime adapter uses an unpinned LTX runtime")
    if value["sulphur_source_commit"] != "875e886e556b955d21149316fd631cc121db6cc1":
        raise RuntimeError("Sulphur runtime adapter uses an unpinned Sulphur source")
    expected_mode = (
        "dev_transformer_plus_pinned_distill_lora"
        if expected_backend == "gguf"
        else "native_distilled_artifact"
    )
    if value["distillation_mode"] != expected_mode:
        raise RuntimeError("Sulphur runtime adapter reported the wrong artifact/distillation mode")
    if value["stage_1_denoise_intervals"] != 8 or value["stage_2_denoise_intervals"] != 3:
        raise RuntimeError("Sulphur runtime adapter reported the wrong distilled denoise schedule")
    for field in ("runtime_name", "runtime_version"):
        if not isinstance(value[field], str) or not value[field] or len(value[field]) > 256:
            raise RuntimeError(f"Sulphur runtime adapter returned invalid {field}")
    return value


def _load(payload):
    global ffmpeg_path, ffprobe_path, input_root, output_root, runtime, runtime_adapter, runtime_description
    payload = _require_exact_fields(
        payload,
        {
            "artifact_path",
            "backend",
            "cache_root",
            "ffmpeg_path",
            "ffprobe_path",
            "model_root",
            "runtime_module",
        },
        "Sulphur load payload",
    )
    backend = payload["backend"]
    if backend not in ("gguf", "mlx"):
        raise ValueError("Sulphur backend must be gguf or mlx")
    model_root = _require_real_directory(payload["model_root"], "Sulphur model root")
    artifact_path = (
        _require_real_file(payload["artifact_path"], "Sulphur GGUF artifact")
        if backend == "gguf"
        else _require_real_directory(payload["artifact_path"], "Sulphur MLX artifact")
    )
    if artifact_path != model_root and model_root not in artifact_path.parents:
        raise ValueError("Sulphur artifact escaped its model root")
    cache_root = _require_real_directory(payload["cache_root"], "Sulphur cache root")
    output_root = _ensure_private_directory(
        cache_root / "outputs",
        "Sulphur output directory",
    )
    input_root = _ensure_private_directory(
        cache_root / "inputs",
        "Sulphur input directory",
    )
    _purge_worker_inputs()
    ffmpeg_path = _require_real_file(payload["ffmpeg_path"], "Sulphur ffmpeg executable")
    ffprobe_path = _require_real_file(payload["ffprobe_path"], "Sulphur ffprobe executable")
    runtime_adapter = importlib.import_module(_safe_module_name(payload["runtime_module"]))
    if getattr(runtime_adapter, "MAYHEM_SULPHUR_API_VERSION", None) != _API_VERSION:
        raise RuntimeError("Sulphur runtime adapter does not declare API version 1")
    for name in _REQUIRED_ADAPTER_FUNCTIONS:
        if not callable(getattr(runtime_adapter, name, None)):
            raise RuntimeError(f"Sulphur runtime adapter is missing {name}()")
    runtime = runtime_adapter.load(
        model_root=str(model_root),
        artifact_path=str(artifact_path),
        backend=backend,
        cache_root=str(cache_root),
    )
    runtime_description = _validate_description(runtime_adapter.describe(runtime), backend)
    execution_config = dict(runtime_description)
    execution_config["ffmpeg_version"] = _tool_version(ffmpeg_path, "ffmpeg")
    execution_config["ffprobe_version"] = _tool_version(ffprobe_path, "ffprobe")
    return {
        "n_ctx_train": 0,
        "n_vocab": 0,
        "worker_pid": os.getpid(),
        "process_group_id": os.getpgrp() if os.name == "posix" else None,
        "execution_config": execution_config,
    }


def _tool_version(path, label):
    completed = subprocess.run(
        [str(path), "-version"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=10,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"Sulphur {label} version probe failed")
    line = completed.stdout.splitlines()[0].decode("utf-8", "strict") if completed.stdout else ""
    if not line.startswith(f"{label} version ") or len(line) > 512:
        raise RuntimeError(f"Sulphur {label} returned invalid version evidence")
    return line


def _validate_request_payload(payload):
    if runtime is None or runtime_adapter is None:
        raise RuntimeError("Sulphur model is not loaded")
    payload = _require_exact_fields(
        payload,
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
        },
        "Sulphur generation payload",
    )
    if not isinstance(payload["prompt"], str) or not 0 < len(payload["prompt"].encode("utf-8")) <= 32768:
        raise ValueError("Sulphur prompt is outside its byte bound")
    if (
        not isinstance(payload["negative_prompt"], str)
        or len(payload["negative_prompt"].encode("utf-8")) > 32768
    ):
        raise ValueError("Sulphur negative_prompt is outside its byte bound")
    if not isinstance(payload["width"], int) or not isinstance(payload["height"], int):
        raise ValueError("Sulphur dimensions must be integers")
    if not 256 <= payload["width"] <= 2048 or payload["width"] % 64:
        raise ValueError("Sulphur width is outside the pinned geometry")
    if not 256 <= payload["height"] <= 2048 or payload["height"] % 64:
        raise ValueError("Sulphur height is outside the pinned geometry")
    if not isinstance(payload["num_frames"], int) or not 1 <= payload["num_frames"] <= 513:
        raise ValueError("Sulphur num_frames is outside its bound")
    if (payload["num_frames"] - 1) % 8:
        raise ValueError("Sulphur num_frames must be 8K+1")
    if not isinstance(payload["frame_rate"], (int, float)) or isinstance(payload["frame_rate"], bool):
        raise ValueError("Sulphur frame_rate must be numeric")
    if not 1.0 <= float(payload["frame_rate"]) <= 50.0:
        raise ValueError("Sulphur frame_rate is outside its bound")
    if not isinstance(payload["seed"], int) or isinstance(payload["seed"], bool):
        raise ValueError("Sulphur seed must be an integer")
    if not 0 <= payload["seed"] <= 0xFFFFFFFF:
        raise ValueError("Sulphur seed is outside its unsigned 32-bit bound")
    if not isinstance(payload["enhance_prompt"], bool):
        raise ValueError("Sulphur enhance_prompt must be boolean")
    if payload["enhance_prompt"] and not runtime_description["prompt_enhancer"]:
        raise ValueError("Sulphur prompt enhancement was not loaded by this runtime")
    images = payload["images"]
    if not isinstance(images, list) or len(images) > 16:
        raise ValueError("Sulphur images must contain at most 16 ordered conditions")
    for image in images:
        image = _require_exact_fields(
            image,
            {"content_type", "crf", "data_base64", "frame_index", "strength"},
            "Sulphur conditioning image",
        )
        if image["content_type"] not in ("image/png", "image/jpeg"):
            raise ValueError("Sulphur conditioning image has an unsupported content type")
        if not isinstance(image["data_base64"], str):
            raise ValueError("Sulphur conditioning image data must be base64 text")
        try:
            decoded = base64.b64decode(image["data_base64"], validate=True)
        except Exception as error:
            raise ValueError("Sulphur conditioning image contains invalid base64") from error
        if not 0 < len(decoded) <= _MAX_CONDITIONING_IMAGE_BYTES:
            raise ValueError("Sulphur conditioning image is outside its byte bound")
        if (
            not isinstance(image["frame_index"], int)
            or isinstance(image["frame_index"], bool)
            or not 0 <= image["frame_index"] < payload["num_frames"]
        ):
            raise ValueError("Sulphur conditioning image frame_index is outside the output")
        if (
            not isinstance(image["strength"], (int, float))
            or isinstance(image["strength"], bool)
            or not math.isfinite(float(image["strength"]))
            or not 0.0 <= float(image["strength"]) <= 1.0
        ):
            raise ValueError("Sulphur conditioning image strength is outside its bound")
        if (
            not isinstance(image["crf"], int)
            or isinstance(image["crf"], bool)
            or not 0 <= image["crf"] <= 51
        ):
            raise ValueError("Sulphur conditioning image crf is outside its bound")
        signature_ok = (
            image["content_type"] == "image/png" and decoded.startswith(b"\x89PNG\r\n\x1a\n")
        ) or (
            image["content_type"] == "image/jpeg" and decoded.startswith(b"\xff\xd8\xff")
        )
        if not signature_ok:
            raise ValueError("Sulphur conditioning image bytes do not match its content type")
    return payload


def _validate_runtime_request(payload, expected_controls):
    evidence = runtime_adapter.validate_video(runtime, payload)
    evidence = _require_exact_fields(
        evidence, {"handled_controls", "valid"}, "Sulphur runtime validation evidence"
    )
    handled = evidence["handled_controls"]
    if (
        evidence["valid"] is not True
        or not isinstance(handled, list)
        or any(not isinstance(control, str) for control in handled)
        or len(handled) != len(set(handled))
        or set(handled) != expected_controls
    ):
        raise RuntimeError("Sulphur runtime did not validate every exact generation control")
    return evidence


def _purge_worker_inputs():
    if input_root is None or not input_root.is_dir():
        return
    for entry in input_root.iterdir():
        if entry.is_file() or entry.is_symlink():
            entry.unlink(missing_ok=True)


@contextlib.contextmanager
def _materialized_request(payload):
    materialized = dict(payload)
    paths = []
    images = []
    try:
        for image in payload["images"]:
            decoded = base64.b64decode(image["data_base64"], validate=True)
            suffix = ".png" if image["content_type"] == "image/png" else ".jpg"
            while True:
                path = input_root / f"conditioning-{secrets.token_hex(16)}{suffix}"
                try:
                    descriptor = os.open(
                        path,
                        os.O_CREAT | os.O_EXCL | os.O_WRONLY,
                        0o600,
                    )
                    break
                except FileExistsError:
                    continue
            try:
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(decoded)
                    stream.flush()
                    os.fsync(stream.fileno())
            except BaseException:
                path.unlink(missing_ok=True)
                raise
            paths.append(path)
            images.append(
                {
                    "content_type": image["content_type"],
                    "crf": image["crf"],
                    "frame_index": image["frame_index"],
                    "path": str(path),
                    "strength": image["strength"],
                }
            )
        materialized["images"] = images
        yield materialized
    finally:
        for path in paths:
            path.unlink(missing_ok=True)


def _validate_request(payload):
    payload = _validate_request_payload(payload)
    with _materialized_request(payload) as materialized:
        return _validate_runtime_request(materialized, set(payload))


def _generate(payload):
    payload = _require_exact_fields(
        payload,
        {"output_path", "request"},
        "Sulphur worker generation payload",
    )
    request = _validate_request_payload(
        _require_object(payload["request"], "Sulphur generation request")
    )
    requested_output = pathlib.Path(payload["output_path"])
    if not requested_output.is_absolute():
        raise ValueError("Sulphur output path must be absolute")
    requested_output = (
        _absolute_local_path(requested_output, "Sulphur output path")
        if _WINDOWS
        else requested_output.resolve(strict=False)
    )
    bounded_output_root = output_root if _WINDOWS else output_root.resolve(strict=True)
    if requested_output.parent != bounded_output_root or requested_output.suffix.lower() != ".mp4":
        raise ValueError("Sulphur output path escaped its bounded MP4 output directory")
    if requested_output.exists():
        raise ValueError("Sulphur output path already exists")
    with _materialized_request(request) as materialized:
        _validate_runtime_request(materialized, set(request))
        result = runtime_adapter.generate_video(runtime, materialized, str(requested_output))
    result = _require_exact_fields(
        result,
        {
            "duration_seconds",
            "frame_count",
            "handled_controls",
            "stage_1_denoise_intervals",
            "stage_2_denoise_intervals",
        },
        "Sulphur generation result",
    )
    if (
        not isinstance(result["duration_seconds"], (int, float))
        or isinstance(result["duration_seconds"], bool)
        or not math.isfinite(float(result["duration_seconds"]))
        or result["duration_seconds"] <= 0.0
        or result["frame_count"] != request["num_frames"]
        or result["stage_1_denoise_intervals"] != 8
        or result["stage_2_denoise_intervals"] != 3
        or not isinstance(result["handled_controls"], list)
        or any(not isinstance(control, str) for control in result["handled_controls"])
        or len(result["handled_controls"]) != len(set(result["handled_controls"]))
        or set(result["handled_controls"]) != set(request)
    ):
        raise RuntimeError("Sulphur runtime returned invalid exact generation evidence")
    output = _require_real_file(requested_output, "Sulphur generated MP4")
    output_size = output.stat().st_size
    if output_size <= 0 or output_size > _MAX_OUTPUT_BYTES:
        output.unlink(missing_ok=True)
        raise RuntimeError("Sulphur generated MP4 has an invalid size")
    result["media_evidence"] = _probe_media(output, request)
    if abs(
        result["media_evidence"]["video_duration_seconds"]
        - float(result["duration_seconds"])
    ) > 1.0 / request["frame_rate"] + 1e-6:
        output.unlink(missing_ok=True)
        raise RuntimeError("Sulphur runtime duration differs from decoded video evidence")
    result["output_path"] = str(output)
    result["output_bytes"] = output_size
    return result


def _parse_rate(value):
    if not isinstance(value, str) or "/" not in value:
        raise RuntimeError("Sulphur ffprobe returned an invalid frame rate")
    numerator, denominator = value.split("/", 1)
    rate = float(numerator) / float(denominator)
    if not rate > 0.0:
        raise RuntimeError("Sulphur ffprobe returned a non-positive frame rate")
    return rate


def _probe_media(output, request):
    probe = subprocess.run(
        [
            str(ffprobe_path),
            "-v",
            "error",
            "-show_entries",
            "stream=index,codec_type,duration,avg_frame_rate:packet=stream_index,pts_time,dts_time",
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
    if probe.returncode != 0 or len(probe.stdout) > 2 * 1024 * 1024:
        raise RuntimeError("Sulphur ffprobe could not decode bounded MP4 evidence")
    evidence = _require_object(json.loads(probe.stdout), "Sulphur ffprobe evidence")
    streams = evidence.get("streams")
    packets = evidence.get("packets")
    if not isinstance(streams, list) or not isinstance(packets, list):
        raise RuntimeError("Sulphur ffprobe evidence is missing streams or packets")
    by_kind = {}
    for stream in streams:
        if not isinstance(stream, dict) or stream.get("codec_type") not in ("video", "audio"):
            continue
        kind = stream["codec_type"]
        if kind in by_kind:
            continue
        duration = float(stream.get("duration", "nan"))
        if not duration > 0.0:
            raise RuntimeError(f"Sulphur {kind} stream has no positive duration")
        by_kind[kind] = {
            "index": int(stream["index"]),
            "duration_seconds": duration,
            "fps": _parse_rate(stream["avg_frame_rate"]) if kind == "video" else None,
            "packet_count": 0,
            "last_timestamp": None,
        }
    if set(by_kind) != {"video", "audio"}:
        raise RuntimeError("Sulphur MP4 must contain both video and audio streams")
    index_to_kind = {value["index"]: kind for kind, value in by_kind.items()}
    for packet in packets:
        if not isinstance(packet, dict) or packet.get("stream_index") not in index_to_kind:
            continue
        kind = index_to_kind[packet["stream_index"]]
        timestamp = packet.get("dts_time") or packet.get("pts_time")
        if timestamp is None:
            raise RuntimeError(f"Sulphur {kind} packet has no timestamp")
        timestamp = float(timestamp)
        previous = by_kind[kind]["last_timestamp"]
        if not math.isfinite(timestamp) or (previous is not None and timestamp + 1e-9 < previous):
            raise RuntimeError(f"Sulphur {kind} packet timestamps are not monotonic")
        by_kind[kind]["last_timestamp"] = timestamp
        by_kind[kind]["packet_count"] += 1
    if by_kind["video"]["packet_count"] != request["num_frames"]:
        raise RuntimeError("Sulphur decoded video packet count differs from requested frames")
    if by_kind["audio"]["packet_count"] == 0:
        raise RuntimeError("Sulphur decoded audio stream contains no packets")
    if abs(by_kind["video"]["fps"] - request["frame_rate"]) > 1e-6:
        raise RuntimeError("Sulphur MP4 frame rate differs from the shared requested FPS")
    one_frame = 1.0 / request["frame_rate"]
    duration_delta = abs(
        by_kind["video"]["duration_seconds"] - by_kind["audio"]["duration_seconds"]
    )
    if duration_delta > one_frame + 1e-6:
        raise RuntimeError("Sulphur audio/video duration delta exceeds one frame")

    decoded = subprocess.run(
        [
            str(ffmpeg_path),
            "-v",
            "error",
            "-i",
            str(output),
            "-map",
            "0:a:0",
            "-f",
            "s16le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=30,
    )
    if decoded.returncode != 0 or not decoded.stdout or len(decoded.stdout) > 400_000:
        raise RuntimeError("Sulphur ffmpeg could not decode bounded audio evidence")
    usable = len(decoded.stdout) - (len(decoded.stdout) % 2)
    peak = max((abs(sample[0]) for sample in struct.iter_unpack("<h", decoded.stdout[:usable])), default=0)
    if peak <= 1:
        raise RuntimeError("Sulphur generated audio is silent")
    return {
        "video_duration_seconds": by_kind["video"]["duration_seconds"],
        "audio_duration_seconds": by_kind["audio"]["duration_seconds"],
        "duration_delta_seconds": duration_delta,
        "fps": by_kind["video"]["fps"],
        "video_packet_count": by_kind["video"]["packet_count"],
        "audio_packet_count": by_kind["audio"]["packet_count"],
        "timestamps_monotonic": True,
        "audio_peak_s16": peak,
        "ffprobe_decodable": True,
        "ffmpeg_audio_decodable": True,
    }


def main():
    if os.name == "posix":
        try:
            os.setsid()
        except PermissionError as error:
            raise RuntimeError("Sulphur worker could not create its process group") from error
    while True:
        raw_line = sys.stdin.buffer.readline(_MAX_PROTOCOL_LINE_BYTES + 1)
        if not raw_line:
            return
        if len(raw_line) > _MAX_PROTOCOL_LINE_BYTES or not raw_line.endswith(b"\n"):
            raise RuntimeError("Sulphur worker protocol line exceeded its bound")
        message_id = None
        try:
            message = _require_exact_fields(
                json.loads(raw_line), {"id", "op", "payload"}, "Sulphur worker message"
            )
            message_id = message["id"]
            operation = message["op"]
            if operation == "load":
                result = _load(message["payload"])
            elif operation == "validate_video":
                result = _validate_request(message["payload"])
            elif operation == "generate_video":
                result = _generate(message["payload"])
            elif operation == "shutdown":
                _reply(message_id, {"shutdown": True})
                return
            else:
                raise ValueError("unsupported Sulphur worker operation")
            _reply(message_id, result)
        except Exception as error:
            _reply(message_id, error=f"{type(error).__name__}: {error}")


if __name__ == "__main__":
    main()
