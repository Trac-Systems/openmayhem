import asyncio
import base64
import copy
import inspect
import io
import json
import os
import sys
import wave


protocol_stdout = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
sys.stdout = sys.stderr


engine = None
tokenizer = None
processor = None
ctx_size = 2048
batch_invariant = False
kernel_policy = "auto"
event_loop = asyncio.new_event_loop()
asyncio.set_event_loop(event_loop)


def send(message):
    protocol_stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    protocol_stdout.flush()


def accepted_kwargs(callable_obj, kwargs):
    try:
        signature = inspect.signature(callable_obj)
    except (TypeError, ValueError):
        return kwargs
    parameters = signature.parameters
    if any(param.kind == inspect.Parameter.VAR_KEYWORD for param in parameters.values()):
        return kwargs
    return {key: value for key, value in kwargs.items() if key in parameters}


def required_sampling_kwargs(callable_obj, kwargs, requested):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in requested if name not in accepted)
    if missing:
        raise ValueError(
            "vLLM backend does not support requested sampling parameter(s): "
            + ", ".join(missing)
        )
    return accepted


def required_engine_kwargs(callable_obj, kwargs, required):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in required if name not in accepted)
    if missing:
        raise ValueError(
            "vLLM backend lacks required deterministic engine option(s): "
            + ", ".join(missing)
        )
    return accepted


def model_uses_hybrid_attention(path):
    from transformers import AutoConfig

    config = AutoConfig.from_pretrained(path, trust_remote_code=False)
    configs = [config, getattr(config, "text_config", None)]
    hybrid_markers = ("linear", "mamba", "ssm", "gdn", "recurrent", "rwkv")
    for candidate in configs:
        if candidate is None:
            continue
        layer_types = getattr(candidate, "layer_types", None) or []
        if any(
            any(marker in str(layer_type).lower() for marker in hybrid_markers)
            for layer_type in layer_types
        ):
            return True
        model_type = str(getattr(candidate, "model_type", "")).lower()
        if any(marker in model_type for marker in ("mamba", "jamba", "rwkv")):
            return True
    return False


def model_uses_nvfp4(path):
    from transformers import AutoConfig

    config = AutoConfig.from_pretrained(path, trust_remote_code=False)
    quantization = getattr(config, "quantization_config", None) or {}
    return "nvfp4" in json.dumps(quantization, sort_keys=True).lower()


def configure_deterministic_runtime(path):
    global batch_invariant

    # Seeded canaries must survive batching and scheduler changes. vLLM's
    # batch-invariant kernels require NVIDIA compute capability 9.0 or newer,
    # and vLLM currently refuses them for hybrid GDN/Mamba attention stacks.
    required_environment = {
        "VLLM_ENABLE_V1_MULTIPROCESSING": "0",
        "PYTHONHASHSEED": "0",
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
    }
    for name, expected in required_environment.items():
        if os.environ.get(name) != expected:
            raise RuntimeError(
                f"deterministic vLLM worker requires {name}={expected} before Python starts"
            )
    import torch

    cuda_supported = bool(torch.version.cuda) and torch.cuda.is_available()
    capability = torch.cuda.get_device_capability() if cuda_supported else (0, 0)
    batch_invariant = (
        cuda_supported and capability >= (9, 0) and not model_uses_hybrid_attention(path)
    )
    os.environ["VLLM_BATCH_INVARIANT"] = "1" if batch_invariant else "0"
    return batch_invariant


