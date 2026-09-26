#!/usr/bin/env python3
"""Compare pinned Qwen reference pooling with retained vLLM canary vectors."""

import argparse
import json
import math
import time
from pathlib import Path

import torch
import torch.nn.functional as F
from transformers import AutoModel, AutoTokenizer


def last_token_pool(last_hidden_states, attention_mask):
    left_padding = attention_mask[:, -1].sum() == attention_mask.shape[0]
    if left_padding:
        return last_hidden_states[:, -1]
    sequence_lengths = attention_mask.sum(dim=1) - 1
    batch_size = last_hidden_states.shape[0]
    return last_hidden_states[torch.arange(batch_size), sequence_lengths]


def cosine(left, right):
    dot = sum(a * b for a, b in zip(left, right))
    ln = math.sqrt(sum(a * a for a in left))
    rn = math.sqrt(sum(b * b for b in right))
    return dot / (ln * rn)


def truncate_normalize(vector, dimensions):
    values = vector[:dimensions]
    norm = math.sqrt(sum(value * value for value in values))
    return [value / norm for value in values]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--canary", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    canary = json.loads(Path(args.canary).read_text())
    vectors = {row["prompt_id"]: row["embedding_vector"] for row in canary["prompts"]}
    query = "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: How does OpenMayhem verify model artifacts?"
    document = "OpenMayhem binds model artifacts to immutable revisions, verified hashes, calibrated canaries, and signed catalog evidence."

    started = time.perf_counter()
    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=False, padding_side="left")
    model = AutoModel.from_pretrained(
        args.model,
        trust_remote_code=False,
        torch_dtype=torch.bfloat16,
        attn_implementation="sdpa",
    ).to("cuda")
    model.eval()
    load_seconds = time.perf_counter() - started

    encoded = tokenizer(
        [query, document],
        padding=True,
        truncation=True,
        max_length=32768,
        return_tensors="pt",
    ).to("cuda")
    inference_started = time.perf_counter()
    with torch.inference_mode():
        outputs = model(**encoded)
        pooled = last_token_pool(outputs.last_hidden_state, encoded["attention_mask"])
        embeddings = F.normalize(pooled, p=2, dim=1).float().cpu().tolist()
    inference_seconds = time.perf_counter() - inference_started

    cases = []
    for index, kind in enumerate(("query", "document")):
        native = embeddings[index]
        reduced = truncate_normalize(native, 1536)
        for suffix, observed in (("native", native), ("1536", reduced)):
            expected = vectors[f"qwen3-embed-{kind}-{suffix}"]
            cases.append({
                "id": f"qwen3-embed-{kind}-{suffix}",
                "dimensions": len(observed),
                "cosine": cosine(expected, observed),
                "reference_norm": math.sqrt(sum(value * value for value in observed)),
                "vllm_norm": math.sqrt(sum(value * value for value in expected)),
            })

    report = {
        "schema_version": 1,
        "model_path": args.model,
        "reference_runtime": "transformers-official-last-token-pooling",
        "dtype": "bfloat16",
        "attention": "sdpa",
        "load_seconds": load_seconds,
        "inference_seconds": inference_seconds,
        "cases": cases,
        "minimum_cosine": min(case["cosine"] for case in cases),
    }
    Path(args.output).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
