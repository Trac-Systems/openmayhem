#!/usr/bin/env python3
"""Compare Qwen3 Embedding 4B and text-embedding-3-small at 1536 dimensions."""

import argparse
import hashlib
import json
import math
import os
import urllib.request
from pathlib import Path


QUERY_INSTRUCTION = (
    "Instruct: Given a user-memory or retrieval query, retrieve relevant passages "
    "that answer the query\nQuery:"
)


def post_embeddings(url: str, key: str, model: str, inputs: list[str]) -> tuple[list[list[float]], dict]:
    body = json.dumps({
        "model": model,
        "input": inputs,
        "dimensions": 1536,
        "encoding_format": "float",
    }).encode()
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        payload = json.load(response)
    rows = sorted(payload["data"], key=lambda row: row["index"])
    vectors = [[float(value) for value in row["embedding"]] for row in rows]
    if len(vectors) != len(inputs) or any(len(vector) != 1536 for vector in vectors):
        raise RuntimeError("embedding response has the wrong item count or dimensions")
    return vectors, payload.get("usage") or {}


def cosine(left: list[float], right: list[float]) -> float:
    dot = sum(a * b for a, b in zip(left, right))
    ln = math.sqrt(sum(value * value for value in left))
    rn = math.sqrt(sum(value * value for value in right))
    return dot / (ln * rn)


def metrics(queries: list[dict], query_vectors: list[list[float]], documents: list[dict], document_vectors: list[list[float]]) -> dict:
    recalls = {1: 0.0, 3: 0.0, 5: 0.0}
    reciprocal_ranks = []
    ndcgs = []
    per_query = []
    for query, vector in zip(queries, query_vectors):
        ranked = sorted(
            zip(documents, document_vectors),
            key=lambda row: cosine(vector, row[1]),
            reverse=True,
        )
        ids = [document["id"] for document, _ in ranked]
        relevant = set(query["relevant"])
        ranks = [index + 1 for index, item_id in enumerate(ids) if item_id in relevant]
        for k in recalls:
            recalls[k] += len(relevant.intersection(ids[:k])) / len(relevant)
        reciprocal_ranks.append(1.0 / min(ranks) if ranks else 0.0)
        dcg = sum(1.0 / math.log2(rank + 1) for rank in ranks if rank <= 10)
        ideal = sum(1.0 / math.log2(rank + 1) for rank in range(1, min(len(relevant), 10) + 1))
        ndcgs.append(dcg / ideal if ideal else 0.0)
        per_query.append({"id": query["id"], "first_relevant_rank": min(ranks) if ranks else None})
    count = len(queries)
    return {
        "recall_at_1": recalls[1] / count,
        "recall_at_3": recalls[3] / count,
        "recall_at_5": recalls[5] / count,
        "mrr_at_10": sum(value if value >= 0.1 else 0.0 for value in reciprocal_ranks) / count,
        "ndcg_at_10": sum(ndcgs) / count,
        "per_query": per_query,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--suite", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--qwen-url", required=True)
    parser.add_argument("--qwen-model", default="Qwen/Qwen3-Embedding-4B")
    parser.add_argument("--openai-url", default="https://api.openai.com/v1/embeddings")
    parser.add_argument("--openai-model", default="text-embedding-3-small")
    args = parser.parse_args()
    qwen_key = os.environ.get("QWEN_API_KEY")
    openai_key = os.environ.get("OPENAI_API_KEY")
    if not qwen_key or not openai_key:
        raise SystemExit("QWEN_API_KEY and OPENAI_API_KEY must be set")

    suite_path = Path(args.suite)
    suite_bytes = suite_path.read_bytes()
    suite = json.loads(suite_bytes)
    documents = suite["documents"]
    queries = suite["queries"]
    document_texts = [row["text"] for row in documents]
    query_texts = [row["text"] for row in queries]

    qwen_documents, qwen_doc_usage = post_embeddings(
        args.qwen_url, qwen_key, args.qwen_model, document_texts
    )
    qwen_plain_queries, qwen_plain_usage = post_embeddings(
        args.qwen_url, qwen_key, args.qwen_model, query_texts
    )
    qwen_instructed_queries, qwen_instructed_usage = post_embeddings(
        args.qwen_url,
        qwen_key,
        args.qwen_model,
        [QUERY_INSTRUCTION + text for text in query_texts],
    )
    openai_documents, openai_doc_usage = post_embeddings(
        args.openai_url, openai_key, args.openai_model, document_texts
    )
    openai_queries, openai_query_usage = post_embeddings(
        args.openai_url, openai_key, args.openai_model, query_texts
    )

    qwen_plain = metrics(queries, qwen_plain_queries, documents, qwen_documents)
    qwen_instructed = metrics(queries, qwen_instructed_queries, documents, qwen_documents)
    openai = metrics(queries, openai_queries, documents, openai_documents)
    aggregate_keys = ("recall_at_1", "recall_at_3", "recall_at_5", "mrr_at_10", "ndcg_at_10")
    report = {
        "schema_version": 1,
        "suite_sha256": hashlib.sha256(suite_bytes).hexdigest(),
        "dimensions": 1536,
        "query_instruction": QUERY_INSTRUCTION,
        "models": {
            "qwen_plain": {"model": args.qwen_model, "metrics": qwen_plain, "usage": {"documents": qwen_doc_usage, "queries": qwen_plain_usage}},
            "qwen_instructed": {"model": args.qwen_model, "metrics": qwen_instructed, "usage": {"documents": qwen_doc_usage, "queries": qwen_instructed_usage}},
            "openai": {"model": args.openai_model, "metrics": openai, "usage": {"documents": openai_doc_usage, "queries": openai_query_usage}},
        },
        "aggregate_delta_qwen_instructed_minus_openai": {
            key: qwen_instructed[key] - openai[key] for key in aggregate_keys
        },
    }
    report["qwen_matches_or_beats_openai_aggregate"] = (
        sum(qwen_instructed[key] for key in aggregate_keys)
        >= sum(openai[key] for key in aggregate_keys)
    )
    Path(args.output).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({
        "qwen_instructed": {key: qwen_instructed[key] for key in aggregate_keys},
        "qwen_plain": {key: qwen_plain[key] for key in aggregate_keys},
        "openai": {key: openai[key] for key in aggregate_keys},
        "qwen_matches_or_beats_openai_aggregate": report["qwen_matches_or_beats_openai_aggregate"],
    }))


if __name__ == "__main__":
    main()