def speciality_maps(payload):
    template_kwargs = {}
    sampling_kwargs = {}
    prompt_suffixes = []
    seen_names = set()
    for item in payload.get("speciality_parameters") or []:
        if not isinstance(item, dict):
            raise ValueError("vLLM speciality parameter must be an object")
        name = str(item.get("name") or "")
        target = str(item.get("target") or "")
        native_path = str(item.get("native_path") or "")
        if not name or name in seen_names or not native_path:
            raise ValueError("vLLM speciality parameters require unique names and native paths")
        seen_names.add(name)
        value = item.get("value")
        if target == "chat_template_kwarg":
            key = native_kwarg_name(native_path, "chat_template_kwargs.")
            if key in template_kwargs:
                raise ValueError(f"duplicate vLLM chat-template speciality mapping {key!r}")
            template_kwargs[key] = value
        elif target == "sampling_parameter":
            key = native_kwarg_name(native_path, "sampling_params.")
            if key in sampling_kwargs:
                raise ValueError(f"duplicate vLLM sampling speciality mapping {key!r}")
            sampling_kwargs[key] = value
        elif target == "prompt_suffix":
            if not isinstance(value, str):
                raise ValueError(f"vLLM prompt-suffix speciality {name!r} must map to a string")
            prompt_suffixes.append(value)
        else:
            raise ValueError(f"unsupported vLLM speciality target {target!r}")
    return template_kwargs, sampling_kwargs, prompt_suffixes


def native_kwarg_name(native_path, prefix):
    key = str(native_path)
    if key.startswith(prefix):
        key = key[len(prefix) :]
    if not key or not key.isidentifier():
        raise ValueError(f"native speciality path {native_path!r} is not a callable keyword")
    return key


def required_call_kwargs(callable_obj, kwargs, required, label):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in required if name not in accepted)
    if missing:
        raise ValueError(
            f"vLLM {label} does not support requested speciality parameter(s): "
            + ", ".join(missing)
        )
    return accepted


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


def import_attr(candidates):
    last_error = None
    for module_name, attr_name in candidates:
        try:
            module = __import__(module_name, fromlist=[attr_name])
            return getattr(module, attr_name)
        except Exception as exc:
            last_error = exc
    raise RuntimeError(f"could not import vLLM API: {last_error}")


def encode_text(text):
    if tokenizer is None:
        return []
    try:
        return [int(token) for token in tokenizer.encode(text, add_special_tokens=True)]
    except TypeError:
        return [int(token) for token in tokenizer.encode(text)]


def vocab_size():
    value = getattr(tokenizer, "vocab_size", None)
    if isinstance(value, int):
        return value
    try:
        return len(tokenizer)
    except TypeError:
        return 0


def model_ctx(default):
    for source in (tokenizer, getattr(engine, "model_config", None), getattr(engine, "config", None)):
        if source is None:
            continue
        for name in (
            "max_position_embeddings",
            "max_sequence_length",
            "max_seq_len",
            "context_length",
            "model_max_length",
        ):
            value = getattr(source, name, None)
            if isinstance(value, int) and 0 < value < 1_000_000_000:
                return value
    return int(default)


def positive_int(value, default):
    try:
        parsed = int(value)
        return parsed if parsed > 0 else int(default)
    except Exception:
        return int(default)


def utilization_float(value):
    if value is None:
        return None
    try:
        parsed = float(value)
    except Exception as exc:
        raise ValueError("gpu_memory_utilization must be a number between 0 and 1") from exc
    if parsed <= 0.0 or parsed > 1.0:
        raise ValueError("gpu_memory_utilization must be between 0 and 1")
    return parsed


def make_structured_outputs_params(grammar):
    if grammar is None:
        return None
    try:
        StructuredOutputsParams = import_attr(
            (
                ("vllm.sampling_params", "StructuredOutputsParams"),
                ("vllm", "StructuredOutputsParams"),
            )
        )
    except Exception:
        GuidedDecodingParams = import_attr(
            (
                ("vllm.sampling_params", "GuidedDecodingParams"),
                ("vllm", "GuidedDecodingParams"),
            )
        )
        return make_guided_params(GuidedDecodingParams, grammar)
    return make_guided_params(StructuredOutputsParams, grammar)


def make_guided_params(cls, grammar):
    kind = grammar.get("kind")
    if kind == "json_schema":
        return cls(**accepted_kwargs(cls, {"json": grammar.get("schema") or {}}))
    if kind == "gbnf":
        return cls(**accepted_kwargs(cls, {"grammar": grammar.get("grammar") or ""}))
    if kind == "tool_call":
        return cls(**accepted_kwargs(cls, {"json": tool_call_schema(grammar.get("tools") or [])}))
    raise ValueError(f"unsupported vLLM grammar kind {kind!r}")


