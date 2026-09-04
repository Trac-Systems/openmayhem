import base64
import copy
import inspect
import io
import json
import math
import queue
import sys
import threading
import types
import wave
from collections import deque


model = None
tokenizer = None
processor = None
multimodal = False
ctx_size = 2048
kv_cache_bits = None
kv_cache_group_size = 64
kv_cache_quantized_start_tokens = 0
request_queue = queue.Queue()
cancelled_requests = set()
cancelled_requests_lock = threading.Lock()
completed_request_id = 0


class RequestCancelled(Exception):
    pass


def mark_cancelled(request_id):
    with cancelled_requests_lock:
        request_id = int(request_id)
        if request_id > completed_request_id:
            cancelled_requests.add(request_id)


def finish_request(request_id):
    global completed_request_id
    with cancelled_requests_lock:
        completed_request_id = max(completed_request_id, int(request_id))
        completed_cancellations = {
            cancelled_id
            for cancelled_id in cancelled_requests
            if cancelled_id <= completed_request_id
        }
        cancelled_requests.difference_update(completed_cancellations)


def check_cancelled(request_id):
    with cancelled_requests_lock:
        cancelled = int(request_id) in cancelled_requests
    if cancelled:
        raise RequestCancelled("engine request cancelled")


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def required_sampling_kwargs(callable_obj, kwargs, requested):
    try:
        parameters = inspect.signature(callable_obj).parameters
    except (TypeError, ValueError):
        return kwargs
    if any(param.kind == inspect.Parameter.VAR_KEYWORD for param in parameters.values()):
        return kwargs
    missing = sorted(name for name in requested if name not in parameters)
    if missing:
        raise ValueError(
            "MLX backend does not support requested sampling parameter(s): "
            + ", ".join(missing)
        )
    return {key: value for key, value in kwargs.items() if key in parameters}


def tool_call_schema(tools):
    branches = []
    definitions = {}
    names = set()
    for index, tool in enumerate(tools):
        name = str(tool.get("name", ""))
        if not name:
            continue
        if name in names:
            raise ValueError(f"duplicate tool name {name!r}")
        names.add(name)
        definition = f"tool_{index}_parameters"
        reference = f"#/$defs/{definition}"
        parameters = copy.deepcopy(
            tool.get(
                "parameters",
                {"type": "object", "additionalProperties": True},
            )
        )
        rebase_local_json_schema_refs(parameters, reference)
        definitions[definition] = parameters
        branches.append(
            {
                "type": "object",
                "additionalProperties": False,
                "required": ["tool", "arguments"],
                "properties": {
                    "tool": {"const": name},
                    "arguments": {"$ref": reference},
                },
            }
        )
    if not branches:
        raise ValueError("tool-call grammar requires at least one tool")
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MayhemToolCall",
        "$defs": definitions,
        "oneOf": branches,
    }


def rebase_local_json_schema_refs(value, new_root):
    if isinstance(value, list):
        for item in value:
            rebase_local_json_schema_refs(item, new_root)
    elif isinstance(value, dict):
        reference = value.get("$ref")
        if reference == "#":
            value["$ref"] = new_root
        elif isinstance(reference, str) and reference.startswith("#/"):
            value["$ref"] = f"{new_root}/{reference[2:]}"
        for item in value.values():
            rebase_local_json_schema_refs(item, new_root)


def structured_logits_processor(payload):
    grammar = payload.get("grammar")
    if grammar is None:
        return None
    if not isinstance(grammar, dict):
        raise ValueError("MLX grammar must be an object")
    kind = str(grammar.get("kind") or "")
    if kind == "json_schema":
        schema = grammar.get("schema")
        if not isinstance(schema, dict):
            raise ValueError("MLX JSON-schema grammar requires an object schema")
    elif kind == "tool_call":
        tools = grammar.get("tools") or []
        if not isinstance(tools, list):
            raise ValueError("MLX tool-call grammar tools must be an array")
        schema = tool_call_schema(tools)
    elif kind == "gbnf":
        raise ValueError("MLX backend does not support GBNF grammar")
    else:
        raise ValueError(f"unsupported MLX grammar kind {kind!r}")

    from mlx_vlm.structured import (
        ThinkingAwareLogitsProcessor,
        build_json_schema_logits_processor,
    )

    constrained = build_json_schema_logits_processor(tokenizer, schema)
    # A required tool call must enter its schema immediately. Allowing free-form
    # reasoning first can exhaust the response budget before a call is emitted.
    if kind != "tool_call" and reasoning_enabled(payload):
        constrained = ThinkingAwareLogitsProcessor(
            constrained,
            tokenizer,
            enable_thinking=True,
        )
    return constrained


