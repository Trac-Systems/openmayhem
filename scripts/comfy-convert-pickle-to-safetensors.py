#!/usr/bin/env python3
"""Convert a PyTorch tensor/state-dict checkpoint into safetensors for Comfy parts.

This is an admission-time helper. It deliberately uses torch.load(...,
weights_only=True) and refuses arbitrary non-tensor objects, so unsafe pickle
payloads never become provider-pulled catalog bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unwrap_state_dict(value: Any) -> Any:
    if not isinstance(value, dict):
        return value
    for key in ("state_dict", "model_state_dict", "params", "model"):
        nested = value.get(key)
        if isinstance(nested, dict):
            return nested
    if len(value) == 1:
        nested = next(iter(value.values()))
        if isinstance(nested, dict):
            return nested
    return value


def collect_tensors(value: Any, prefix: str, output: dict[str, Any], torch: Any) -> None:
    if isinstance(value, torch.nn.Parameter):
        value = value.detach()
    if torch.is_tensor(value):
        key = prefix.strip(".")
        if not key:
            raise ValueError("tensor key is empty")
        if value.is_sparse:
            raise ValueError(f"tensor {key!r} is sparse; safetensors requires dense tensors")
        if key in output:
            raise ValueError(f"duplicate tensor key after flattening: {key}")
        output[key] = value.detach().cpu().contiguous().clone()
        return
    if isinstance(value, dict):
        for key in sorted(value.keys(), key=str):
            text_key = str(key).strip()
            if not text_key:
                raise ValueError("state dict contains an empty key")
            collect_tensors(value[key], f"{prefix}.{text_key}" if prefix else text_key, output, torch)
        return
    if isinstance(value, (list, tuple)):
        for index, item in enumerate(value):
            collect_tensors(item, f"{prefix}.{index}" if prefix else str(index), output, torch)
        return
    if value is None or isinstance(value, (bool, int, float, str, bytes)):
        return
    raise ValueError(f"unsupported non-tensor value at {prefix or '<root>'}: {type(value).__name__}")


def atomic_replace(temp: Path, output: Path) -> None:
    with temp.open("rb") as handle:
        os.fsync(handle.fileno())
    os.replace(temp, output)
    directory = os.open(str(output.parent), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Convert a PyTorch pickle checkpoint into a safetensors payload."
    )
    parser.add_argument("--input", required=True, type=Path, help="Source .pth/.pt/.bin checkpoint")
    parser.add_argument("--output", required=True, type=Path, help="Destination .safetensors file")
    parser.add_argument("--force", action="store_true", help="Replace an existing output file")
    args = parser.parse_args()

    source = args.input.expanduser().resolve()
    output = args.output.expanduser().resolve()
    if not source.is_file():
        raise SystemExit(f"input is not a regular file: {source}")
    if output.suffix.lower() not in (".safetensors", ".sft"):
        raise SystemExit("output must end with .safetensors or .sft")
    if output.exists() and not args.force:
        raise SystemExit(f"output already exists; pass --force to replace: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)

    import safetensors.torch
    import torch

    source_sha256 = sha256_file(source)
    loaded = torch.load(source, map_location="cpu", weights_only=True)
    state = unwrap_state_dict(loaded)
    tensors: dict[str, Any] = {}
    collect_tensors(state, "", tensors, torch)
    if not tensors:
        raise SystemExit("source did not contain any tensor weights")

    temp = output.with_name(f".{output.name}.part-{os.getpid()}")
    if temp.exists():
        temp.unlink()
    metadata = {
        "format": "pt-state-dict",
        "converted_by": "openmayhem-comfy-admission",
        "source_sha256": source_sha256,
        "source_name": source.name,
    }
    safetensors.torch.save_file(tensors, str(temp), metadata=metadata)
    atomic_replace(temp, output)

    report = {
        "ok": True,
        "input": str(source),
        "output": str(output),
        "input_sha256": source_sha256,
        "output_sha256": sha256_file(output),
        "input_bytes": source.stat().st_size,
        "output_bytes": output.stat().st_size,
        "tensor_count": len(tensors),
        "file_format": "safetensors",
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