def tool_call_schema(tools):
    names = [str(tool.get("name", "")) for tool in tools if str(tool.get("name", ""))]
    if not names:
        raise ValueError("tool-call grammar requires at least one tool")
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MayhemToolCall",
        "type": "object",
        "additionalProperties": False,
        "required": ["tool", "arguments"],
        "properties": {
            "tool": {"type": "string", "enum": names},
            "arguments": {"type": "object"},
        },
    }


def make_sampling_params(payload, speciality_sampling_kwargs=None):
    SamplingParams = import_attr((("vllm", "SamplingParams"),))
    kwargs = {
        "max_tokens": int(payload.get("max_new_tokens") or 64),
        "temperature": float(payload.get("temperature") or 0.0),
    }
    top_p = payload.get("top_p")
    if top_p is not None and float(top_p) > 0.0:
        kwargs["top_p"] = float(top_p)
    requested = set()
    top_k = payload.get("top_k")
    if top_k is not None:
        kwargs["top_k"] = int(top_k)
        requested.add("top_k")
    min_p = payload.get("min_p")
    if min_p is not None:
        kwargs["min_p"] = float(min_p)
        requested.add("min_p")
    repeat_penalty = payload.get("repeat_penalty")
    if repeat_penalty is not None:
        kwargs["repetition_penalty"] = float(repeat_penalty)
        requested.add("repetition_penalty")
    frequency_penalty = payload.get("frequency_penalty")
    if frequency_penalty is not None:
        kwargs["frequency_penalty"] = float(frequency_penalty)
        requested.add("frequency_penalty")
    presence_penalty = payload.get("presence_penalty")
    if presence_penalty is not None:
        kwargs["presence_penalty"] = float(presence_penalty)
        requested.add("presence_penalty")
    stop = payload.get("stop") or []
    if stop:
        kwargs["stop"] = [str(value) for value in stop]
        requested.add("stop")
    if payload.get("seed") is not None:
        kwargs["seed"] = int(payload.get("seed"))
    if bool(payload.get("ignore_eos")):
        kwargs["ignore_eos"] = True
        kwargs["min_tokens"] = kwargs["max_tokens"]
    for name, value in (speciality_sampling_kwargs or {}).items():
        if name in kwargs:
            raise ValueError(
                f"vLLM speciality sampling parameter {name!r} conflicts with a standard request field"
            )
        kwargs[name] = value
        requested.add(name)
    structured = make_structured_outputs_params(payload.get("grammar"))
    if structured is not None:
        kwargs["structured_outputs"] = structured
        kwargs["guided_decoding"] = structured
    try:
        RequestOutputKind = import_attr((("vllm.sampling_params", "RequestOutputKind"),))
        kwargs["output_kind"] = getattr(RequestOutputKind, "DELTA")
    except Exception:
        pass
    return SamplingParams(**required_sampling_kwargs(SamplingParams, kwargs, requested))