class StopSequenceStream:
    def __init__(self, request_id, stops):
        self.request_id = int(request_id)
        self.stops = [str(stop) for stop in stops]
        self.pending = deque()
        self.pending_text = ""
        self.output = ""
        self.stopped = False

    def push(self, chunk):
        text = str(chunk.get("text", ""))
        self.pending.append(dict(chunk))
        self.pending_text += text
        return self._flush_safe_text()

    def append_to_latest(self, text):
        text = str(text)
        if not text:
            return False
        if not self.pending:
            raise RuntimeError("MLX terminal text has no generated token")
        self.pending[-1]["text"] = str(self.pending[-1].get("text", "")) + text
        self.pending_text += text
        return self._flush_safe_text()

    def _flush_safe_text(self):
        positions = [
            position
            for stop in self.stops
            if (position := self.pending_text.find(stop)) >= 0
        ]
        if positions:
            self._flush_through(min(positions))
            self.stopped = True
            return True
        held_suffix = self._longest_stop_prefix_suffix()
        self._flush_complete_chunks(len(self.pending_text) - held_suffix)
        return False

    def finish(self):
        if self.stopped:
            return
        while self.pending:
            self._emit(self.pending.popleft())
        self.pending_text = ""

    def _longest_stop_prefix_suffix(self):
        held = 0
        for stop in self.stops:
            maximum = min(len(stop) - 1, len(self.pending_text))
            for length in range(maximum, held, -1):
                if self.pending_text.endswith(stop[:length]):
                    held = length
                    break
        return held

    def _flush_complete_chunks(self, safe_chars):
        # Keep the newest token until another token or finish arrives. MLX
        # detokenizers may emit its final buffered text only in a terminal event.
        while (
            len(self.pending) > 1
            and len(str(self.pending[0].get("text", ""))) <= safe_chars
        ):
            chunk = self.pending.popleft()
            text = str(chunk.get("text", ""))
            safe_chars -= len(text)
            self.pending_text = self.pending_text[len(text) :]
            self._emit(chunk)

    def _flush_through(self, allowed_chars):
        while self.pending:
            chunk = self.pending.popleft()
            text = str(chunk.get("text", ""))
            take = min(allowed_chars, len(text))
            chunk["text"] = text[:take]
            allowed_chars -= take
            self._emit(chunk)
        self.pending_text = ""

    def _emit(self, chunk):
        stream_chunk = bool(chunk.pop("_stream", True))
        text = str(chunk.get("text", ""))
        self.output += text
        if not stream_chunk:
            return
        send(
            {
                "id": self.request_id,
                "type": "token",
                "chunk": chunk,
            }
        )


class MlxGenerationEvents:
    def __init__(self, request_id, stops):
        self.stream = StopSequenceStream(request_id, stops)
        self.completion_tokens = 0

    def push(self, response):
        segment = str(getattr(response, "text", ""))
        token_value = getattr(response, "token", None)
        token = -1 if token_value is None else int(token_value)
        generated_value = getattr(response, "generation_tokens", None)
        generated = (
            int(generated_value)
            if generated_value is not None
            else self.completion_tokens + (1 if token >= 0 else 0)
        )
        if generated < self.completion_tokens or generated > self.completion_tokens + 1:
            raise RuntimeError(
                "MLX generation counter must advance by at most one token per event: "
                f"previous={self.completion_tokens}, current={generated}"
            )

        final_reason = getattr(response, "finish_reason", None)
        new_token = generated == self.completion_tokens + 1
        if new_token:
            if token < 0:
                raise RuntimeError("MLX generation event advanced without a token id")
            stopped = self.stream.push(
                {
                    "index": generated - 1,
                    "token_id": token,
                    "text": segment,
                }
            )
        else:
            if final_reason is None:
                raise RuntimeError(
                    "MLX repeated a generation counter before the terminal event"
                )
            stopped = self.stream.append_to_latest(segment)

        self.completion_tokens = generated
        return new_token, stopped, final_reason

    def finish(self):
        self.stream.finish()


