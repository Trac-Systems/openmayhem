import json
import sys


model = None
tokenizer = None
ctx_size = 2048


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


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
    if model is None or tokenizer is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = int(payload.get("max_new_tokens") or 64)
    prompt = str(payload.get("prompt", ""))
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
        return handle_grammar_generate(request_id, grammar, prompt_tokens, max_tokens)

    import mlx.core as mx
    from mlx_lm import stream_generate
    from mlx_lm.sample_utils import make_sampler

    seed = payload.get("seed")
    if seed is not None:
        mx.random.seed(int(seed))

    temperature = float(payload.get("temperature") or 0.0)
    top_p = float(payload.get("top_p") or 0.0)
    sampler = make_sampler(temp=temperature, top_p=top_p)

    text = ""
    completion_tokens = 0
    finish_reason = "length"
    for response in stream_generate(
        model,
        tokenizer,
        prompt,
        max_tokens=max_tokens,
        sampler=sampler,
        max_kv_size=ctx_size,
    ):
        segment = str(getattr(response, "text", ""))
        token = int(getattr(response, "token", -1))
        generated = int(getattr(response, "generation_tokens", completion_tokens))
        if segment:
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
            text += segment
        completion_tokens = max(completion_tokens, generated)
        final_reason = getattr(response, "finish_reason", None)
        if final_reason is not None:
            finish_reason = str(final_reason)
            break

    return {
        "text": text,
        "usage": {
            "prompt_tokens": len(prompt_tokens),
            "completion_tokens": completion_tokens,
            "total_tokens": len(prompt_tokens) + completion_tokens,
        },
        "finish_reason": normalize_finish_reason(finish_reason),
    }


def handle_grammar_generate(request_id, grammar, prompt_tokens, max_tokens):
    if grammar.get("kind") != "tool_call":
        raise ValueError("MLX backend currently supports tool_call grammar constraints")
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
        send({"id": request_id, "type": "response", "ok": False, "error": str(exc)})