def create_engine(payload):
    global kernel_policy

    path = str(payload["path"])
    configure_deterministic_runtime(path)
    AsyncEngineArgs = import_attr(
        (
            ("vllm.engine.arg_utils", "AsyncEngineArgs"),
            ("vllm", "AsyncEngineArgs"),
        )
    )
    AsyncLLM = import_attr(
        (
            ("vllm.v1.engine.async_llm", "AsyncLLM"),
            ("vllm", "AsyncLLM"),
        )
    )
    tensor_parallel = positive_int(payload.get("tensor_parallel"), 1)
    kwargs = {
        "model": path,
        "tokenizer": path,
        "trust_remote_code": False,
        "max_model_len": positive_int(payload.get("ctx_size"), 2048),
        "max_num_seqs": positive_int(payload.get("max_batch_size"), 1),
        "max_num_batched_tokens": positive_int(
            payload.get("max_num_tokens"), max(256, positive_int(payload.get("ctx_size"), 2048))
        ),
        "tensor_parallel_size": tensor_parallel,
        "enforce_eager": bool(payload.get("enforce_eager", True)),
        "seed": 0,
        "use_fp64_gumbel": True,
        "async_scheduling": False,
        "limit_mm_per_prompt": {"image": 1, "audio": 1, "video": 1},
        "mm_processor_cache_gb": 0,
    }
    required_options = {"seed", "use_fp64_gumbel", "async_scheduling"}
    if model_uses_nvfp4(path):
        # vLLM's batch-invariant path selects CUTLASS for deterministic NVFP4
        # execution. Hybrid GDN models cannot enable that global mode, so pin
        # the same kernel family explicitly instead of using auto-selected
        # FlashInfer linear/MoE kernels.
        kwargs["linear_backend"] = "cutlass"
        kwargs["moe_backend"] = "cutlass"
        required_options.update(("linear_backend", "moe_backend"))
        kernel_policy = "nvfp4-cutlass"
    else:
        kernel_policy = "auto"
    dtype = payload.get("dtype")
    if dtype:
        kwargs["dtype"] = str(dtype)
    gpu_memory_utilization = utilization_float(payload.get("gpu_memory_utilization"))
    if gpu_memory_utilization is not None:
        kwargs["gpu_memory_utilization"] = gpu_memory_utilization
    args = AsyncEngineArgs(
        **required_engine_kwargs(
            AsyncEngineArgs,
            kwargs,
            required_options,
        )
    )
    if hasattr(AsyncLLM, "from_engine_args"):
        return AsyncLLM.from_engine_args(args)
    return AsyncLLM(args)


def get_tokenizer():
    global tokenizer
    if tokenizer is not None:
        return tokenizer
    if hasattr(engine, "get_tokenizer"):
        tokenizer = engine.get_tokenizer()
        return tokenizer
    try:
        from transformers import AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=False)
        return tokenizer
    except Exception:
        tokenizer = None
        return None


def request_max_tokens(payload):
    value = payload.get("max_new_tokens")
    return 64 if value is None else int(value)


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
        raise ValueError("multimodal audio must be a bounded PCM WAV") from exc
    dtype_by_width = {1: np.uint8, 2: np.int16, 4: np.int32}
    dtype = dtype_by_width.get(sample_width)
    if dtype is None or channels <= 0 or sample_rate <= 0:
        raise ValueError("multimodal WAV sample format is unsupported")
    samples = np.frombuffer(frames, dtype=dtype).astype(np.float32)
    if sample_width == 1:
        samples = (samples - 128.0) / 128.0
    else:
        samples /= float(1 << (sample_width * 8 - 1))
    if channels > 1:
        if samples.size % channels:
            raise ValueError("multimodal WAV channel data is malformed")
        samples = samples.reshape(-1, channels).mean(axis=1)
    return samples, sample_rate


def decode_video(item):
    import av
    import numpy as np

    requested = int(item.get("num_frames") or 8)
    if requested <= 0 or requested > 1024:
        raise ValueError("multimodal video num_frames must be between 1 and 1024")
    try:
        container = av.open(io.BytesIO(inline_media_bytes(item)))
        stream = container.streams.video[0]
    except Exception as exc:
        raise ValueError("multimodal video container cannot be decoded") from exc
    total = int(getattr(stream, "frames", 0) or 0)
    average_rate = getattr(stream, "average_rate", None)
    source_fps = float(average_rate) if average_rate is not None else 0.0
    if source_fps <= 0:
        source_fps = float(item.get("fps") or 1.0)
    stream_duration = getattr(stream, "duration", None)
    time_base = getattr(stream, "time_base", None)
    duration = (
        float(stream_duration * time_base)
        if stream_duration is not None and time_base is not None
        else 0.0
    )
    wanted = None
    if total > 0:
        wanted = set(np.linspace(0, total - 1, min(requested, total), dtype=int).tolist())
    frames = []
    frame_indices = []
    try:
        for index, frame in enumerate(container.decode(stream)):
            if wanted is None:
                if len(frames) < requested:
                    frames.append(frame.to_ndarray(format="rgb24"))
                    frame_indices.append(index)
                else:
                    break
            elif index in wanted:
                frames.append(frame.to_ndarray(format="rgb24"))
                frame_indices.append(index)
                if len(frames) == len(wanted):
                    break
    finally:
        container.close()
    if not frames:
        raise ValueError("multimodal video contains no decodable frames")
    total_num_frames = max(total, frame_indices[-1] + 1, len(frames))
    duration = max(duration, total_num_frames / source_fps)
    metadata = {
        "fps": source_fps,
        "duration": duration,
        "total_num_frames": total_num_frames,
        "frames_indices": frame_indices,
        "video_backend": "pyav",
        "do_sample_frames": False,
    }
    return np.stack(frames), metadata