def speciality_maps(payload):
    template_kwargs = {}
    sampling_kwargs = {}
    prompt_suffixes = []
    seen_names = set()
    for item in payload.get("speciality_parameters") or []:
        if not isinstance(item, dict):
            raise ValueError("MLX speciality parameter must be an object")
        name = str(item.get("name") or "")
        target = str(item.get("target") or "")
        native_path = str(item.get("native_path") or "")
        if not name or name in seen_names or not native_path:
            raise ValueError("MLX speciality parameters require unique names and native paths")
        seen_names.add(name)
        value = item.get("value")
        if target == "chat_template_kwarg":
            key = native_kwarg_name(native_path, "chat_template_kwargs.")
            if key in template_kwargs:
                raise ValueError(f"duplicate MLX chat-template speciality mapping {key!r}")
            template_kwargs[key] = value
        elif target == "sampling_parameter":
            key = native_kwarg_name(native_path, "sampling_params.")
            if key in sampling_kwargs:
                raise ValueError(f"duplicate MLX sampling speciality mapping {key!r}")
            sampling_kwargs[key] = value
        elif target == "prompt_suffix":
            if not isinstance(value, str):
                raise ValueError(f"MLX prompt-suffix speciality {name!r} must map to a string")
            prompt_suffixes.append(value)
        else:
            raise ValueError(f"unsupported MLX speciality target {target!r}")
    return template_kwargs, sampling_kwargs, prompt_suffixes


def native_kwarg_name(native_path, prefix):
    key = str(native_path)
    if key.startswith(prefix):
        key = key[len(prefix) :]
    if not key or not key.isidentifier():
        raise ValueError(f"native speciality path {native_path!r} is not a callable keyword")
    return key


def required_call_kwargs(callable_obj, kwargs, required, label):
    try:
        parameters = inspect.signature(callable_obj).parameters
    except (TypeError, ValueError):
        return kwargs
    if any(param.kind == inspect.Parameter.VAR_KEYWORD for param in parameters.values()):
        return kwargs
    missing = sorted(name for name in required if name not in parameters)
    if missing:
        raise ValueError(
            f"MLX {label} does not support requested speciality parameter(s): "
            + ", ".join(missing)
        )
    return {key: value for key, value in kwargs.items() if key in parameters}


def reasoning_enabled(payload):
    for item in payload.get("speciality_parameters") or []:
        name = str(item.get("name") or "").lower()
        native_path = str(item.get("native_path") or "").lower()
        haystack = f"{name} {native_path}"
        if "reason" not in haystack and "think" not in haystack:
            continue
        if any(marker in haystack for marker in ("preserve", "history", "retain")):
            continue
        value = item.get("value")
        level = str(item.get("level") or "").lower()
        if value is False or value == 0 or str(value).lower() in ("false", "off", "none", "disabled"):
            return False
        if level in ("none", "off", "disabled"):
            return False
        return True
    return False


def reasoning_budget(payload):
    budget = None
    for item in payload.get("speciality_parameters") or []:
        name = str(item.get("name") or "").lower()
        native_path = str(item.get("native_path") or "").lower()
        haystack = f"{name} {native_path}"
        if "reason" not in haystack and "think" not in haystack:
            continue
        if any(marker in haystack for marker in ("preserve", "history", "retain")):
            continue
        value = item.get("max_reasoning_tokens")
        if value is None:
            continue
        value = int(value)
        if value < 0:
            raise ValueError("MLX reasoning budget must not be negative")
        if budget is not None:
            raise ValueError("MLX received duplicate reasoning-budget specialities")
        budget = value
    return budget


