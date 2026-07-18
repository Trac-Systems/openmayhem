import base64
import io
import json
import math
import os
import sys

import numpy as np
import soundfile as sf
import soxr
import torch
from transformers import AutoModelForTDT, AutoProcessor
from transformers.utils import logging as transformers_logging


transformers_logging.set_verbosity_error()

CORE_CHUNK_SECONDS = 120.0
LEFT_CONTEXT_SECONDS = 10.0
RIGHT_CONTEXT_SECONDS = 2.0
MAX_AUDIO_SECONDS = 3 * 60 * 60
SENTENCE_ENDINGS = (".", "!", "?", "…")

model = None
processor = None
device = None
sample_rate = 16000
subsampling_factor = 8


def emit(request_id, *, ok=False, result=None, error=None):
    message = {"id": request_id, "ok": ok}
    if result is not None:
        message["result"] = result
    if error is not None:
        message["error"] = error
    print(json.dumps(message, ensure_ascii=False, allow_nan=False), flush=True)


def choose_device():
    requested = os.environ.get("MAYHEM_TRANSFORMERS_ASR_DEVICE", "auto").strip().lower()
    if requested == "auto":
        if torch.cuda.is_available():
            return torch.device("cuda")
        if hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
            return torch.device("mps")
        return torch.device("cpu")
    if requested == "cuda":
        if not torch.cuda.is_available():
            raise RuntimeError("MAYHEM_TRANSFORMERS_ASR_DEVICE=cuda but CUDA is unavailable")
        return torch.device("cuda")
    if requested == "mps":
        if not hasattr(torch.backends, "mps") or not torch.backends.mps.is_available():
            raise RuntimeError("MAYHEM_TRANSFORMERS_ASR_DEVICE=mps but Metal/MPS is unavailable")
        return torch.device("mps")
    if requested == "cpu":
        return torch.device("cpu")
    raise RuntimeError(
        "MAYHEM_TRANSFORMERS_ASR_DEVICE must be auto, cuda, mps, or cpu"
    )


def load_model(payload):
    global model, processor, device, sample_rate, subsampling_factor

    model_path = payload["path"]
    if not os.path.isdir(model_path):
        raise RuntimeError("Transformers ASR model path must be a local directory")

    device = choose_device()
    if device.type == "cuda":
        torch.backends.cuda.matmul.allow_tf32 = False
        torch.backends.cudnn.allow_tf32 = False

    processor = AutoProcessor.from_pretrained(
        model_path,
        local_files_only=True,
        trust_remote_code=False,
    )
    model = AutoModelForTDT.from_pretrained(
        model_path,
        local_files_only=True,
        trust_remote_code=False,
        dtype=torch.float32,
    )
    model.eval()
    model.to(device)

    sample_rate = int(processor.feature_extractor.sampling_rate)
    encoder_config = getattr(model.config, "encoder_config", None)
    subsampling_factor = int(getattr(encoder_config, "subsampling_factor", 8))
    return {
        "device": device.type,
        "n_ctx_train": 0,
        "n_vocab": int(getattr(model.config, "vocab_size", 0)),
        "sample_rate": sample_rate,
        "subsampling_factor": subsampling_factor,
    }


def decode_audio(encoded):
    try:
        payload = base64.b64decode(encoded, validate=True)
    except Exception as error:
        raise RuntimeError(f"audio payload is not valid base64: {error}") from error
    if not payload:
        raise RuntimeError("audio transcription input cannot be empty")

    try:
        samples, source_rate = sf.read(
            io.BytesIO(payload),
            dtype="float32",
            always_2d=True,
        )
    except Exception as error:
        raise RuntimeError(f"audio payload cannot be decoded: {error}") from error
    if samples.shape[0] == 0:
        raise RuntimeError("decoded audio contains no samples")

    samples = np.mean(samples, axis=1, dtype=np.float32)
    if int(source_rate) != sample_rate:
        samples = soxr.resample(samples, int(source_rate), sample_rate, quality="HQ")
    samples = np.asarray(samples, dtype=np.float32)
    if not np.isfinite(samples).all():
        raise RuntimeError("decoded audio contains non-finite samples")

    duration = float(samples.shape[0]) / float(sample_rate)
    if duration > MAX_AUDIO_SECONDS:
        raise RuntimeError(
            f"audio duration {duration:.3f}s exceeds the calibrated {MAX_AUDIO_SECONDS}s limit"
        )
    return samples, duration


def words_from_timestamps(timestamps):
    words = []
    for timestamp in timestamps:
        if not isinstance(timestamp, dict):
            raise RuntimeError("Parakeet processor returned an invalid timestamp entry")
        token = timestamp.get("token")
        if not isinstance(token, str):
            raise RuntimeError("Parakeet processor timestamp is missing its token text")
        pieces = token.split()
        if not pieces:
            continue

        try:
            start = float(timestamp["start"])
            end = float(timestamp["end"])
        except (KeyError, TypeError, ValueError) as error:
            raise RuntimeError(
                "Parakeet processor returned an invalid timestamp range"
            ) from error
        if not math.isfinite(start) or not math.isfinite(end) or start < 0 or end < start:
            raise RuntimeError("Parakeet processor returned an invalid timestamp range")

        begins_word = token[0].isspace()
        for index, piece in enumerate(pieces):
            if begins_word or index > 0 or not words:
                words.append({"text": piece, "start": start, "end": end})
            else:
                words[-1]["text"] += piece
                words[-1]["end"] = max(words[-1]["end"], end)

    return words


