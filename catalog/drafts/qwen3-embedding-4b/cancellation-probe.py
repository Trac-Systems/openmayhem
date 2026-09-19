#!/usr/bin/env python3
"""Focused vLLM pooling cancellation and post-abort recovery proof."""

import argparse
import asyncio
import json
import os
import time
import uuid
from pathlib import Path

from vllm import AsyncEngineArgs, PoolingParams
from vllm.v1.engine.async_llm import AsyncLLM


async def encode(
    engine,
    text: str,
    request_id: str,
    cancelled: asyncio.Event | None = None,
) -> int:
    final = None
    async for output in engine.encode(
        prompt=text,
        pooling_params=PoolingParams(task="embed", dimensions=None),
        request_id=request_id,
    ):
        if cancelled is not None and cancelled.is_set():
            raise asyncio.CancelledError("caller cancelled embedding request")
        final = output
    if final is None or not final.finished:
        raise RuntimeError("embedding ended without a final result")
    return len(final.prompt_token_ids)


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

    engine = AsyncLLM.from_engine_args(AsyncEngineArgs(
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
        gpu_memory_utilization=0.13,
        limit_mm_per_prompt={"image": 1, "audio": 1, "video": 1},
        mm_processor_cache_gb=0,
    ))
    request_id = f"cancel-{uuid.uuid4().hex}"
    cancelled = asyncio.Event()
    task = asyncio.create_task(
        encode(engine, " cancel" * 32700, request_id, cancelled)
    )
    await asyncio.sleep(0.5)
    done_before_abort = task.done()
    started = time.perf_counter()
    cancelled.set()
    await engine.abort(request_id)
    outcome = {"aborted": False, "done_before_abort": done_before_abort}
    try:
        await asyncio.wait_for(task, timeout=5)
        outcome["unexpected"] = "completed"
    except BaseException as error:
        outcome.update({
            "aborted": True,
            "type": type(error).__name__,
            "message": str(error)[:500],
        })
    outcome["abort_seconds"] = time.perf_counter() - started

    recovery_started = time.perf_counter()
    recovery_tokens = await asyncio.wait_for(
        encode(engine, "Recovery after cancellation.", f"recovery-{uuid.uuid4().hex}"),
        timeout=10,
    )
    outcome["recovery_seconds"] = time.perf_counter() - recovery_started
    outcome["recovery_tokens"] = recovery_tokens
    outcome["passed"] = (
        outcome["aborted"]
        and not outcome["done_before_abort"]
        and outcome["abort_seconds"] < 5
        and recovery_tokens > 0
    )
    Path(args.output).write_text(json.dumps(outcome, indent=2) + "\n")
    shutdown = getattr(engine, "shutdown", None)
    if callable(shutdown):
        shutdown()
    print(json.dumps(outcome))
    if not outcome["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    asyncio.run(main())