def processor_chat_messages(messages):
    prepared = copy.deepcopy(messages)
    for message in prepared:
        content = message.get("content")
        if not isinstance(content, list):
            continue
        normalized = []
        for part in content:
            kind = part.get("type") if isinstance(part, dict) else None
            if kind == "image_url":
                normalized.append({"type": "image"})
            elif kind == "input_audio":
                normalized.append({"type": "audio"})
            elif kind == "video":
                normalized.append({"type": "video"})
            else:
                normalized.append(part)
        message["content"] = normalized
    return prepared


def request_prompt(payload, template_kwargs, prompt_suffixes, num_images=0, num_audios=0):
    prompt = str(payload.get("prompt", ""))
    template_tools = payload.get("tools") or []
    if not isinstance(template_tools, list):
        raise ValueError("MLX chat-template tools must be an array")
    if multimodal:
        if processor is None:
            raise RuntimeError("MLX multimodal processor has not been loaded")
        messages = processor_chat_messages(payload.get("messages") or [])
        if not messages:
            raise ValueError("MLX multimodal request is missing structured chat messages")
        kwargs = {
            "tokenize": False,
            "add_generation_prompt": True,
            **template_kwargs,
        }
        if template_tools:
            kwargs["tools"] = template_tools
        try:
            if hasattr(processor, "apply_chat_template"):
                prompt = str(processor.apply_chat_template(messages, **kwargs))
            else:
                from mlx_vlm.prompt_utils import apply_chat_template

                kwargs.pop("tokenize", None)
                prompt = str(
                    apply_chat_template(
                        processor,
                        model.config,
                        messages,
                        num_images=num_images,
                        num_audios=num_audios,
                        **kwargs,
                    )
                )
        except Exception as exc:
            raise ValueError(f"MLX multimodal chat template failed: {exc}") from exc
        return prompt + "".join(prompt_suffixes)
    if template_kwargs or template_tools:
        messages = payload.get("messages") or []
        if not messages:
            raise ValueError("MLX chat-template request is missing structured messages")
        if tokenizer is None or not hasattr(tokenizer, "apply_chat_template"):
            raise ValueError("MLX tokenizer does not expose the requested chat-template speciality")
        kwargs = {
            "tokenize": False,
            "add_generation_prompt": True,
            **template_kwargs,
        }
        if template_tools:
            kwargs["tools"] = template_tools
        try:
            prompt = str(
                tokenizer.apply_chat_template(
                    messages,
                    **required_call_kwargs(
                        tokenizer.apply_chat_template,
                        kwargs,
                        set(template_kwargs) | ({"tools"} if template_tools else set()),
                        "chat template",
                    ),
                )
            )
        except Exception as exc:
            raise ValueError(f"MLX model chat template failed: {exc}") from exc
    return prompt + "".join(prompt_suffixes)


def encode_text(text):
    if tokenizer is None:
        raise RuntimeError("model has not been loaded")
    bos_token = getattr(tokenizer, "bos_token", None)
    add_special_tokens = bos_token is None or not str(text).startswith(str(bos_token))
    return [int(token) for token in tokenizer.encode(text, add_special_tokens=add_special_tokens)]


def decode_tokens(tokens):
    if tokenizer is None:
        raise RuntimeError("model has not been loaded")
    try:
        return tokenizer.decode([int(token) for token in tokens])
    except TypeError:
        return tokenizer.decode([int(token) for token in tokens], skip_special_tokens=False)


def vocab_size():
    value = getattr(tokenizer, "vocab_size", None)
    if isinstance(value, int):
        return value
    try:
        return len(tokenizer)
    except TypeError:
        return 0


def effective_top_k(value):
    value = int(value)
    n_vocab = vocab_size()
    if value > 0 and n_vocab > 0:
        # Asking for more candidates than exist is exactly equivalent to using
        # the complete vocabulary; MLX itself rejects the larger integer.
        return min(value, n_vocab)
    return value


def snapshot_mlx_random_state():
    import mlx.core as mx

    state = [mx.array(value) for value in mx.random.state]
    mx.eval(*state)
    return state


def restore_mlx_random_state(state):
    import mlx.core as mx

    mx.random.state[:] = [mx.array(value) for value in state]
    mx.eval(*mx.random.state)


