import contextlib
import json
import math
import os
import sys

import numpy as np
import torch


router = None


def emit(request_id, *, ok=False, result=None, error=None):
    message = {"id": request_id, "ok": ok}
    if result is not None:
        message["result"] = result
    if error is not None:
        message["error"] = error
    print(json.dumps(message, ensure_ascii=False, allow_nan=False), flush=True)


def require_cuda():
    if not torch.cuda.is_available():
        raise RuntimeError("Laya production backend requires CUDA")


def validate_resident_cuda():
    for name, agent in router._agents.items():
        if getattr(agent, "device", None) is None or agent.device.type != "cuda":
            raise RuntimeError("Laya checkpoint %s is not resident on CUDA" % name)


def load_model(payload):
    global router
    root = os.path.realpath(payload["path"])
    if not os.path.isdir(root):
        raise RuntimeError("Laya artifact path must be a local snapshot directory")
    required = [
        os.path.join(root, "model.safetensors"),
        os.path.join(root, "multilingual", "model.safetensors"),
        os.path.join(root, "typed-decisions", "model.safetensors"),
    ]
    missing = [path for path in required if not os.path.isfile(path)]
    if missing:
        raise RuntimeError("Laya snapshot is incomplete: " + ", ".join(missing))

    require_cuda()
    from laya import Router

    models = {
        "english": (root, None),
        "multilingual": (root, "multilingual"),
        "typed-decisions": (root, "typed-decisions"),
    }
    with contextlib.redirect_stdout(sys.stderr):
        router = Router(
            models=models,
            device="cuda",
            max_loaded=3,
            auto_task_detection=False,
            preload=True,
        )
    validate_resident_cuda()
    vocab = max(int(getattr(agent.tok, "vocab_size", 0)) for agent in router._agents.values())
    return {
        "device": "cuda",
        "n_ctx_train": 1024,
        "n_vocab": vocab,
        "loaded": list(router.loaded),
    }


def validate_finite(value):
    if isinstance(value, float) and not math.isfinite(value):
        raise RuntimeError("Laya returned a non-finite number")
    if isinstance(value, dict):
        for child in value.values():
            validate_finite(child)
    elif isinstance(value, list):
        for child in value:
            validate_finite(child)


def preprocess_state(state, email):
    if email is None:
        return state, None
    from laya import clean_email_body

    if not isinstance(state, dict) or not isinstance(state.get("body"), str):
        raise RuntimeError("email preprocessing requires state.body to be a string")
    clean = bool(email.get("clean", True))
    max_chars = int(email.get("max_chars", 3000))
    updated = dict(state)
    original = updated["body"]
    updated["body"] = clean_email_body(original, max_chars=max_chars) if clean else original[:max_chars]
    return updated, {
        "email": True,
        "clean": clean,
        "max_chars": max_chars,
        "input_body_chars": len(original),
        "processed_body_chars": len(updated["body"]),
    }


