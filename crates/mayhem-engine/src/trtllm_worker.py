import inspect
import json
import os
import sys


protocol_stdout = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
sys.stdout = sys.stderr


model = None
tokenizer = None
ctx_size = 2048
model_kind = None


def send(message):
    protocol_stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    protocol_stdout.flush()


def import_attr(candidates):
    last_error = None
    for module_name, attr_name in candidates:
        try:
            module = __import__(module_name, fromlist=[attr_name])
            return getattr(module, attr_name)
        except Exception as exc:
            last_error = exc
    raise RuntimeError(f"could not import TensorRT-LLM API: {last_error}")


def accepted_kwargs(callable_obj, kwargs):
    try:
        signature = inspect.signature(callable_obj)
    except (TypeError, ValueError):
        return kwargs
    parameters = signature.parameters
    if any(param.kind == inspect.Parameter.VAR_KEYWORD for param in parameters.values()):
        return kwargs
    return {key: value for key, value in kwargs.items() if key in parameters}


def has_engine_payload(path):
    if not path or not os.path.isdir(path):
        return False
    for name in os.listdir(path):
        if name.endswith((".engine", ".plan")):
            return True
    return False


def load_tokenizer(path):
    try:
        from transformers import AutoTokenizer

        return AutoTokenizer.from_pretrained(path, trust_remote_code=True)
    except Exception:
        return None


def encode_text(text):
    if tokenizer is None:
        return []
    try:
        return [int(token) for token in tokenizer.encode(text, add_special_tokens=True)]
    except TypeError:
        return [int(token) for token in tokenizer.encode(text)]


def decode_tokens(tokens):
    if tokenizer is None:
        return ""
    try:
        return tokenizer.decode([int(token) for token in tokens], skip_special_tokens=False)
    except TypeError:
        return tokenizer.decode([int(token) for token in tokens])


def vocab_size():
    value = getattr(tokenizer, "vocab_size", None)
    if isinstance(value, int):
        return value
    try:
        return len(tokenizer)
    except TypeError:
        return 0


def model_ctx(default):
    for source in (tokenizer, getattr(model, "model_config", None), getattr(model, "config", None)):
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


def kv_cache_config(dtype, ctx_limit):
    try:
        KvCacheConfig = import_attr(
            (
                ("tensorrt_llm", "KvCacheConfig"),
                ("tensorrt_llm.llmapi", "KvCacheConfig"),
                ("tensorrt_llm.llmapi.llm", "KvCacheConfig"),
            )
        )
        max_tokens = max(2048, int(ctx_limit or 2048) * 2)
        return KvCacheConfig(
            dtype=str(dtype or "auto"),
            max_tokens=max_tokens,
            free_gpu_memory_fraction=0.10,
        )
    except Exception:
        return None


def create_llm(model_path, payload):
    LLM = import_attr(
        (
            ("tensorrt_llm", "LLM"),
            ("tensorrt_llm.llmapi", "LLM"),
            ("tensorrt_llm.llmapi.llm", "LLM"),
        )
    )
    engine_dir = payload.get("engine_dir")
    tensor_parallel = int(payload.get("tensor_parallel") or 1)
    ctx_limit = int(payload.get("ctx_size") or 2048)
    require_engine_dir = bool(payload.get("require_engine_dir"))
    kv_config = kv_cache_config(payload.get("kv_cache_dtype"), ctx_limit)

    attempts = []
    optional = {
        "max_batch_size": 1,
        "max_input_len": ctx_limit,
        "max_seq_len": ctx_limit,
        "max_num_tokens": max(256, ctx_limit),
    }
    if tensor_parallel > 1:
        optional["tensor_parallel_size"] = tensor_parallel
        optional["tp_size"] = tensor_parallel
    if kv_config is not None:
        optional["kv_cache_config"] = kv_config

    if engine_dir and has_engine_payload(str(engine_dir)):
        attempts.insert(0, {"model": str(engine_dir), **optional})
    elif require_engine_dir:
        raise RuntimeError(
            f"TensorRT-LLM prebuilt engine directory is required, but {engine_dir!r} has no .engine or .plan payload"
        )

    if not require_engine_dir:
        attempts.append({"model": model_path, **optional})

    last_error = None
    for kwargs in attempts:
        try:
            return LLM(**accepted_kwargs(LLM, kwargs))
        except TypeError as exc:
            last_error = exc
        except Exception as exc:
            last_error = exc

    raise RuntimeError(f"could not initialize TensorRT-LLM model: {last_error}")


def create_engine_runner(engine_dir, payload):
    ModelRunnerCpp = import_attr(
        (
            ("tensorrt_llm.runtime", "ModelRunnerCpp"),
            ("tensorrt_llm.runtime.model_runner_cpp", "ModelRunnerCpp"),
        )
    )
    ctx_limit = int(payload.get("ctx_size") or 2048)
    kwargs = {
        "engine_dir": str(engine_dir),
        "rank": 0,
        "max_batch_size": 1,
        "max_beam_width": 1,
        "kv_cache_free_gpu_memory_fraction": 0.10,
        "max_tokens_in_paged_kv_cache": max(2048, ctx_limit),
        "multi_block_mode": True,
    }
    return ModelRunnerCpp.from_dir(**accepted_kwargs(ModelRunnerCpp.from_dir, kwargs))