def install_qwen35_mixed_visual_support(loaded_model):
    module = type(loaded_model).__module__
    if not module.startswith(
        ("mlx_vlm.models.qwen3_5.", "mlx_vlm.models.qwen3_5_moe.")
    ):
        return

    original = loaded_model.get_input_embeddings

    def mixed_visual_embeddings(self, input_ids=None, pixel_values=None, **kwargs):
        video_values = kwargs.get("pixel_values_videos")
        if pixel_values is None or video_values is None:
            return original(
                input_ids=input_ids,
                pixel_values=pixel_values,
                **kwargs,
            )

        import mlx.core as mx
        from mlx_vlm.models.base import InputEmbeddingsFeatures
        from mlx_vlm.models.qwen3_vl.qwen3_vl import masked_scatter

        image_grid = kwargs.get("image_grid_thw")
        video_grid = kwargs.get("video_grid_thw")
        mask = kwargs.get("mask")
        dtype = self.vision_tower.patch_embed.proj.weight.dtype
        image_features, _ = self.vision_tower(pixel_values.astype(dtype), image_grid)
        video_features, _ = self.vision_tower(video_values.astype(dtype), video_grid)
        embeddings = self.language_model.model.embed_tokens(input_ids)

        for token_id, features, label in (
            (self.config.image_token_index, image_features, "image"),
            (self.config.video_token_index, video_features, "video"),
        ):
            token_mask = input_ids == token_id
            token_count = token_mask.sum()
            expanded_mask = mx.broadcast_to(token_mask[..., None], embeddings.shape)
            if expanded_mask.sum() != features.size:
                raise ValueError(
                    f"{label} features and tokens do not match: "
                    f"tokens {token_count}, features {features.shape[0]}"
                )
            embeddings = masked_scatter(embeddings, expanded_mask, features)

        position_ids, rope_deltas = self.language_model.get_rope_index(
            input_ids,
            image_grid,
            video_grid,
            mask,
        )
        self.language_model._position_ids = position_ids
        self.language_model._rope_deltas = rope_deltas
        return InputEmbeddingsFeatures(inputs_embeds=embeddings)

    loaded_model.get_input_embeddings = types.MethodType(
        mixed_visual_embeddings,
        loaded_model,
    )


def model_ctx(default):
    candidates = [
        getattr(model, "args", None),
        getattr(model, "config", None),
        getattr(getattr(model, "language_model", None), "args", None),
        getattr(getattr(model, "language_model", None), "config", None),
    ]
    for candidate in candidates:
        for name in (
            "max_position_embeddings",
            "max_sequence_length",
            "max_seq_len",
            "context_length",
            "model_max_length",
        ):
            value = getattr(candidate, name, None)
            if isinstance(candidate, dict):
                value = candidate.get(name)
            if isinstance(value, int) and value > 0:
                return value
    return int(default)


def inline_media_bytes(item):
    data = item.get("data")
    if data:
        try:
            return base64.b64decode(str(data), validate=True)
        except Exception as exc:
            raise ValueError("multimodal data is not valid base64") from exc
    url = item.get("url")
    if not url:
        raise ValueError("multimodal input is missing inline data")
    url = str(url)
    if not url.startswith("data:"):
        raise ValueError("remote media URLs are forbidden; use inline base64 data")
    header, separator, encoded = url.partition(",")
    if not separator or ";base64" not in header:
        raise ValueError("multimodal data URL must be base64 encoded")
    try:
        return base64.b64decode(encoded, validate=True)
    except Exception as exc:
        raise ValueError("multimodal data URL contains invalid base64") from exc


def decode_image(item):
    from PIL import Image

    image = Image.open(io.BytesIO(inline_media_bytes(item)))
    image.load()
    return image.convert("RGB")


