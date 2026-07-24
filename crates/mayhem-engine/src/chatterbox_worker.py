import base64
import contextlib
import copy
import hashlib
import importlib.metadata
import inspect
import io
import json
import math
import os
import pathlib
import random
import sys
import tempfile
import wave


API_VERSION = 1
MODEL_FAMILY = "original_english"
RUNTIME_PACKAGE = "chatterbox-tts"
RUNTIME_VERSION = "0.1.7"
SOURCE_COMMIT = "59bc590b3cad826e5d5987745bf6844627a21ad5"
MODEL_REVISION = "5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18"
TTS_SOURCE_SHA256 = "7896787bc17e20eafcd1dce7b8a4a6ea3a6478baab771c60d63e9e81f5564195"
PERTH_COMMIT = "ce86c49d029f42272c1902eccb675556b9ed2330"
MAX_INPUT_CHARACTERS = 16 * 1024
MAX_INPUT_BYTES = MAX_INPUT_CHARACTERS * 4
EXPECTED_MAX_TEXT_TOKENS = 2048
MAX_REFERENCE_AUDIO_BYTES = 16 * 1024 * 1024
MAX_REFERENCE_AUDIO_SECONDS = 10
T3_REFERENCE_SECONDS = 6
S3GEN_REFERENCE_SECONDS = 10
MAX_OUTPUT_AUDIO_BYTES = 64 * 1024 * 1024
MAX_OUTPUT_AUDIO_SECONDS = 120
MAX_PROTOCOL_LINE_BYTES = 24 * 1024 * 1024
REQUIRED_MODEL_FILES = (
    "ve.safetensors",
    "t3_cfg.safetensors",
    "s3gen.safetensors",
    "tokenizer.json",
    "conds.pt",
)
EXPECTED_GENERATE_PARAMETERS = (
    "self",
    "text",
    "repetition_penalty",
    "min_p",
    "top_p",
    "audio_prompt_path",
    "exaggeration",
    "cfg_weight",
    "temperature",
)
HANDLED_CONTROLS = tuple(
    sorted(
        (
            "input",
            "reference_audio",
            "exaggeration",
            "cfg_weight",
            "temperature",
            "seed",
            "repetition_penalty",
            "min_p",
            "top_p",
        )
    )
)

_runtime = None


class ProtocolError(Exception):
    pass


def _reject_constant(value):
    raise ProtocolError(f"non-finite JSON number {value!r} is forbidden")


def _exact_object(value, fields, label):
    if not isinstance(value, dict):
        raise ProtocolError(f"{label} must be an object")
    actual = set(value)
    expected = set(fields)
    if actual != expected:
        raise ProtocolError(
            f"{label} fields must be exactly {sorted(expected)!r}, got {sorted(actual)!r}"
        )
    return value


def _integer(value, label, minimum=0, maximum=(1 << 63) - 1):
    if isinstance(value, bool) or not isinstance(value, int):
        raise ProtocolError(f"{label} must be an integer")
    if value < minimum or value > maximum:
        raise ProtocolError(f"{label} is outside its allowed range")
    return value


def _number(value, label, minimum, maximum):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ProtocolError(f"{label} must be a number")
    value = float(value)
    if not math.isfinite(value) or value < minimum or value > maximum:
        raise ProtocolError(f"{label} is outside its allowed range")
    return value


def _offline_environment():
    for name in (
        "HF_HUB_OFFLINE",
        "HF_DATASETS_OFFLINE",
        "TRANSFORMERS_OFFLINE",
        "DIFFUSERS_OFFLINE",
        "PIP_NO_INDEX",
        "UV_OFFLINE",
    ):
        if os.environ.get(name) != "1":
            raise ProtocolError(f"{name} must be 1 for the offline Chatterbox worker")


def _load_runtime_dependencies():
    import numpy
    import torch
    from chatterbox.tts import ChatterboxTTS, punc_norm

    return (
        numpy,
        torch,
        ChatterboxTTS,
        punc_norm,
        importlib.metadata.version(RUNTIME_PACKAGE),
    )