def transcribe_window(samples):
    inputs = processor(samples, sampling_rate=sample_rate, return_tensors="pt")
    moved = {}
    for name, value in inputs.items():
        if torch.is_tensor(value):
            value = value.to(device)
            if value.is_floating_point():
                value = value.to(torch.float32)
        moved[name] = value

    with torch.inference_mode():
        output = model.generate(
            **moved,
            return_dict_in_generate=True,
        )
    decoded = processor.decode(
        output.sequences,
        durations=output.durations,
        skip_special_tokens=True,
    )
    if not isinstance(decoded, tuple) or len(decoded) != 2:
        raise RuntimeError("Parakeet processor did not return canonical timestamps")
    decoded_texts, timestamp_batches = decoded
    if len(decoded_texts) != 1 or len(timestamp_batches) != 1:
        raise RuntimeError("Parakeet processor returned an unexpected decode batch size")
    return decoded_texts[0].strip(), words_from_timestamps(timestamp_batches[0])


def render_text(words):
    return " ".join(word["text"] for word in words).strip()


def build_segments(words):
    segments = []
    current = []
    for word in words:
        if current and (
            word["start"] - current[-1]["end"] >= 1.5
            or word["end"] - current[0]["start"] >= 30.0
        ):
            segments.append(
                {
                    "text": render_text(current),
                    "start": current[0]["start"],
                    "end": current[-1]["end"],
                }
            )
            current = []
        current.append(word)
        if word["text"].endswith(SENTENCE_ENDINGS):
            segments.append(
                {
                    "text": render_text(current),
                    "start": current[0]["start"],
                    "end": current[-1]["end"],
                }
            )
            current = []
    if current:
        segments.append(
            {
                "text": render_text(current),
                "start": current[0]["start"],
                "end": current[-1]["end"],
            }
        )
    return segments


def reconcile_word_timestamps(words, duration):
    reconciled = []
    for raw_word in words:
        word = dict(raw_word)
        word["start"] = max(0.0, min(float(word["start"]), duration))
        word["end"] = max(0.0, min(float(word["end"]), duration))
        if word["end"] <= word["start"]:
            raise RuntimeError("Parakeet returned a word outside the decoded audio duration")
        if reconciled and word["start"] < reconciled[-1]["end"]:
            previous = reconciled[-1]
            previous_center = (previous["start"] + previous["end"]) / 2.0
            current_center = (word["start"] + word["end"]) / 2.0
            if current_center <= previous_center:
                raise RuntimeError("Parakeet returned out-of-order word timestamps")
            boundary = (previous_center + current_center) / 2.0
            previous["end"] = min(previous["end"], boundary)
            word["start"] = max(word["start"], boundary)
            if previous["end"] <= previous["start"] or word["end"] <= word["start"]:
                raise RuntimeError("Parakeet returned irreconcilable overlapping word timestamps")
        reconciled.append(word)
    return reconciled


def transcribe(payload):
    if model is None or processor is None:
        raise RuntimeError("Transformers ASR model has not been loaded")
    if payload.get("language"):
        raise RuntimeError(
            "this Transformers TDT model detects language automatically and does not accept language forcing"
        )
    if payload.get("prompt"):
        raise RuntimeError("this Transformers TDT model does not support transcription prompts")

    samples, duration = decode_audio(payload["audio_base64"])
    core_samples = max(1, int(CORE_CHUNK_SECONDS * sample_rate))
    left_samples = int(LEFT_CONTEXT_SECONDS * sample_rate)
    right_samples = int(RIGHT_CONTEXT_SECONDS * sample_rate)

    if samples.shape[0] <= core_samples:
        decoded_text, words = transcribe_window(samples)
        text = decoded_text or render_text(words)
    else:
        words = []
        for core_start in range(0, samples.shape[0], core_samples):
            core_end = min(samples.shape[0], core_start + core_samples)
            load_start = max(0, core_start - left_samples)
            load_end = min(samples.shape[0], core_end + right_samples)
            _, window_words = transcribe_window(samples[load_start:load_end])
            absolute_start = float(core_start) / float(sample_rate)
            absolute_end = float(core_end) / float(sample_rate)
            offset = float(load_start) / float(sample_rate)
            final_core = core_end == samples.shape[0]
            for word in window_words:
                word = dict(word)
                word["start"] += offset
                word["end"] += offset
                midpoint = (word["start"] + word["end"]) / 2.0
                if midpoint >= absolute_start and (
                    midpoint < absolute_end or final_core and midpoint <= absolute_end
                ):
                    words.append(word)
        text = render_text(words)

    if not text:
        raise RuntimeError("Transformers ASR produced an empty transcript")
    if not words:
        raise RuntimeError("Transformers ASR produced no timestamped words")

    words = reconcile_word_timestamps(words, duration)
    text = render_text(words)
    for item in words:
        item["start"] = round(max(0.0, float(item["start"])), 6)
        item["end"] = round(max(item["start"], float(item["end"])), 6)
    segments = build_segments(words)
    for item in segments:
        item["start"] = round(max(0.0, float(item["start"])), 6)
        item["end"] = round(max(item["start"], float(item["end"])), 6)

    return {
        "text": text,
        "audio_seconds": max(1, int(math.ceil(duration))),
        "duration_seconds": round(duration, 6),
        "words": words,
        "segments": segments,
    }


def main():
    for line in sys.stdin:
        try:
            request = json.loads(line)
            request_id = request["id"]
            operation = request["op"]
            payload = request.get("payload") or {}
            if operation == "load":
                emit(request_id, ok=True, result=load_model(payload))
            elif operation == "transcribe":
                emit(request_id, ok=True, result=transcribe(payload))
            elif operation == "shutdown":
                emit(request_id, ok=True, result={})
                return
            else:
                emit(request_id, error=f"unsupported operation {operation!r}")
        except Exception as error:
            request_id = locals().get("request_id", 0)
            emit(request_id, error=str(error))


if __name__ == "__main__":
    main()
