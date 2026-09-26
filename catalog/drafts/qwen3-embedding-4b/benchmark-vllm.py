#!/usr/bin/env python3
"""Bounded Qwen3 embedding calibration benchmark for the pinned vLLM runtime."""

import argparse
import asyncio
import json
import math
import os
import statistics
import time
import uuid
from pathlib import Path

import psutil
from transformers import AutoTokenizer
from vllm import AsyncEngineArgs, PoolingParams
from vllm.v1.engine.async_llm import AsyncLLM


def process_tree_rss() -> int:
    root = psutil.Process(os.getpid())
    processes = [root, *root.children(recursive=True)]
    total = 0
    for process in processes:
        try:
            total += process.memory_info().rss
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    return total


def memory_snapshot() -> dict:
    memory = psutil.virtual_memory()
    swap = psutil.swap_memory()
    return {
        "process_tree_rss_bytes": process_tree_rss(),
        "system_available_bytes": memory.available,
        "system_used_bytes": memory.used,
        "swap_used_bytes": swap.used,
    }


def normalized_prefix(vector, dimensions: int) -> list[float]:
    values = [float(value) for value in vector[:dimensions]]
    norm = math.sqrt(sum(value * value for value in values))
    if not math.isfinite(norm) or norm <= 0:
        raise RuntimeError("embedding prefix has no finite norm")
    return [value / norm for value in values]


def vector_from_output(output) -> list[float]:
    values = getattr(output.outputs, "embedding", None)
    if values is None:
        values = getattr(output.outputs, "data", None)
    if hasattr(values, "detach"):
        values = values.detach().float().cpu().tolist()
    return [float(value) for value in values]


async def encode_one(engine, text: str, request_id: str):
    final = None
    async for output in engine.encode(
        prompt=text,
        pooling_params=PoolingParams(task="embed", dimensions=None),
        request_id=request_id,
    ):
        final = output
    if final is None or not final.finished:
        raise RuntimeError("embedding ended without a final result")
    vector = vector_from_output(final)
    return vector, len(final.prompt_token_ids)


async def measured_batch(engine, texts: list[str], label: str) -> dict:
    started = time.perf_counter()
    rows = await asyncio.gather(*[
        encode_one(engine, text, f"{label}-{index}-{uuid.uuid4().hex}")
        for index, text in enumerate(texts)
    ])
    elapsed = time.perf_counter() - started
    tokens = sum(row[1] for row in rows)
    vectors = [normalized_prefix(row[0], 1536) for row in rows]
    return {
        "label": label,
        "items": len(rows),
        "tokens": tokens,
        "elapsed_seconds": elapsed,
        "items_per_second": len(rows) / elapsed,
        "tokens_per_second": tokens / elapsed,
        "native_dimensions": len(rows[0][0]),
        "requested_dimensions": len(vectors[0]),
        "norm_min": min(math.sqrt(sum(v * v for v in row)) for row in vectors),
        "norm_max": max(math.sqrt(sum(v * v for v in row)) for row in vectors),
        "process_tree_rss_bytes": process_tree_rss(),
    }


def percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    index = math.ceil(quantile * len(ordered)) - 1
    return ordered[max(0, min(index, len(ordered) - 1))]


async def repeated_batches(engine, texts: list[str], label: str, repetitions: int) -> dict:
    rows = []
    for repetition in range(repetitions):
        rows.append(await measured_batch(engine, texts, f"{label}-{repetition}"))
    latencies = [row["elapsed_seconds"] for row in rows]
    tokens = sum(row["tokens"] for row in rows)
    elapsed = sum(latencies)
    return {
        "label": label,
        "repetitions": repetitions,
        "items_per_request": len(texts),
        "total_items": repetitions * len(texts),
        "total_tokens": tokens,
        "elapsed_seconds": elapsed,
        "requests_per_second": repetitions / elapsed,
        "tokens_per_second": tokens / elapsed,
        "latency_p50_seconds": statistics.median(latencies),
        "latency_p90_seconds": percentile(latencies, 0.90),
        "latency_p99_seconds": percentile(latencies, 0.99),
        "memory": memory_snapshot(),
    }