def decode_multimodal_data(payload):
    grouped = {}
    for item in payload.get("media") or []:
        kind = str(item.get("kind") or "")
        if kind == "image":
            value = decode_image(item)
        elif kind == "audio":
            value = decode_audio(item)
        elif kind == "video":
            value = decode_video(item)
        else:
            raise ValueError(f"unsupported multimodal input kind {kind!r}")
        grouped.setdefault(kind, []).append(value)
    return {
        kind: values[0] if len(values) == 1 else values
        for kind, values in grouped.items()
    }


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


def render_chat_template(renderer, messages, template_kwargs, label):
    if renderer is None or not hasattr(renderer, "apply_chat_template"):
        raise RuntimeError(f"requested speciality requires the model {label}/chat template")
    kwargs = {
        "tokenize": False,
        "add_generation_prompt": True,
        **template_kwargs,
    }
    try:
        return str(
            renderer.apply_chat_template(
                messages,
                **required_call_kwargs(
                    renderer.apply_chat_template,
                    kwargs,
                    set(template_kwargs),
                    f"{label} chat template",
                ),
            )
        )
    except Exception as exc:
        raise ValueError(f"model {label} chat template failed: {exc}") from exc


def multimodal_engine_prompt(payload, mm_data, template_kwargs, prompt_suffixes):
    prompt = str(payload.get("prompt", ""))
    messages = payload.get("messages") or []
    if mm_data:
        if not messages:
            raise ValueError("multimodal request is missing structured chat messages")
        rendered = render_chat_template(
            processor,
            processor_chat_messages(messages),
            template_kwargs,
            "multimodal processor",
        )
        rendered += "".join(prompt_suffixes)
        return rendered, {"prompt": rendered, "multi_modal_data": mm_data}
    if template_kwargs:
        if not messages:
            raise ValueError("chat-template request is missing structured chat messages")
        prompt = render_chat_template(tokenizer, messages, template_kwargs, "tokenizer")
    prompt += "".join(prompt_suffixes)
    return prompt, prompt