def decode_audio(item):
    import numpy as np

    try:
        with wave.open(io.BytesIO(inline_media_bytes(item)), "rb") as wav:
            channels = int(wav.getnchannels())
            sample_width = int(wav.getsampwidth())
            sample_rate = int(wav.getframerate())
            frames = wav.readframes(wav.getnframes())
    except Exception as exc:
        raise ValueError("MLX multimodal audio must be a bounded PCM WAV") from exc
    dtype = {1: np.uint8, 2: np.int16, 4: np.int32}.get(sample_width)
    if dtype is None or channels <= 0 or sample_rate <= 0:
        raise ValueError("MLX multimodal WAV sample format is unsupported")
    samples = np.frombuffer(frames, dtype=dtype).astype(np.float32)
    if sample_width == 1:
        samples = (samples - 128.0) / 128.0
    else:
        samples /= float(1 << (sample_width * 8 - 1))
    if channels > 1:
        if samples.size % channels:
            raise ValueError("MLX multimodal WAV channel data is malformed")
        samples = samples.reshape(-1, channels).mean(axis=1)
    return samples


def decode_video(item):
    import av
    import numpy as np

    requested = int(item.get("num_frames") or 8)
    if requested <= 0 or requested > 64:
        raise ValueError("MLX multimodal video num_frames must be between 1 and 64")
    inline_frames = item.get("frames") or []
    if inline_frames:
        if item.get("data") or item.get("url"):
            raise ValueError("MLX video must use decoded frames or a container, not both")
        if len(inline_frames) != requested:
            raise ValueError("MLX video num_frames does not match decoded frames")
        decoded = [
            np.asarray(decode_image({"url": frame}), dtype=np.uint8)
            for frame in inline_frames
        ]
        shape = decoded[0].shape
        if any(frame.shape != shape for frame in decoded):
            raise ValueError("MLX video decoded frames must have matching dimensions")
        fps = float(item.get("fps") or 1.0)
        if not math.isfinite(fps) or fps <= 0:
            raise ValueError("MLX video fps must be a positive finite number")
        return np.transpose(np.stack(decoded), (0, 3, 1, 2)), fps

    try:
        container = av.open(io.BytesIO(inline_media_bytes(item)))
        stream = container.streams.video[0]
    except Exception as exc:
        raise ValueError("MLX video container cannot be decoded") from exc
    total = int(getattr(stream, "frames", 0) or 0)
    average_rate = getattr(stream, "average_rate", None)
    fps = float(average_rate) if average_rate is not None else 0.0
    if fps <= 0:
        fps = float(item.get("fps") or 1.0)
    wanted = (
        set(np.linspace(0, total - 1, min(requested, total), dtype=int).tolist())
        if total > 0
        else None
    )
    frames = []
    try:
        for index, frame in enumerate(container.decode(stream)):
            if wanted is None:
                if len(frames) >= requested:
                    break
                frames.append(frame.to_ndarray(format="rgb24"))
            elif index in wanted:
                frames.append(frame.to_ndarray(format="rgb24"))
    finally:
        container.close()
    if not frames:
        raise ValueError("MLX video container contains no decodable frames")
    return np.transpose(np.stack(frames), (0, 3, 1, 2)), fps


def request_media(payload):
    images = []
    audios = []
    videos = []
    video_fps = None
    for item in payload.get("media") or []:
        kind = str(item.get("kind") or "")
        if kind == "image":
            images.append(decode_image(item))
        elif kind == "audio":
            audios.append(decode_audio(item))
        elif kind == "video":
            video, fps = decode_video(item)
            videos.append(video)
            video_fps = fps if video_fps is None else video_fps
        else:
            raise ValueError(f"unsupported MLX multimodal input kind {kind!r}")
    return images, audios, videos, video_fps


def handle_load(payload):
    global model, tokenizer, processor, multimodal, ctx_size
    global kv_cache_bits, kv_cache_group_size, kv_cache_quantized_start_tokens

    path = str(payload["path"])
    ctx_size = int(payload.get("ctx_size") or 2048)
    multimodal = bool(payload.get("multimodal", False))
    kv_cache_bits = payload.get("kv_cache_bits")
    if kv_cache_bits is not None:
        kv_cache_bits = int(kv_cache_bits)
        if kv_cache_bits not in (4, 8):
            raise ValueError("MLX KV-cache bits must be 4 or 8")
    kv_cache_group_size = int(payload.get("kv_cache_group_size") or 64)
    if kv_cache_group_size <= 0:
        raise ValueError("MLX KV-cache group size must be positive")
    kv_cache_quantized_start_tokens = int(
        payload.get("kv_cache_quantized_start_tokens") or 0
    )
    if kv_cache_quantized_start_tokens < 0:
        raise ValueError("MLX KV-cache quantized start must not be negative")
    if multimodal:
        from mlx_vlm.utils import load as load_vlm

        model, processor = load_vlm(path)
        install_qwen35_mixed_visual_support(model)
        tokenizer = getattr(processor, "tokenizer", processor)
    else:
        from mlx_lm import load as load_lm

        model, tokenizer = load_lm(path)
        processor = None
    return {
        "n_ctx_train": model_ctx(ctx_size),
        "n_vocab": int(vocab_size()),
    }