def _sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_runtime_sources(chatterbox_tts):
    source_path = pathlib.Path(inspect.getsourcefile(chatterbox_tts))
    if not source_path.is_absolute():
        raise ProtocolError("installed Chatterbox source path must be absolute")
    if not _real_regular_file(source_path):
        raise ProtocolError("installed Chatterbox source must be a regular file")
    source_sha256 = _sha256_file(source_path)
    if source_sha256 != TTS_SOURCE_SHA256:
        raise ProtocolError(
            "installed Chatterbox source does not match the pinned 0.1.7 release commit"
        )

    distribution = importlib.metadata.distribution("resemble-perth")
    direct_url_text = distribution.read_text("direct_url.json")
    if not direct_url_text:
        raise ProtocolError(
            "resemble-perth must be installed from its pinned immutable Git commit"
        )
    try:
        direct_url = json.loads(direct_url_text)
    except json.JSONDecodeError as error:
        raise ProtocolError("resemble-perth direct_url.json is invalid") from error
    commit_id = (
        direct_url.get("vcs_info", {}).get("commit_id")
        if isinstance(direct_url, dict)
        else None
    )
    if commit_id != PERTH_COMMIT:
        raise ProtocolError(
            f"resemble-perth must resolve to commit {PERTH_COMMIT}, got {commit_id!r}"
        )
    return source_sha256


def _select_device(torch):
    requested = os.environ.get("MAYHEM_CHATTERBOX_DEVICE", "auto").strip().lower()
    if requested not in {"auto", "cpu", "cuda", "mps"}:
        raise ProtocolError("MAYHEM_CHATTERBOX_DEVICE must be auto, cpu, cuda, or mps")
    cuda_available = bool(torch.cuda.is_available())
    mps_available = bool(
        hasattr(torch.backends, "mps") and torch.backends.mps.is_available()
    )
    if requested == "cuda":
        if not cuda_available:
            raise ProtocolError("CUDA was requested but is unavailable")
        return "cuda"
    if requested == "mps":
        if not mps_available:
            raise ProtocolError("MPS was requested but is unavailable")
        return "mps"
    if requested == "cpu":
        return "cpu"
    if cuda_available:
        return "cuda"
    if mps_available:
        return "mps"
    return "cpu"


def _real_regular_file(path):
    return path.is_file() and not path.is_symlink()


