import inspect
import json
import queue
import sys
import threading


model = None
tokenizer = None
ctx_size = 2048
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
        cancelled_requests.difference_update(
            request_id
            for request_id in cancelled_requests
            if request_id <= completed_request_id
        )


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


def request_prompt(payload, template_kwargs, prompt_suffixes):
    prompt = str(payload.get("prompt", ""))
    template_tools = payload.get("tools") or []
    if not isinstance(template_tools, list):
        raise ValueError("MLX chat-template tools must be an array")
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


def model_ctx(default):
    args = getattr(model, "args", None)
    for name in (
        "max_position_embeddings",
        "max_sequence_length",
        "max_seq_len",
        "context_length",
        "model_max_length",
    ):
        value = getattr(args, name, None)
        if isinstance(value, int) and value > 0:
            return value
    return int(default)


def handle_load(payload):
    global model, tokenizer, ctx_size
    from mlx_lm import load

    path = str(payload["path"])
    ctx_size = int(payload.get("ctx_size") or 2048)
    model, tokenizer = load(path)
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
    prompt = request_prompt(payload, template_kwargs, prompt_suffixes)
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

    grammar = payload.get("grammar")
    if grammar is not None:
        raise ValueError(
            "MLX backend does not support grammar-constrained tool calls; advertise caps.tools=false"
        )
    if payload.get("frequency_penalty") is not None:
        raise ValueError(
            "MLX backend does not support frequency_penalty; omit it from this artifact endpoint contract"
        )
    if payload.get("presence_penalty") is not None:
        raise ValueError(
            "MLX backend does not support presence_penalty; omit it from this artifact endpoint contract"
        )
    if payload.get("stop"):
        raise ValueError(
            "MLX backend does not support stop sequences; omit stop from this artifact endpoint contract"
        )

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
        sampler_kwargs["top_k"] = int(payload.get("top_k"))
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
    logits_processors = make_logits_processors(
        repetition_penalty=(
            None if repeat_penalty is None else float(repeat_penalty)
        )
    )

    text = ""
    completion_tokens = 0
    finish_reason = "length"
    reasoning_tokens = 0
    reasoning_active = reasoning_enabled(payload)
    for response in stream_generate(
        model,
        tokenizer,
        prompt,
        max_tokens=max_tokens,
        sampler=sampler,
        logits_processors=logits_processors,
        max_kv_size=ctx_size,
    ):
        check_cancelled(request_id)
        segment = str(getattr(response, "text", ""))
        token = int(getattr(response, "token", -1))
        generated_attr = getattr(response, "generation_tokens", None)
        generated = (
            int(generated_attr)
            if generated_attr is not None
            else completion_tokens + (1 if token >= 0 else 0)
        )
        if token >= 0:
            send(
                {
                    "id": request_id,
                    "type": "token",
                    "chunk": {
                        "index": max(0, generated - 1),
                        "token_id": token,
                        "text": segment,
                    },
                }
            )
            if reasoning_active:
                reasoning_tokens += 1
        if segment:
            text += segment
            if reasoning_active and "</think>" in text:
                reasoning_active = False
        completion_tokens = max(completion_tokens, generated)
        final_reason = getattr(response, "finish_reason", None)
        if final_reason is not None:
            finish_reason = str(final_reason)
            break

    check_cancelled(request_id)

    return {
        "text": text,
        "usage": {
            "prompt_tokens": len(prompt_tokens),
            "completion_tokens": completion_tokens,
            "total_tokens": len(prompt_tokens) + completion_tokens,
            "reasoning_tokens": min(reasoning_tokens, completion_tokens),
            "vision_tokens": 0,
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
    try:
        if "parse_error" in request:
            raise ValueError(request["parse_error"])
        result = handle(request_id, str(request.get("op", "")), request.get("payload"))
        check_cancelled(request_id)
        send({"id": request_id, "type": "response", "ok": True, "result": result})
    except SystemExit:
        raise
    except RequestCancelled as exc:
        send(
            {
                "id": request_id,
                "type": "response",
                "ok": False,
                "cancelled": True,
                "error": str(exc),
            }
        )
    except Exception as exc:
        send({"id": request_id, "type": "response", "ok": False, "error": str(exc)})
    finally:
        finish_request(request_id)