def handle_tokenize(payload):
    return {"token_ids": encode_text(str(payload.get("text", "")))}


def handle_generate(request_id, payload):
    check_cancelled(request_id)
    if model is None or tokenizer is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = int(payload.get("max_new_tokens") or 64)
    template_kwargs, sampling_kwargs, prompt_suffixes = speciality_maps(payload)
    media = payload.get("media") or []
    if media and not multimodal:
        raise ValueError("MLX text runtime cannot accept multimodal input")
    images, audios, videos, video_fps = (
        request_media(payload) if multimodal else ([], [], [], None)
    )
    prompt = request_prompt(
        payload,
        template_kwargs,
        prompt_suffixes,
        num_images=len(images),
        num_audios=len(audios),
    )
    prompt_tokens = encode_text(prompt)
    if len(prompt_tokens) >= ctx_size:
        raise ValueError(
            f"prompt has {len(prompt_tokens)} tokens, leaving no room in ctx_size={ctx_size}"
        )
    if max_tokens <= 0:
        return {
            "text": "",
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            "finish_reason": "length",
        }

    import mlx.core as mx
    from mlx_lm import stream_generate
    from mlx_lm.sample_utils import make_logits_processors, make_sampler

    seed = payload.get("seed")
    if seed is not None:
        mx.random.seed(int(seed))

    temperature = float(payload.get("temperature") or 0.0)
    top_p = float(payload.get("top_p") or 0.0)
    sampler_kwargs = {"temp": temperature, "top_p": top_p}
    requested = set()
    if payload.get("top_k") is not None:
        sampler_kwargs["top_k"] = effective_top_k(payload.get("top_k"))
        requested.add("top_k")
    if payload.get("min_p") is not None:
        sampler_kwargs["min_p"] = float(payload.get("min_p"))
        requested.add("min_p")
    for name, value in sampling_kwargs.items():
        if name in sampler_kwargs:
            raise ValueError(
                f"MLX speciality sampling parameter {name!r} conflicts with a standard request field"
            )
        sampler_kwargs[name] = value
        requested.add(name)
    sampler = make_sampler(
        **required_sampling_kwargs(make_sampler, sampler_kwargs, requested)
    )
    repeat_penalty = payload.get("repeat_penalty")
    logits_processors = list(make_logits_processors(
        repetition_penalty=(
            None if repeat_penalty is None else float(repeat_penalty)
        ),
        presence_penalty=(
            None
            if payload.get("presence_penalty") is None
            else float(payload.get("presence_penalty"))
        ),
        frequency_penalty=(
            None
            if payload.get("frequency_penalty") is None
            else float(payload.get("frequency_penalty"))
        ),
    ) or [])
    constrained = structured_logits_processor(payload)
    if constrained is not None:
        logits_processors.append(constrained)

    finish_reason = "length"
    reasoning_tokens = 0
    reasoning_active = reasoning_enabled(payload)
    generation_events = MlxGenerationEvents(request_id, payload.get("stop") or [])
    generation_kwargs = {
        "max_tokens": max_tokens,
        "sampler": sampler,
        "logits_processors": logits_processors,
        "max_kv_size": ctx_size,
        "kv_bits": kv_cache_bits,
        "kv_group_size": kv_cache_group_size,
        "quantized_kv_start": kv_cache_quantized_start_tokens,
    }
    thinking_budget = reasoning_budget(payload)
    if thinking_budget is not None:
        if not multimodal:
            raise ValueError(
                "MLX text runtime cannot enforce a calibrated reasoning-token budget"
            )
        generation_kwargs["thinking_budget"] = thinking_budget
        generation_kwargs["enable_thinking"] = reasoning_enabled(payload)
    if multimodal:
        from mlx_vlm.generate import stream_generate as stream_vlm_generate

        responses = stream_vlm_generate(
            model,
            processor,
            prompt,
            image=images or None,
            audio=audios or None,
            video=videos or None,
            fps=video_fps or 1.0,
            **generation_kwargs,
        )
    else:
        responses = stream_generate(model, tokenizer, prompt, **generation_kwargs)

    actual_prompt_tokens = len(prompt_tokens)
    for response in responses:
        check_cancelled(request_id)
        segment = str(getattr(response, "text", ""))
        new_token, stopped, final_reason_value = generation_events.push(response)
        if new_token and reasoning_active:
            reasoning_tokens += 1
        if segment:
            if reasoning_active and "</think>" in segment:
                reasoning_active = False
        actual_prompt_tokens = max(
            actual_prompt_tokens,
            int(getattr(response, "prompt_tokens", actual_prompt_tokens)),
        )
        if stopped:
            finish_reason = "stop"
            break
        if final_reason_value is not None:
            finish_reason = str(final_reason_value)
            break

    check_cancelled(request_id)
    generation_events.finish()
    completion_tokens = generation_events.completion_tokens

    return {
        "text": generation_events.stream.output,
        "usage": {
            "prompt_tokens": actual_prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": actual_prompt_tokens + completion_tokens,
            "reasoning_tokens": min(
                reasoning_tokens,
                completion_tokens,
                thinking_budget if thinking_budget is not None else completion_tokens,
            ),
            "vision_tokens": (
                max(0, actual_prompt_tokens - len(prompt_tokens))
                if images or videos
                else 0
            ),
            "audio_tokens": (
                max(0, actual_prompt_tokens - len(prompt_tokens))
                if audios and not images and not videos
                else 0
            ),
        },
        "finish_reason": normalize_finish_reason(finish_reason),
    }