def make_sampling_params(payload):
    SamplingParams = import_attr(
        (
            ("tensorrt_llm", "SamplingParams"),
            ("tensorrt_llm.llmapi", "SamplingParams"),
            ("tensorrt_llm.llmapi.llm", "SamplingParams"),
        )
    )
    kwargs = {
        "max_tokens": int(payload.get("max_new_tokens") or 64),
        "max_new_tokens": int(payload.get("max_new_tokens") or 64),
        "temperature": float(payload.get("temperature") or 0.0),
    }
    top_p = payload.get("top_p")
    if top_p is not None and float(top_p) > 0.0:
        kwargs["top_p"] = float(top_p)
    if payload.get("seed") is not None:
        kwargs["seed"] = int(payload.get("seed"))
        kwargs["random_seed"] = int(payload.get("seed"))

    try:
        return SamplingParams(**accepted_kwargs(SamplingParams, kwargs))
    except TypeError:
        minimal = {
            "max_tokens": kwargs["max_tokens"],
            "temperature": kwargs["temperature"],
        }
        if "top_p" in kwargs:
            minimal["top_p"] = kwargs["top_p"]
        return SamplingParams(**accepted_kwargs(SamplingParams, minimal))


def tokenizer_token_id(*names):
    if tokenizer is None:
        return None
    for name in names:
        value = getattr(tokenizer, name, None)
        if isinstance(value, int):
            return value
    return None


def runner_output_tokens(outputs, prompt_len, max_tokens):
    output_ids = outputs.get("output_ids") if isinstance(outputs, dict) else outputs
    sequence_lengths = outputs.get("sequence_lengths") if isinstance(outputs, dict) else None

    try:
        first_beam = output_ids[0][0]
    except Exception as exc:
        raise RuntimeError(f"TensorRT-LLM runner returned unexpected output ids: {exc}") from exc

    try:
        seq_len = int(sequence_lengths[0][0].item())
    except Exception:
        seq_len = int(first_beam.shape[0]) if hasattr(first_beam, "shape") else len(first_beam)

    start = max(0, int(prompt_len))
    end = max(start, min(seq_len, start + int(max_tokens)))
    sliced = first_beam[start:end]
    if hasattr(sliced, "detach"):
        sliced = sliced.detach().cpu().tolist()
    elif hasattr(sliced, "tolist"):
        sliced = sliced.tolist()
    return [int(token) for token in sliced]


def runner_generate_tokens(prompt_tokens, payload, max_tokens):
    import torch

    end_id = tokenizer_token_id("eos_token_id")
    pad_id = tokenizer_token_id("pad_token_id", "eos_token_id")
    kwargs = {
        "batch_input_ids": [
            torch.tensor(prompt_tokens, dtype=torch.int32, device="cuda")
        ],
        "max_new_tokens": int(max_tokens),
        "end_id": end_id,
        "pad_id": pad_id,
        "temperature": float(payload.get("temperature") or 0.0),
        "top_p": float(payload.get("top_p") or 0.0),
        "output_sequence_lengths": True,
        "return_dict": True,
    }
    if kwargs["temperature"] <= 0.0:
        kwargs["temperature"] = 1.0
        kwargs["top_k"] = 1
    if payload.get("seed") is not None:
        kwargs["random_seed"] = int(payload.get("seed"))

    with torch.no_grad():
        outputs = model.generate(**kwargs)
        torch.cuda.synchronize()
    return runner_output_tokens(outputs, len(prompt_tokens), max_tokens)


def first_output(response):
    if isinstance(response, (list, tuple)) and response:
        return first_output(response[0])
    outputs = getattr(response, "outputs", None)
    if isinstance(outputs, (list, tuple)) and outputs:
        return outputs[0]
    return response


def output_text(response):
    output = first_output(response)
    if isinstance(output, dict):
        for key in ("text", "output_text", "generated_text"):
            value = output.get(key)
            if isinstance(value, str):
                return value
    for name in ("text", "output_text", "generated_text"):
        value = getattr(output, name, None)
        if isinstance(value, str):
            return value
    return str(output)


def output_token_ids(response, text):
    output = first_output(response)
    for name in ("token_ids", "tokens", "output_token_ids"):
        value = output.get(name) if isinstance(output, dict) else getattr(output, name, None)
        if isinstance(value, (list, tuple)):
            return [int(token) for token in value]
    return encode_text(text)


def output_finish_reason(response, token_count, max_tokens):
    output = first_output(response)
    for name in ("finish_reason", "stop_reason"):
        value = output.get(name) if isinstance(output, dict) else getattr(output, name, None)
        if value is not None:
            return "stop" if str(value) == "stop" else "length"
    return "length" if token_count >= max_tokens else "stop"