def _handle_load(payload):
    global _runtime
    payload = _exact_object(
        payload,
        ("cache_root", "input_character_limit", "model_root"),
        "load payload",
    )
    _offline_environment()
    model_root = pathlib.Path(payload["model_root"])
    cache_root = pathlib.Path(payload["cache_root"])
    if not model_root.is_absolute() or not cache_root.is_absolute():
        raise ProtocolError("model_root and cache_root must be absolute")
    if not model_root.is_dir() or model_root.is_symlink():
        raise ProtocolError("model_root must be a real directory")
    if not cache_root.is_dir() or cache_root.is_symlink():
        raise ProtocolError("cache_root must be a real directory")
    for name in REQUIRED_MODEL_FILES:
        if not _real_regular_file(model_root / name):
            raise ProtocolError(f"model_root is missing required regular file {name}")
    inputs_root = cache_root / "inputs"
    inputs_root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if inputs_root.is_symlink():
        raise ProtocolError("worker input directory must not be a symlink")

    numpy, torch, chatterbox_tts, punc_norm, runtime_version = (
        _load_runtime_dependencies()
    )
    if runtime_version != RUNTIME_VERSION:
        raise ProtocolError(
            f"{RUNTIME_PACKAGE} version must be {RUNTIME_VERSION}, got {runtime_version}"
        )
    runtime_source_sha256 = _verify_runtime_sources(chatterbox_tts)
    generate_parameters = tuple(inspect.signature(chatterbox_tts.generate).parameters)
    if generate_parameters != EXPECTED_GENERATE_PARAMETERS:
        raise ProtocolError(
            "installed Chatterbox generate API does not match the pinned original English API"
        )
    device = _select_device(torch)
    input_character_limit = _integer(
        payload["input_character_limit"],
        "input_character_limit",
        1,
        MAX_INPUT_CHARACTERS,
    )
    with contextlib.redirect_stdout(sys.stderr):
        model = chatterbox_tts.from_local(model_root, device)
    sample_rate = _integer(int(model.sr), "sample rate", 8_000, 192_000)
    max_text_tokens = _integer(
        int(model.t3.hp.max_text_tokens),
        "T3 max_text_tokens",
        1,
        MAX_INPUT_CHARACTERS,
    )
    if max_text_tokens != EXPECTED_MAX_TEXT_TOKENS:
        raise ProtocolError(
            "original Chatterbox T3 max_text_tokens must be "
            f"{EXPECTED_MAX_TEXT_TOKENS}, got {max_text_tokens}"
        )
    if not callable(punc_norm) or not callable(
        getattr(model.tokenizer, "text_to_tokens", None)
    ):
        raise ProtocolError(
            "original Chatterbox normalization/tokenizer surface is unavailable"
        )
    if model.conds is None:
        raise ProtocolError("original Chatterbox model has no built-in conds.pt voice")
    default_conditionals = model.conds
    model.conds = None
    _runtime = {
        "cache_root": cache_root,
        "default_conditionals": default_conditionals,
        "device": device,
        "model": model,
        "numpy": numpy,
        "punc_norm": punc_norm,
        "sample_rate": sample_rate,
        "torch": torch,
        "input_character_limit": input_character_limit,
        "max_text_tokens": max_text_tokens,
    }
    return {
        "execution_config": {
            "api_version": API_VERSION,
            "device": device,
            "model_family": MODEL_FAMILY,
            "model_revision": MODEL_REVISION,
            "perth_commit": PERTH_COMMIT,
            "runtime_package": RUNTIME_PACKAGE,
            "runtime_version": runtime_version,
            "runtime_source_sha256": runtime_source_sha256,
            "sample_rate": sample_rate,
            "input_byte_limit": MAX_INPUT_BYTES,
            "input_character_limit": input_character_limit,
            "max_text_tokens": max_text_tokens,
            "reference_audio_limit_seconds": MAX_REFERENCE_AUDIO_SECONDS,
            "s3gen_reference_seconds": S3GEN_REFERENCE_SECONDS,
            "seed_semantics": "official_gradio_global_rng_nonzero",
            "source_commit": SOURCE_COMMIT,
            "supports_voice_cloning": True,
            "t3_reference_seconds": T3_REFERENCE_SECONDS,
        },
        "n_ctx_train": max_text_tokens,
        "n_vocab": 0,
    }


def _decode_reference_audio(value):
    if value is None:
        return None
    value = _exact_object(
        value, ("content_type", "data_base64"), "reference_audio"
    )
    if value["content_type"] not in {"audio/wav", "audio/x-wav"}:
        raise ProtocolError("reference_audio must use audio/wav")
    encoded = value["data_base64"]
    if not isinstance(encoded, str) or not encoded:
        raise ProtocolError("reference_audio data_base64 must be a non-empty string")
    try:
        decoded = base64.b64decode(encoded, validate=True)
    except Exception as error:
        raise ProtocolError(f"reference_audio is invalid base64: {error}") from error
    if len(decoded) < 44 or len(decoded) > MAX_REFERENCE_AUDIO_BYTES:
        raise ProtocolError("reference_audio is outside its byte bound")
    if decoded[:4] != b"RIFF" or decoded[8:12] != b"WAVE":
        raise ProtocolError("reference_audio is not a WAV")
    try:
        with wave.open(io.BytesIO(decoded), "rb") as wav:
            frame_rate = wav.getframerate()
            frame_count = wav.getnframes()
    except (EOFError, wave.Error) as error:
        raise ProtocolError("reference_audio is not a supported PCM WAV") from error
    if frame_rate <= 0 or frame_count <= 0:
        raise ProtocolError("reference_audio WAV is empty")
    if frame_count > frame_rate * MAX_REFERENCE_AUDIO_SECONDS:
        raise ProtocolError(
            f"reference_audio exceeds the {MAX_REFERENCE_AUDIO_SECONDS}-second bound"
        )
    return decoded


