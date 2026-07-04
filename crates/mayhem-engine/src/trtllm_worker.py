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


def positive_int(value, default):
    try:
        parsed = int(value)
        return parsed if parsed > 0 else int(default)
    except Exception:
        return int(default)


def load_limits(payload):
    ctx_limit = positive_int(payload.get("ctx_size"), 2048)
    max_batch_size = positive_int(payload.get("max_batch_size"), 1)
    max_num_tokens = positive_int(payload.get("max_num_tokens"), max(256, ctx_limit))
    max_num_tokens = max(max_num_tokens, ctx_limit)
    return ctx_limit, max_batch_size, max_num_tokens


def kv_cache_config(dtype, ctx_limit, max_batch_size, max_num_tokens):
    try:
        KvCacheConfig = import_attr(
            (
                ("tensorrt_llm", "KvCacheConfig"),
                ("tensorrt_llm.llmapi", "KvCacheConfig"),
                ("tensorrt_llm.llmapi.llm", "KvCacheConfig"),
            )
        )
        max_tokens = max(
            2048,
            int(max_num_tokens or 0),
            int(ctx_limit or 2048) * int(max_batch_size or 1),
        )
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
    ctx_limit, max_batch_size, max_num_tokens = load_limits(payload)
    require_engine_dir = bool(payload.get("require_engine_dir"))
    kv_config = kv_cache_config(
        payload.get("kv_cache_dtype"), ctx_limit, max_batch_size, max_num_tokens
    )

    attempts = []
    optional = {
        "max_batch_size": max_batch_size,
        "max_input_len": ctx_limit,
        "max_seq_len": ctx_limit,
        "max_num_tokens": max_num_tokens,
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
    ctx_limit, max_batch_size, max_num_tokens = load_limits(payload)
    kwargs = {
        "engine_dir": str(engine_dir),
        "rank": 0,
        "max_batch_size": max_batch_size,
        "max_beam_width": 1,
        "kv_cache_free_gpu_memory_fraction": 0.10,
        "max_tokens_in_paged_kv_cache": max(
            2048, max_num_tokens, ctx_limit * max_batch_size
        ),
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
    ignore_eos = bool(payload.get("ignore_eos"))
    kwargs = {
        "max_tokens": int(payload.get("max_new_tokens") or 64),
        "max_new_tokens": int(payload.get("max_new_tokens") or 64),
        "temperature": float(payload.get("temperature") or 0.0),
        "ignore_eos": ignore_eos,
    }
    if ignore_eos:
        kwargs["min_tokens"] = kwargs["max_tokens"]
        kwargs["min_new_tokens"] = kwargs["max_tokens"]
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


def request_max_tokens(payload):
    value = payload.get("max_new_tokens")
    return 64 if value is None else int(value)


def runner_output_tokens(outputs, prompt_len, max_tokens):
    return runner_batch_output_tokens(outputs, [prompt_len], max_tokens)[0]


def runner_batch_output_tokens(outputs, prompt_lens, max_tokens):
    output_ids = outputs.get("output_ids") if isinstance(outputs, dict) else outputs
    sequence_lengths = outputs.get("sequence_lengths") if isinstance(outputs, dict) else None

    batch_tokens = []
    for batch_index, prompt_len in enumerate(prompt_lens):
        try:
            first_beam = output_ids[batch_index][0]
        except Exception as exc:
            raise RuntimeError(
                f"TensorRT-LLM runner returned unexpected output ids for batch index {batch_index}: {exc}"
            ) from exc

        try:
            seq_len = int(sequence_lengths[batch_index][0].item())
        except Exception:
            seq_len = int(first_beam.shape[0]) if hasattr(first_beam, "shape") else len(first_beam)

        start = max(0, int(prompt_len))
        end = max(start, min(seq_len, start + int(max_tokens)))
        sliced = first_beam[start:end]
        if hasattr(sliced, "detach"):
            sliced = sliced.detach().cpu().tolist()
        elif hasattr(sliced, "tolist"):
            sliced = sliced.tolist()
        batch_tokens.append([int(token) for token in sliced])
    return batch_tokens


def runner_generate_tokens(prompt_tokens, payload, max_tokens):
    return runner_generate_batch_tokens([prompt_tokens], payload, max_tokens)[0]


def runner_generate_batch_tokens(prompt_batches, payload, max_tokens):
    import torch

    pad_id = tokenizer_token_id("pad_token_id", "eos_token_id")
    kwargs = {
        "batch_input_ids": [
            torch.tensor(prompt_tokens, dtype=torch.int32, device="cuda")
            for prompt_tokens in prompt_batches
        ],
        "max_new_tokens": int(max_tokens),
        "pad_id": pad_id,
        "temperature": float(payload.get("temperature") or 0.0),
        "top_p": float(payload.get("top_p") or 0.0),
        "output_sequence_lengths": True,
        "return_dict": True,
    }
    if not bool(payload.get("ignore_eos")):
        kwargs["end_id"] = tokenizer_token_id("eos_token_id")
    if kwargs["temperature"] <= 0.0:
        kwargs["temperature"] = 1.0
        kwargs["top_k"] = 1
    if payload.get("seed") is not None:
        kwargs["random_seed"] = int(payload.get("seed"))

    with torch.no_grad():
        outputs = model.generate(**kwargs)
        torch.cuda.synchronize()
    return runner_batch_output_tokens(
        outputs, [len(prompt_tokens) for prompt_tokens in prompt_batches], max_tokens
    )


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


def prepare_prompt(payload):
    prompt = str(payload.get("prompt", ""))
    prompt_tokens = encode_text(prompt)
    if prompt_tokens and len(prompt_tokens) >= ctx_size:
        raise ValueError(
            f"prompt has {len(prompt_tokens)} tokens, leaving no room in ctx_size={ctx_size}"
        )
    return prompt, prompt_tokens


def runner_result(prompt_tokens, completion_tokens, max_tokens):
    if len(completion_tokens) > max_tokens:
        completion_tokens = completion_tokens[:max_tokens]
    text = decode_tokens(completion_tokens)
    prompt_count = len(prompt_tokens)
    completion_count = len(completion_tokens) if completion_tokens else (1 if text else 0)
    return {
        "text": text,
        "usage": {
            "prompt_tokens": prompt_count,
            "completion_tokens": completion_count,
            "total_tokens": prompt_count + completion_count,
        },
        "finish_reason": "length" if completion_count >= max_tokens else "stop",
    }


def batch_generation_settings(payload):
    return (
        request_max_tokens(payload),
        bool(payload.get("ignore_eos")),
        float(payload.get("temperature") or 0.0),
        float(payload.get("top_p") or 0.0),
        payload.get("seed"),
    )


def can_runner_batch(payloads):
    if model_kind != "runner_cpp" or not payloads:
        return False
    settings = batch_generation_settings(payloads[0])
    for payload in payloads:
        if payload.get("grammar") is not None:
            return False
        if batch_generation_settings(payload) != settings:
            return False
    return True


def handle_generate(request_id, payload):
    if model is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = request_max_tokens(payload)
    prompt, prompt_tokens = prepare_prompt(payload)
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


def handle_generate_batch(request_id, payload):
    if model is None:
        raise RuntimeError("model has not been loaded")
    if not isinstance(payload, list):
        raise ValueError("generate_batch payload must be a list of requests")

    payloads = []
    for item in payload:
        if item is None:
            item = {}
        if not isinstance(item, dict):
            raise ValueError("generate_batch entries must be request objects")
        payloads.append(item)
    if not payloads:
        return []

    if not can_runner_batch(payloads):
        return [handle_generate(request_id, item) for item in payloads]

    max_tokens = request_max_tokens(payloads[0])
    prepared = [prepare_prompt(item)[1] for item in payloads]
    if max_tokens <= 0:
        return [runner_result(prompt_tokens, [], max_tokens) for prompt_tokens in prepared]

    completion_batches = runner_generate_batch_tokens(prepared, payloads[0], max_tokens)
    if len(completion_batches) != len(prepared):
        raise RuntimeError(
            f"TensorRT-LLM runner returned {len(completion_batches)} batch outputs for {len(prepared)} requests"
        )
    return [
        runner_result(prompt_tokens, completion_tokens, max_tokens)
        for prompt_tokens, completion_tokens in zip(prepared, completion_batches)
    ]


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
    if op == "generate_batch":
        return handle_generate_batch(request_id, payload or [])
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