def sized_text(tokenizer, target_tokens: int, marker: str) -> str:
    unit = f" {marker} retrieval passage evidence"
    text = unit * max(1, target_tokens // 4)
    for _ in range(8):
        count = len(tokenizer.encode(text, add_special_tokens=False))
        if target_tokens - 4 <= count <= target_tokens:
            return text
        if count > target_tokens:
            text = text[: max(1, int(len(text) * target_tokens / count * 0.998))]
        else:
            text += unit * max(1, (target_tokens - count) // 4)
    while len(tokenizer.encode(text, add_special_tokens=False)) > target_tokens:
        text = text[:-1]
    return text


def sized_char_text(target_bytes: int, marker: str) -> str:
    unit = f" {marker} retrieval evidence"
    return (unit * (target_bytes // len(unit) + 1))[:target_bytes]


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--memory-utilization", type=float, default=0.12)
    args = parser.parse_args()

    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    baseline_memory = memory_snapshot()
    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=False)
    load_started = time.perf_counter()
    engine_args = AsyncEngineArgs(
        model=args.model,
        tokenizer=args.model,
        trust_remote_code=False,
        max_model_len=32768,
        max_num_seqs=8,
        max_num_batched_tokens=32768,
        tensor_parallel_size=1,
        enforce_eager=True,
        seed=0,
        use_fp64_gumbel=True,
        async_scheduling=False,
        runner="pooling",
        convert="embed",
        enable_prefix_caching=True,
        dtype="bfloat16",
        gpu_memory_utilization=args.memory_utilization,
        limit_mm_per_prompt={"image": 1, "audio": 1, "video": 1},
        mm_processor_cache_gb=0,
    )
    engine = AsyncLLM.from_engine_args(engine_args)
    loaded_seconds = time.perf_counter() - load_started
    loaded_memory = memory_snapshot()
    results = []
    errors = []
    try:
        await measured_batch(engine, ["warm embedding request"], "warmup")
        short = "A compact passage about vector search and retrieval quality."
        for count in (1, 8, 32):
            results.append(await measured_batch(
                engine,
                [f"{short} Item {index}." for index in range(count)],
                f"batch-{count}-short",
            ))
        text512 = sized_text(tokenizer, 512, "medium")
        results.append(await measured_batch(
            engine,
            [f"{text512} batch item {index}" for index in range(8)],
            "batch-8-512-token",
        ))
        for character_count in (256, 1024, 4096):
            text = sized_char_text(character_count, f"chars-{character_count}")
            for batch_size in (1, 8, 32):
                results.append(await measured_batch(
                    engine,
                    [f"{text} item {index}" for index in range(batch_size)],
                    f"batch-{batch_size}-{character_count}-chars",
                ))

        text256 = sized_char_text(256, "concurrent")
        for concurrency in (1, 2, 4, 8):
            started = time.perf_counter()
            concurrent = await asyncio.gather(*[
                measured_batch(
                    engine,
                    [f"{text256} request {request} item {index}" for index in range(8)],
                    f"concurrent-{concurrency}-{request}-batch-8",
                )
                for request in range(concurrency)
            ])
            elapsed = time.perf_counter() - started
            tokens = sum(row["tokens"] for row in concurrent)
            items = sum(row["items"] for row in concurrent)
            results.append({
                "label": f"concurrent-{concurrency}x-batch-8-256-chars",
                "requests": concurrency,
                "items": items,
                "tokens": tokens,
                "elapsed_seconds": elapsed,
                "requests_per_second": concurrency / elapsed,
                "items_per_second": items / elapsed,
                "tokens_per_second": tokens / elapsed,
                "memory": memory_snapshot(),
            })

        results.append(await repeated_batches(
            engine,
            [short],
            "sustained-batch-1-short",
            50,
        ))
        results.append(await repeated_batches(
            engine,
            [f"{short} sustained item {index}" for index in range(8)],
            "sustained-batch-8-short",
            40,
        ))
        results.append(await repeated_batches(
            engine,
            [f"{short} sustained item {index}" for index in range(32)],
            "sustained-batch-32-short",
            20,
        ))

        # vLLM appends one model token, leaving 32,767 caller-supplied tokens in
        # the checkpoint's 32,768-token context window.
        long_text = " x" * 32767
        results.append(await measured_batch(engine, [long_text], "single-max-input"))

        overflow_text = " x" * 32768
        try:
            await measured_batch(engine, [overflow_text], "single-overflow")
            errors.append({"label": "overflow", "unexpected": "accepted"})
        except Exception as error:
            errors.append({
                "label": "overflow",
                "expected_rejection": True,
                "type": type(error).__name__,
                "message": str(error)[:500],
            })

        blocker_id = f"cancel-blocker-{uuid.uuid4().hex}"
        cancel_id = f"cancel-target-{uuid.uuid4().hex}"
        blocker_text = " blocker" * 32700
        cancel_text = " target" * 32700
        blocker_task = asyncio.create_task(encode_one(engine, blocker_text, blocker_id))
        cancel_task = asyncio.create_task(encode_one(engine, cancel_text, cancel_id))
        await asyncio.sleep(0.01)
        cancel_started = time.perf_counter()
        await engine.abort(cancel_id)
        try:
            await asyncio.wait_for(cancel_task, timeout=20)
            errors.append({"label": "cancel", "unexpected": "completed"})
        except BaseException as error:
            errors.append({
                "label": "cancel",
                "expected_abort": True,
                "type": type(error).__name__,
                "message": str(error)[:500],
                "abort_seconds": time.perf_counter() - cancel_started,
            })
        await asyncio.wait_for(blocker_task, timeout=30)
        results.append(await measured_batch(
            engine,
            ["Recovery request after a cancelled long embedding."],
            "post-cancel-recovery",
        ))
    finally:
        shutdown = getattr(engine, "shutdown", None)
        if callable(shutdown):
            shutdown()

    report = {
        "schema_version": 1,
        "model_path": args.model,
        "runtime": "vllm-0.24.0",
        "memory_utilization": args.memory_utilization,
        "max_model_len": 32768,
        "max_num_seqs": 8,
        "max_num_batched_tokens": 32768,
        "load_seconds": loaded_seconds,
        "baseline_memory": baseline_memory,
        "loaded_memory": loaded_memory,
        "final_memory": memory_snapshot(),
        "results": results,
        "boundary_results": errors,
    }
    Path(args.output).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({
        "output": args.output,
        "load_seconds": loaded_seconds,
        "result_count": len(results),
        "boundary_results": errors,
    }))


if __name__ == "__main__":
    asyncio.run(main())