def _default_conditionals_for_request():
    conditionals = copy.copy(_runtime["default_conditionals"])
    if hasattr(conditionals, "gen"):
        conditionals.gen = dict(conditionals.gen)
    return conditionals


def _apply_seed(seed, numpy, torch):
    if seed in (None, 0):
        return False
    _integer(seed, "seed", 1, (1 << 32) - 1)
    torch.manual_seed(seed)
    if hasattr(torch, "cuda"):
        if hasattr(torch.cuda, "manual_seed"):
            torch.cuda.manual_seed(seed)
        if hasattr(torch.cuda, "manual_seed_all"):
            torch.cuda.manual_seed_all(seed)
    random.seed(seed)
    numpy.random.seed(seed)
    return True


def _preflight_text_tokens(text):
    normalized = _runtime["punc_norm"](text)
    if not isinstance(normalized, str) or not normalized:
        raise ProtocolError("pinned Chatterbox text normalization returned invalid text")
    try:
        tokens = _runtime["model"].tokenizer.text_to_tokens(normalized)
        token_count = int(tokens.shape[-1])
    except Exception as error:
        raise ProtocolError(
            "pinned Chatterbox tokenizer could not preflight the input"
        ) from error
    _integer(token_count, "normalized text token count", 1, MAX_INPUT_CHARACTERS)
    max_text_tokens = _runtime["max_text_tokens"]
    if token_count > max_text_tokens:
        raise ProtocolError(
            f"normalized input has {token_count} text tokens, exceeding pinned "
            f"T3 max_text_tokens={max_text_tokens}; input was not truncated"
        )
    return token_count


def _waveform_to_pcm16(waveform, sample_rate, torch):
    tensor = waveform.detach().to(device="cpu", dtype=torch.float32).squeeze()
    if tensor.ndim != 1:
        raise ProtocolError("Chatterbox returned non-mono audio")
    sample_count = _integer(
        int(tensor.numel()),
        "sample count",
        1,
        sample_rate * MAX_OUTPUT_AUDIO_SECONDS,
    )
    if not bool(torch.isfinite(tensor).all().item()):
        raise ProtocolError("Chatterbox returned non-finite audio")
    pcm = (
        torch.clamp(tensor, -1.0, 1.0)
        .mul(32767.0)
        .round()
        .to(torch.int16)
        .contiguous()
        .numpy()
        .astype("<i2", copy=False)
        .tobytes()
    )
    if len(pcm) != sample_count * 2:
        raise ProtocolError("Chatterbox PCM size does not match its sample count")
    return pcm, sample_count


def _wav_bytes(pcm, sample_rate):
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(pcm)
    result = output.getvalue()
    if len(result) < 44 or len(result) > MAX_OUTPUT_AUDIO_BYTES:
        raise ProtocolError("Chatterbox WAV is outside its output bound")
    return result