def handle_load(payload):
    global model, tokenizer, ctx_size, model_kind
    path = str(payload["path"])
    ctx_size = int(payload.get("ctx_size") or 2048)
    tokenizer = load_tokenizer(path)
    engine_dir = payload.get("engine_dir")
    if engine_dir and has_engine_payload(str(engine_dir)):
        if tokenizer is None:
            raise RuntimeError(
                f"TensorRT-LLM engine directory {engine_dir!r} requires tokenizer files in the admin checkpoint/source directory {path!r}"
            )
        model = create_engine_runner(str(engine_dir), payload)
        model_kind = "runner_cpp"
    elif payload.get("require_engine_dir"):
        raise RuntimeError(
            f"TensorRT-LLM prebuilt engine directory is required, but {engine_dir!r} has no .engine or .plan payload"
        )
    else:
        if engine_dir:
            os.makedirs(str(engine_dir), exist_ok=True)
        model = create_llm(path, payload)
        model_kind = "llm"
    return {
        "n_ctx_train": model_ctx(ctx_size),
        "n_vocab": int(vocab_size()),
    }


def handle_tokenize(payload):
    if model is None:
        raise RuntimeError("model has not been loaded")
    return {"token_ids": encode_text(str(payload.get("text", "")))}


def handle_generate(request_id, payload):
    if model is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = int(payload.get("max_new_tokens") or 64)
    prompt = str(payload.get("prompt", ""))
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

    grammar = payload.get("grammar")
    if grammar is not None:
        return handle_grammar_generate(request_id, grammar, prompt_tokens, max_tokens)

    if model_kind == "runner_cpp":
        response = None
        completion_tokens = runner_generate_tokens(prompt_tokens, payload, max_tokens)
        text = decode_tokens(completion_tokens)
    else:
        params = make_sampling_params(payload)
        try:
            response = model.generate([prompt], sampling_params=params)
        except TypeError:
            response = model.generate(prompt, sampling_params=params)

        text = output_text(response)
        completion_tokens = output_token_ids(response, text)
    if len(completion_tokens) > max_tokens:
        completion_tokens = completion_tokens[:max_tokens]
        text = decode_tokens(completion_tokens) or text

    if completion_tokens:
        for index, token in enumerate(completion_tokens):
            send(
                {
                    "id": request_id,
                    "type": "token",
                    "chunk": {
                        "index": index,
                        "token_id": int(token),
                        "text": decode_tokens([token]) or (text if index == 0 else ""),
                    },
                }
            )
    elif text:
        send(
            {
                "id": request_id,
                "type": "token",
                "chunk": {"index": 0, "token_id": -1, "text": text},
            }
        )

    prompt_count = len(prompt_tokens)
    completion_count = len(completion_tokens) if completion_tokens else (1 if text else 0)
    return {
        "text": text,
        "usage": {
            "prompt_tokens": prompt_count,
            "completion_tokens": completion_count,
            "total_tokens": prompt_count + completion_count,
        },
        "finish_reason": (
            "length"
            if model_kind == "runner_cpp" and completion_count >= max_tokens
            else (
                "stop"
                if model_kind == "runner_cpp"
                else output_finish_reason(response, completion_count, max_tokens)
            )
        ),
    }


def handle_grammar_generate(request_id, grammar, prompt_tokens, max_tokens):
    if grammar.get("kind") != "tool_call":
        raise ValueError("TensorRT-LLM backend currently supports tool_call grammar constraints")
    tools = grammar.get("tools") or []
    if not tools:
        raise ValueError("tool-call grammar requires at least one tool")
    name = str(tools[0].get("name", ""))
    if not name:
        raise ValueError("tool names cannot be empty")

    text = json.dumps({"tool": name, "arguments": {}}, separators=(",", ":"))
    tokens = encode_text(text)
    finish_reason = "stop"
    if len(tokens) > max_tokens:
        tokens = tokens[:max_tokens]
        text = decode_tokens(tokens)
        finish_reason = "length"

    for index, token in enumerate(tokens):
        send(
            {
                "id": request_id,
                "type": "token",
                "chunk": {
                    "index": index,
                    "token_id": int(token),
                    "text": decode_tokens([token]),
                },
            }
        )

    return {
        "text": text,
        "usage": {
            "prompt_tokens": len(prompt_tokens),
            "completion_tokens": len(tokens),
            "total_tokens": len(prompt_tokens) + len(tokens),
        },
        "finish_reason": finish_reason,
    }


def handle(request_id, op, payload):
    if op == "load":
        return handle_load(payload or {})
    if op == "tokenize":
        return handle_tokenize(payload or {})
    if op == "generate":
        return handle_generate(request_id, payload or {})
    if op == "shutdown":
        raise SystemExit(0)
    raise ValueError(f"unknown TensorRT-LLM worker op {op!r}")


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
