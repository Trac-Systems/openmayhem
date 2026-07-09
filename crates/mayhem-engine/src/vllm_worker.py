import asyncio
import inspect
import json
import os
import sys


protocol_stdout = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
sys.stdout = sys.stderr


engine = None
tokenizer = None
ctx_size = 2048
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


def make_sampling_params(payload):
    SamplingParams = import_attr((("vllm", "SamplingParams"),))
    kwargs = {
        "max_tokens": int(payload.get("max_new_tokens") or 64),
        "temperature": float(payload.get("temperature") or 0.0),
    }
    top_p = payload.get("top_p")
    if top_p is not None and float(top_p) > 0.0:
        kwargs["top_p"] = float(top_p)
    if payload.get("seed") is not None:
        kwargs["seed"] = int(payload.get("seed"))
    if bool(payload.get("ignore_eos")):
        kwargs["ignore_eos"] = True
        kwargs["min_tokens"] = kwargs["max_tokens"]
    structured = make_structured_outputs_params(payload.get("grammar"))
    if structured is not None:
        kwargs["structured_outputs"] = structured
        kwargs["guided_decoding"] = structured
    try:
        RequestOutputKind = import_attr((("vllm.sampling_params", "RequestOutputKind"),))
        kwargs["output_kind"] = getattr(RequestOutputKind, "DELTA")
    except Exception:
        pass
    return SamplingParams(**accepted_kwargs(SamplingParams, kwargs))


def create_engine(payload):
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
    path = str(payload["path"])
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
    }
    dtype = payload.get("dtype")
    if dtype:
        kwargs["dtype"] = str(dtype)
    gpu_memory_utilization = utilization_float(payload.get("gpu_memory_utilization"))
    if gpu_memory_utilization is not None:
        kwargs["gpu_memory_utilization"] = gpu_memory_utilization
    args = AsyncEngineArgs(**accepted_kwargs(AsyncEngineArgs, kwargs))
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


async def async_handle_generate(request_id, payload):
    if engine is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = request_max_tokens(payload)
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

    sampling_params = make_sampling_params(payload)
    text = ""
    completion_tokens = 0
    token_ids = []
    finish_reason = "length"
    async for output in engine.generate(
        request_id=f"mayhem-{request_id}", prompt=prompt, sampling_params=sampling_params
    ):
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
                text += chunk_text
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
            "prompt_tokens": len(prompt_tokens),
            "completion_tokens": completion_tokens,
            "total_tokens": len(prompt_tokens) + completion_tokens,
        },
        "finish_reason": finish_reason,
    }


def handle_load(payload):
    global engine, tokenizer, ctx_size, model_path
    model_path = str(payload["path"])
    ctx_size = positive_int(payload.get("ctx_size"), 2048)
    engine = create_engine(payload)
    try:
        from transformers import AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=False)
    except Exception:
        tokenizer = None
    get_tokenizer()
    return {
        "n_ctx_train": model_ctx(ctx_size),
        "n_vocab": int(vocab_size()),
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