def _handle_synthesize(payload):
    if _runtime is None:
        raise ProtocolError("Chatterbox model has not been loaded")
    payload = _exact_object(payload, HANDLED_CONTROLS, "synthesize payload")
    text = payload["input"]
    if not isinstance(text, str) or not text.strip():
        raise ProtocolError("input must be a non-empty string")
    if (
        len(text.encode("utf-8")) > MAX_INPUT_BYTES
        or len(text) > _runtime["input_character_limit"]
    ):
        raise ProtocolError("input exceeds the signed Chatterbox text bound")
    _preflight_text_tokens(text)
    reference_audio = _decode_reference_audio(payload["reference_audio"])
    exaggeration = _number(payload["exaggeration"], "exaggeration", 0.25, 2.0)
    cfg_weight = _number(payload["cfg_weight"], "cfg_weight", 0.0, 1.0)
    temperature = _number(payload["temperature"], "temperature", 0.05, 5.0)
    repetition_penalty = _number(
        payload["repetition_penalty"], "repetition_penalty", 1.0, 2.0
    )
    min_p = _number(payload["min_p"], "min_p", 0.0, 1.0)
    top_p = _number(payload["top_p"], "top_p", 0.0, 1.0)
    seed = payload["seed"]
    if seed is not None:
        _integer(seed, "seed", 0, (1 << 32) - 1)
    seed_applied = _apply_seed(seed, _runtime["numpy"], _runtime["torch"])

    reference_path = None
    model = _runtime["model"]
    try:
        if reference_audio is None:
            model.conds = _default_conditionals_for_request()
        else:
            model.conds = None
            with tempfile.NamedTemporaryFile(
                mode="xb",
                suffix=".wav",
                prefix="reference-",
                dir=_runtime["cache_root"] / "inputs",
                delete=False,
            ) as reference_file:
                reference_file.write(reference_audio)
                reference_file.flush()
                reference_path = pathlib.Path(reference_file.name)
        with contextlib.redirect_stdout(sys.stderr):
            waveform = model.generate(
                text,
                repetition_penalty=repetition_penalty,
                min_p=min_p,
                top_p=top_p,
                audio_prompt_path=(
                    str(reference_path) if reference_path is not None else None
                ),
                exaggeration=exaggeration,
                cfg_weight=cfg_weight,
                temperature=temperature,
            )
    finally:
        model.conds = None
        if reference_path is not None:
            try:
                reference_path.unlink()
            except FileNotFoundError:
                pass

    sample_rate = _runtime["sample_rate"]
    pcm, sample_count = _waveform_to_pcm16(
        waveform, sample_rate, _runtime["torch"]
    )
    wav = _wav_bytes(pcm, sample_rate)
    return {
        "content_type": "audio/wav",
        "data_base64": base64.b64encode(wav).decode("ascii"),
        "duration_seconds": sample_count / sample_rate,
        "handled_controls": list(HANDLED_CONTROLS),
        "reference_audio_used": reference_audio is not None,
        "sample_count": sample_count,
        "sample_rate": sample_rate,
        "seed_applied": seed_applied,
        "sha256": hashlib.sha256(wav).hexdigest(),
    }


def _emit(value):
    encoded = json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    )
    sys.stdout.write(encoded)
    sys.stdout.write("\n")
    sys.stdout.flush()


def _handle_message(message):
    message = _exact_object(message, ("id", "op", "payload"), "worker message")
    message_id = _integer(message["id"], "message id")
    operation = message["op"]
    if not isinstance(operation, str):
        raise ProtocolError("worker operation must be a string")
    if operation == "load":
        result = _handle_load(message["payload"])
    elif operation == "synthesize":
        result = _handle_synthesize(message["payload"])
    elif operation == "shutdown":
        if message["payload"] is not None:
            raise ProtocolError("shutdown payload must be null")
        result = {"shutdown": True}
    else:
        raise ProtocolError(f"unsupported worker operation {operation!r}")
    return message_id, operation, result


def main():
    while True:
        line = sys.stdin.buffer.readline(MAX_PROTOCOL_LINE_BYTES + 1)
        if not line:
            return
        if len(line) > MAX_PROTOCOL_LINE_BYTES or not line.endswith(b"\n"):
            _emit(
                {
                    "error": "worker request exceeded its protocol bound",
                    "id": 0,
                    "ok": False,
                }
            )
            return
        message_id = 0
        try:
            message = json.loads(line, parse_constant=_reject_constant)
            if isinstance(message, dict):
                candidate = message.get("id")
                if isinstance(candidate, int) and not isinstance(candidate, bool):
                    message_id = candidate
            message_id, operation, result = _handle_message(message)
            _emit({"id": message_id, "ok": True, "result": result})
            if operation == "shutdown":
                return
        except Exception as error:
            _emit({"error": str(error), "id": message_id, "ok": False})


if __name__ == "__main__":
    main()