def per_request_system_one(agent, state, questions, temperatures, limits):
    if temperatures is None and limits is None:
        return agent.system_one(state, questions)

    from laya.common import (
        QTYPES,
        QTYPE_NAMES,
        build_sequence,
        collate_items,
        confidence_from_probs,
        render_options,
        temp_bucket,
    )

    ids = list(questions.keys())
    items = []
    temperatures = temperatures or {}
    limits = limits or {}
    config = getattr(agent, "cfg", None) or {}
    max_len = int(limits.get("max_len", config.get("max_len", 512)))
    head_max_len = int(
        limits.get("head_max_len", config.get("head_max_len", 192))
    )
    for qid in ids:
        question = agent._to_internal(questions[qid])
        sequence, markers = build_sequence(agent.tok, state, question, max_len, head_max_len)
        if len(markers) != len(render_options(question)):
            raise RuntimeError("question %r options exceed head_max_len=%d" % (qid, head_max_len))
        items.append({"ids": sequence, "markers": markers, "qtype": QTYPES[question["t"]]})

    batch = collate_items([items], agent.tok.pad_token_id)
    use_amp = agent.device.type == "cuda"
    with torch.no_grad(), torch.autocast(
        device_type=agent.device.type, dtype=agent.dtype, enabled=use_amp
    ):
        logits, action = agent.model(
            batch["input_ids"].to(agent.device),
            batch["attention_mask"].to(agent.device),
            batch["marker_pos"].to(agent.device),
            batch["marker_mask"].to(agent.device),
            batch["qtype"].to(agent.device),
        )
    logits = logits.float().cpu().numpy()
    action = torch.softmax(action.float(), -1).cpu().numpy()
    answers = {}
    for row, qid in enumerate(ids):
        question = agent._to_internal(questions[qid])
        count = len(items[row]["markers"])
        kind = QTYPES[question["t"]]
        bucket = temp_bucket(kind, count)
        default = agent.temperature_by_options.get(bucket, agent.temperature[kind])
        scale = float(temperatures.get(bucket, temperatures.get(QTYPE_NAMES[kind], default)))
        values = logits[row, :count] / scale
        probabilities = np.exp(values - values.max())
        probabilities = probabilities / probabilities.sum()
        confidence = round(confidence_from_probs(probabilities, count), 4)
        action_result = {"act_probability": round(float(action[row, 0]), 4)}
        if question["t"] == "choice":
            keys = list(question["crit"].keys())
            answers[qid] = {
                "type": "choice",
                "choice": keys[int(probabilities.argmax())],
                "probabilities": {
                    key: round(float(value), 4) for key, value in zip(keys, probabilities)
                },
                "confidence": confidence,
                "action": action_result,
            }
        elif question["t"] == "score":
            answers[qid] = {
                "type": "score",
                "score": round(float((np.arange(count) * probabilities).sum()), 4),
                "legend": {str(index): criterion for index, criterion in enumerate(question["crit"])},
                "probabilities": {
                    str(index): round(float(value), 4)
                    for index, value in enumerate(probabilities)
                },
                "confidence": confidence,
                "action": action_result,
            }
        else:
            truth = float(probabilities[1])
            answers[qid] = {
                "type": "noul",
                "noul": round(truth, 4),
                "confidence": round(max(truth, 1.0 - truth), 4),
                "action": action_result,
            }
    return {
        "model": "laya-rl-agent",
        "answers": answers,
        "usage": {
            "input_tokens": int(batch["attention_mask"].sum()),
            "output_tokens": 0,
        },
    }


def counted_embed_fn(agent, max_length, batch_size):
    counter = {"input_tokens": 0}

    def embed(texts):
        rows = ["" if text is None else str(text) for text in texts]
        parts = []
        for start in range(0, len(rows), batch_size):
            encoded = agent.tok(
                rows[start : start + batch_size],
                padding=True,
                truncation=True,
                max_length=max_length,
                return_tensors="pt",
            )
            input_ids = encoded["input_ids"].to(agent.device)
            attention_mask = encoded["attention_mask"].to(agent.device)
            counter["input_tokens"] += int(attention_mask.sum())
            with torch.inference_mode():
                hidden = agent.model.encoder(
                    input_ids=input_ids, attention_mask=attention_mask
                ).last_hidden_state
                mask = attention_mask.unsqueeze(-1).to(dtype=hidden.dtype)
                pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1.0)
            parts.append(pooled.float().cpu().numpy())
        return np.concatenate(parts, axis=0)

    return embed, counter