def normalize_finish_reason(reason):
    if reason == "stop":
        return "stop"
    return "length"


def handle(request_id, op, payload):
    if op == "load":
        return handle_load(payload or {})
    if op == "tokenize":
        return handle_tokenize(payload or {})
    if op == "generate":
        return handle_generate(request_id, payload or {})
    if op == "shutdown":
        raise SystemExit(0)
    raise ValueError(f"unknown MLX worker op {op!r}")


def read_requests():
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
        except Exception as exc:
            request_queue.put({"id": 0, "op": "invalid", "parse_error": str(exc)})
            continue
        request_id = int(request.get("id", 0))
        if str(request.get("op", "")) == "cancel":
            payload = request.get("payload") or {}
            mark_cancelled(int(payload.get("request_id", request_id)))
            continue
        request_queue.put(request)
    request_queue.put(None)


threading.Thread(target=read_requests, name="mayhem-mlx-control", daemon=True).start()

while True:
    request = request_queue.get()
    if request is None:
        break
    request_id = int(request.get("id", 0))
    random_state = None
    try:
        if "parse_error" in request:
            raise ValueError(request["parse_error"])
        op = str(request.get("op", ""))
        if op == "generate":
            random_state = snapshot_mlx_random_state()
        result = handle(request_id, op, request.get("payload"))
        check_cancelled(request_id)
        send({"id": request_id, "type": "response", "ok": True, "result": result})
    except SystemExit:
        raise
    except RequestCancelled as exc:
        error = str(exc)
        if random_state is not None:
            try:
                restore_mlx_random_state(random_state)
            except Exception as recovery_exc:
                error += f"; MLX random-state recovery failed: {recovery_exc}"
        send(
            {
                "id": request_id,
                "type": "response",
                "ok": False,
                "cancelled": True,
                "error": error,
            }
        )
    except Exception as exc:
        error = str(exc)
        if random_state is not None:
            try:
                restore_mlx_random_state(random_state)
            except Exception as recovery_exc:
                error += f"; MLX random-state recovery failed: {recovery_exc}"
        send({"id": request_id, "type": "response", "ok": False, "error": error})
    finally:
        finish_request(request_id)