async def async_handle_generate(request_id, payload):
    if engine is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = request_max_tokens(payload)
    template_kwargs, sampling_kwargs, prompt_suffixes = speciality_maps(payload)
    template_tools = payload.get("tools") or []
    if not isinstance(template_tools, list):
        raise ValueError("vLLM chat-template tools must be an array")
    if template_tools:
        if "tools" in template_kwargs:
            raise ValueError("vLLM chat-template tools conflict with a speciality mapping")
        template_kwargs["tools"] = template_tools
    mm_data = decode_multimodal_data(payload)
    prompt, engine_prompt = multimodal_engine_prompt(
        payload, mm_data, template_kwargs, prompt_suffixes
    )
    prompt_tokens = encode_text(prompt)
    if prompt_tokens and len(prompt_tokens) >= ctx_size:
        raise ValueError(
            f"prompt has {len(prompt_tokens)} tokens, leaving no room in ctx_size={ctx_size}"
        )
    if max_tokens <= 0:
        return {
            "text": "",
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            "finish_reason": "length",
        }

    sampling_params = make_sampling_params(payload, sampling_kwargs)
    text = ""
    completion_tokens = 0
    token_ids = []
    finish_reason = "length"
    reasoning_tokens = 0
    reasoning_active = reasoning_enabled(payload)
    actual_prompt_tokens = len(prompt_tokens)
    async for output in engine.generate(
        request_id=f"mayhem-{request_id}", prompt=engine_prompt, sampling_params=sampling_params
    ):
        output_prompt_ids = getattr(output, "prompt_token_ids", None)
        if output_prompt_ids is not None:
            actual_prompt_tokens = max(actual_prompt_tokens, len(output_prompt_ids))
        for completion in getattr(output, "outputs", []) or []:
            chunk_text = str(getattr(completion, "text", "") or "")
            ids = getattr(completion, "token_ids", None) or []
            ids = [int(token) for token in ids]
            if chunk_text or ids:
                if not ids:
                    ids = [-1]
                for token in ids:
                    send(
                        {
                            "id": request_id,
                            "type": "token",
                            "chunk": {
                                "index": completion_tokens,
                                "token_id": int(token),
                                "text": chunk_text if token == ids[0] else "",
                            },
                        }
                    )
                    completion_tokens += 1
                    token_ids.append(int(token))
                    if reasoning_active:
                        reasoning_tokens += 1
                text += chunk_text
                if reasoning_active and "</think>" in text:
                    reasoning_active = False
            reason = getattr(completion, "finish_reason", None)
            if reason is not None:
                finish_reason = "stop" if str(reason) == "stop" else "length"
        if getattr(output, "finished", False):
            break

    if completion_tokens == 0 and text:
        completion_tokens = 1
    return {
        "text": text,
        "usage": {
            "prompt_tokens": actual_prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": actual_prompt_tokens + completion_tokens,
            "reasoning_tokens": min(reasoning_tokens, completion_tokens),
            "vision_tokens": max(0, actual_prompt_tokens - len(prompt_tokens)) if mm_data else 0,
        },
        "finish_reason": finish_reason,
    }


def handle_load(payload):
    global engine, tokenizer, processor, ctx_size, model_path
    model_path = str(payload["path"])
    ctx_size = positive_int(payload.get("ctx_size"), 2048)
    engine = create_engine(payload)
    try:
        from transformers import AutoProcessor, AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=False)
        try:
            processor = AutoProcessor.from_pretrained(model_path, trust_remote_code=False)
        except Exception:
            processor = None
    except Exception:
        tokenizer = None
        processor = None
    get_tokenizer()
    return {
        "n_ctx_train": model_ctx(ctx_size),
        "n_vocab": int(vocab_size()),
        "determinism": {
            "async_scheduling": False,
            "batch_invariant": batch_invariant,
            "v1_multiprocessing": False,
            "python_hash_seed": 0,
            "fp64_gumbel": True,
            "kernel_policy": kernel_policy,
            "seed": 0,
        },
    }


def handle_tokenize(payload):
    if engine is None:
        raise RuntimeError("model has not been loaded")
    return {"token_ids": encode_text(str(payload.get("text", "")))}


def handle(request_id, op, payload):
    if op == "load":
        return handle_load(payload or {})
    if op == "tokenize":
        return handle_tokenize(payload or {})
    if op == "generate":
        return event_loop.run_until_complete(async_handle_generate(request_id, payload or {}))
    if op == "shutdown":
        raise SystemExit(0)
    raise ValueError(f"unknown vLLM worker op {op!r}")


for line in sys.stdin:
    if not line.strip():
        continue
    try:
        request = json.loads(line)
        request_id = int(request.get("id", 0))
        result = handle(request_id, str(request.get("op", "")), request.get("payload"))
        send({"id": request_id, "type": "response", "ok": True, "result": result})
    except SystemExit:
        raise
    except Exception as exc:
        request_id = 0
        try:
            request_id = int(json.loads(line).get("id", 0))
        except Exception:
            pass
        error = str(exc) or repr(exc) or type(exc).__name__
        send({"id": request_id, "type": "response", "ok": False, "error": error})