def supplied_vector_embed_fn(questions, supplied, k):
    matrices = []
    for qid, question in questions.items():
        if question.get("type") != "choice":
            continue
        criteria = question["criteria"]
        labels = list(criteria.keys()) if isinstance(criteria, dict) else list(range(len(criteria)))
        if len(labels) <= k:
            continue
        entry = supplied[qid]
        options = entry["options"]
        option_vectors = (
            [options[label] for label in labels]
            if isinstance(criteria, dict)
            else options
        )
        matrices.append(np.asarray([entry["query"], *option_vectors], dtype=np.float32))
    cursor = {"index": 0}

    def embed(texts):
        index = cursor["index"]
        if index >= len(matrices):
            raise RuntimeError("shortlist supplied-vector calls exceeded validated questions")
        matrix = matrices[index]
        cursor["index"] += 1
        if matrix.ndim != 2 or matrix.shape[0] != len(texts):
            raise RuntimeError("shortlist supplied vectors do not match the requested rows")
        return matrix

    return embed, cursor, len(matrices)


class PerRequestPredictor:
    def __init__(self, agent, temperatures, limits):
        self.agent = agent
        self.temperatures = temperatures
        self.limits = limits

    def predict(self, state, questions):
        return per_request_system_one(
            self.agent, state, questions, self.temperatures, self.limits
        )


def decide(payload):
    if router is None:
        raise RuntimeError("Laya model is not loaded")
    validate_resident_cuda()
    state, preprocessing = preprocess_state(payload["state"], payload.get("email"))
    questions = payload["questions"]
    checkpoint = payload.get("checkpoint")
    task = payload.get("task")
    workflow = None
    if payload.get("auto_task_detection", False) and checkpoint is None and task is None:
        from laya.router import match_typed_decisions_workflow

        workflow = match_typed_decisions_workflow(questions)
        if workflow is not None:
            task = "typed_decisions"
    with contextlib.redirect_stdout(sys.stderr):
        route = router.route(
            state,
            questions,
            model=checkpoint,
            task=task,
            lang=payload.get("lang"),
        )
        if workflow is not None:
            route["reason"] = (
                "question ids match the %r typed-decisions workflow" % workflow
            )
            route["workflow"] = workflow
        route.pop("repo", None)
        agent = router.load(route["model"])
        shortlist = payload.get("shortlist")
        if shortlist is not None:
            from laya import predict_shortlist

            supplied = shortlist.get("vectors")
            k = int(shortlist.get("k", 20))
            if supplied is None:
                embed, counter = counted_embed_fn(
                    agent,
                    int(shortlist.get("max_length", 512)),
                    int(shortlist.get("batch_size", 32)),
                )
                expected_calls = None
            else:
                embed, cursor, expected_calls = supplied_vector_embed_fn(
                    questions, supplied, k
                )
                counter = {"input_tokens": 0}
            result = predict_shortlist(
                PerRequestPredictor(
                    agent, payload.get("temperature"), payload.get("limits")
                ),
                state,
                questions,
                embed,
                k=k,
            )
            if expected_calls is not None and cursor["index"] != expected_calls:
                raise RuntimeError("shortlist did not consume every supplied vector set")
            result["usage"]["shortlist_input_tokens"] = counter["input_tokens"]
            result["usage"]["input_tokens"] += counter["input_tokens"]
            for metadata in result["shortlist"].values():
                metadata["embedding_source"] = (
                    "supplied_vectors" if supplied is not None else "local_encoder"
                ) if not metadata["passthrough"] else "passthrough"
        else:
            result = per_request_system_one(
                agent,
                state,
                questions,
                payload.get("temperature"),
                payload.get("limits"),
            )
        result["routing"] = dict(route)
    if preprocessing is not None:
        result["preprocessing"] = preprocessing
    validate_resident_cuda()
    validate_finite(result)
    return result


def main():
    for line in sys.stdin:
        request_id = 0
        try:
            message = json.loads(line)
            request_id = int(message.get("id", 0))
            operation = message.get("op")
            payload = message.get("payload") or {}
            if operation == "shutdown":
                emit(request_id, ok=True, result={})
                return
            if operation == "load":
                emit(request_id, ok=True, result=load_model(payload))
            elif operation == "decide":
                emit(request_id, ok=True, result=decide(payload))
            else:
                raise RuntimeError("unsupported Laya worker operation: %r" % operation)
        except Exception as error:
            emit(request_id, error=str(error))


if __name__ == "__main__":
    main()
