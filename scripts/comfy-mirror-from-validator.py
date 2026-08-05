#!/usr/bin/env python3
"""Download and normalize Comfy part payloads from a validator report.

The input is the JSON emitted by:

  mayhem admin parts validate-yaml --include-drafts ...

This runner is intentionally admission-side tooling. It downloads only rows
that are already importable by the validator, verifies the declared source
SHA-256, and converts pickle-backed tensor checkpoints into safetensors before
they can become provider-facing payloads.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urljoin, urlparse
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen


DIRECT_FORMATS = {"safetensors", "gguf"}
PICKLE_MARKER = "pickle"
CHUNK_BYTES = 4 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


NO_REDIRECT_OPENER = build_opener(NoRedirect)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(CHUNK_BYTES), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_token(path: Path | None) -> str | None:
    if path is None:
        return None
    try:
        token = path.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return None
    return token or None


def safe_component(value: str, fallback: str) -> str:
    value = value.strip() or fallback
    value = re.sub(r"[^A-Za-z0-9._+-]+", "-", value)
    value = value.strip(".-")
    return value[:120] or fallback


def source_candidates(draft: dict[str, Any]) -> list[dict[str, Any]]:
    sources = draft.get("sources") or {}
    # For mirroring, prefer canonical origins. Existing mirrors are fallback
    # inputs only when a row has no origin block.
    return list(sources.get("origins") or []) + list(sources.get("mirrors") or [])


def choose_source(draft: dict[str, Any]) -> dict[str, Any] | None:
    for source in source_candidates(draft):
        if source.get("url"):
            return source
    return None


def headers_for(source: dict[str, Any], hf_token: str | None, civitai_token: str | None) -> dict[str, str]:
    headers = {"User-Agent": "openmayhem-comfy-mirror/1"}
    kind = str(source.get("kind") or "").lower()
    if kind == "huggingface" and hf_token:
        headers["Authorization"] = f"Bearer {hf_token}"
    if kind == "civitai" and civitai_token:
        headers["Authorization"] = f"Bearer {civitai_token}"
    return headers


def verify_existing(path: Path, expected_sha256: str, expected_size: int) -> bool:
    if not path.is_file():
        return False
    if path.stat().st_size != expected_size:
        return False
    return sha256_file(path) == expected_sha256


def download_once(url: str, destination: Path, headers: dict[str, str], timeout: int) -> None:
    partial = destination.with_name(f"{destination.name}.part")
    partial.parent.mkdir(parents=True, exist_ok=True)
    offset = partial.stat().st_size if partial.exists() else 0
    request_headers = dict(headers)
    if offset:
        request_headers["Range"] = f"bytes={offset}-"
    with open_following_redirects(url, request_headers, timeout) as response:
        status = getattr(response, "status", response.getcode())
        mode = "ab" if offset and status == 206 else "wb"
        if offset and status != 206:
            offset = 0
        with partial.open(mode + "") as output:
            while True:
                chunk = response.read(CHUNK_BYTES)
                if not chunk:
                    break
                output.write(chunk)
    os.replace(partial, destination)


def open_following_redirects(url: str, headers: dict[str, str], timeout: int):
    current_url = url
    current_headers = dict(headers)
    for _ in range(10):
        request = Request(current_url, headers=current_headers)
        try:
            return NO_REDIRECT_OPENER.open(request, timeout=timeout)
        except HTTPError as error:
            if error.code not in (301, 302, 303, 307, 308):
                raise
            location = error.headers.get("Location")
            if not location:
                raise
            if error.fp is not None:
                error.fp.close()
            next_url = urljoin(current_url, location)
            if urlparse(next_url).netloc.lower() != urlparse(current_url).netloc.lower():
                current_headers = {
                    key: value
                    for key, value in current_headers.items()
                    if key.lower() != "authorization"
                }
            current_url = next_url
    raise RuntimeError(f"too many redirects while downloading {url}")


def download_verified(
    url: str,
    destination: Path,
    headers: dict[str, str],
    expected_sha256: str,
    expected_size: int,
    retries: int,
    timeout: int,
) -> None:
    if verify_existing(destination, expected_sha256, expected_size):
        return
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            download_once(url, destination, headers, timeout)
            actual_size = destination.stat().st_size
            if actual_size != expected_size:
                raise RuntimeError(f"size mismatch: expected {expected_size}, got {actual_size}")
            actual_sha256 = sha256_file(destination)
            if actual_sha256 != expected_sha256:
                raise RuntimeError(f"sha256 mismatch: expected {expected_sha256}, got {actual_sha256}")
            return
        except (HTTPError, URLError, OSError, RuntimeError) as error:
            last_error = error
            if attempt == retries:
                break
            time.sleep(min(30, 2 ** attempt))
    raise RuntimeError(f"download failed after {retries + 1} attempt(s): {last_error}")


def convert_pickle(
    python: str,
    converter: Path,
    source: Path,
    output: Path,
    force: bool,
) -> dict[str, Any]:
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists() and not force:
        return {
            "ok": True,
            "input": str(source),
            "output": str(output),
            "input_sha256": sha256_file(source),
            "output_sha256": sha256_file(output),
            "input_bytes": source.stat().st_size,
            "output_bytes": output.stat().st_size,
            "file_format": "safetensors",
            "reused_existing": True,
        }
    command = [
        python,
        str(converter),
        "--input",
        str(source),
        "--output",
        str(output),
        "--force",
    ]
    completed = subprocess.run(command, check=True, text=True, capture_output=True)
    return json.loads(completed.stdout)


def classify(draft: dict[str, Any]) -> str:
    file_format = str(draft.get("file_format") or "").lower()
    if file_format in DIRECT_FORMATS:
        return "direct"
    if PICKLE_MARKER in file_format:
        return "pickle_convert"
    return "unknown_hold"


def result_line(result: dict[str, Any]) -> str:
    return json.dumps(result, sort_keys=True, separators=(",", ":"))


def main() -> int:
    parser = argparse.ArgumentParser(description="Mirror Comfy validator drafts into a staging directory.")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--converter", type=Path, default=Path("scripts/comfy-convert-pickle-to-safetensors.py"))
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--hf-token-file", type=Path)
    parser.add_argument("--civitai-token-file", type=Path)
    parser.add_argument("--max-count", type=int, default=0, help="Process at most N actionable rows; 0 means all.")
    parser.add_argument("--only-part-id", action="append", default=[])
    parser.add_argument("--include-unknown", action="store_true", help="Download unknown-format rows too.")
    parser.add_argument("--worker-count", type=int, default=1, help="Shard actionable rows across N workers.")
    parser.add_argument("--worker-index", type=int, default=0, help="Zero-based shard index for this worker.")
    parser.add_argument("--results-name", default="mirror-results.jsonl", help="Result JSONL filename under results/.")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--force-convert", action="store_true")
    parser.add_argument("--retries", type=int, default=4)
    parser.add_argument("--timeout-seconds", type=int, default=90)
    args = parser.parse_args()
    if args.worker_count < 1:
        raise SystemExit("--worker-count must be at least 1")
    if args.worker_index < 0 or args.worker_index >= args.worker_count:
        raise SystemExit("--worker-index must be in [0, --worker-count)")

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    drafts = manifest.get("drafts") or []
    output_dir = args.output_dir.expanduser().resolve()
    downloads_dir = output_dir / "downloads"
    converted_dir = output_dir / "converted"
    results_dir = output_dir / "results"
    results_dir.mkdir(parents=True, exist_ok=True)
    results_path = results_dir / safe_component(args.results_name, "mirror-results.jsonl")
    hf_token = read_token(args.hf_token_file)
    civitai_token = read_token(args.civitai_token_file)
    selected_ids = set(args.only_part_id)
    counts: dict[str, int] = {}
    failures = 0
    processed = 0
    actionable_index = 0

    with results_path.open("a", encoding="utf-8") as results:
        for draft in drafts:
            part_id = str(draft.get("part_id") or "")
            if selected_ids and part_id not in selected_ids:
                continue
            category = classify(draft)
            counts[category] = counts.get(category, 0) + 1
            if category == "unknown_hold" and not args.include_unknown:
                if args.worker_index == 0:
                    results.write(
                        result_line({"ok": True, "part_id": part_id, "status": "skipped_unknown"})
                        + "\n"
                    )
                continue
            if category == "unknown_hold" and args.include_unknown:
                category = "direct_unknown"
            current_actionable_index = actionable_index
            actionable_index += 1
            if current_actionable_index % args.worker_count != args.worker_index:
                continue
            if args.max_count and processed >= args.max_count:
                continue
            source = choose_source(draft)
            if not source:
                failures += 1
                results.write(result_line({"ok": False, "part_id": part_id, "status": "no_source"}) + "\n")
                continue
            name = safe_component(str(draft.get("name") or ""), "part")
            source_name = Path(urlparse(str(source["url"])).path).name or f"{part_id}.payload"
            source_name = safe_component(source_name, "payload")
            download_path = downloads_dir / part_id / source_name
            expected_sha256 = str(draft["sha256"])
            expected_size = int(draft["size_bytes"])
            record: dict[str, Any] = {
                "ok": True,
                "part_id": part_id,
                "name": name,
                "status": "dry_run" if args.dry_run else "mirrored",
                "category": category,
                "source_kind": source.get("kind"),
                "download_path": str(download_path),
                "expected_sha256": expected_sha256,
                "expected_size_bytes": expected_size,
                "worker_count": args.worker_count,
                "worker_index": args.worker_index,
            }
            try:
                if not args.dry_run:
                    headers = headers_for(source, hf_token, civitai_token)
                    download_verified(
                        str(source["url"]),
                        download_path,
                        headers,
                        expected_sha256,
                        expected_size,
                        args.retries,
                        args.timeout_seconds,
                    )
                    if category == "pickle_convert":
                        converted_path = converted_dir / f"{part_id}.safetensors"
                        converted = convert_pickle(
                            args.python,
                            args.converter,
                            download_path,
                            converted_path,
                            args.force_convert,
                        )
                        record["status"] = "converted"
                        record["converted_path"] = str(converted_path)
                        record["converted_sha256"] = converted["output_sha256"]
                        record["converted_size_bytes"] = converted["output_bytes"]
                        record["tensor_count"] = converted.get("tensor_count")
                processed += 1
            except Exception as error:  # noqa: BLE001 - result manifest needs the exact failing row.
                failures += 1
                record = {
                    "ok": False,
                    "part_id": part_id,
                    "name": name,
                    "status": "failed",
                    "category": category,
                    "source_kind": source.get("kind"),
                    "error": str(error),
                    "worker_count": args.worker_count,
                    "worker_index": args.worker_index,
                }
            results.write(result_line(record) + "\n")
            results.flush()

    summary = {
        "ok": failures == 0,
        "manifest": str(args.manifest),
        "output_dir": str(output_dir),
        "processed": processed,
        "failures": failures,
        "counts": counts,
        "results": str(results_path),
        "dry_run": args.dry_run,
        "worker_count": args.worker_count,
        "worker_index": args.worker_index,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
