#!/usr/bin/env python3
"""Prepare provider-facing Comfy corpus records from verified staging results.

This script joins the current validator allowlist with verified download/convert
results, enforces the W2.3 mirror-vs-origin split, and builds a hardlink upload
tree for mirrorable provider-facing payloads. It does not upload, sign, submit,
or touch any live node.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
from pathlib import Path
from typing import Any


DEFAULT_ORIGIN_ONLY_NAMES = {
    "PiDiNet table5",
    "Fooocus LaMa (object removal)",
    "Fooocus inpaint head",
}


def safe_component(value: str, fallback: str = "part") -> str:
    value = re.sub(r"[^A-Za-z0-9._+-]+", "-", value.strip())
    value = value.strip(".-")[:120]
    return value or fallback


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def extension_for(file_format: str, payload: Path) -> str:
    ext = payload.suffix.lower().lstrip(".")
    if ext in {"safetensors", "gguf"}:
        return ext
    normalized = file_format.lower()
    if "safetensors" in normalized:
        return "safetensors"
    if "gguf" in normalized:
        return "gguf"
    return "bin"


def draft_sources_for_output(
    draft: dict[str, Any],
    mirror: dict[str, str] | None,
) -> dict[str, Any]:
    sources = dict(draft.get("sources") or {})
    if mirror is not None:
        mirrors = list(sources.get("mirrors") or [])
        if not any(existing.get("url") == mirror["url"] for existing in mirrors):
            mirrors.insert(0, mirror)
        sources["mirrors"] = mirrors
    if sources.get("mirrors"):
        sources["mirrors"] = sorted(sources["mirrors"], key=lambda item: item["url"])
    if sources.get("origins"):
        sources["origins"] = sorted(sources["origins"], key=lambda item: item["url"])
    return sources


def provider_draft(
    draft: dict[str, Any],
    result: dict[str, Any],
    mirror: dict[str, str] | None,
) -> dict[str, Any]:
    converted = result.get("status") == "converted"
    sha256 = result.get("converted_sha256") if converted else draft["sha256"]
    size_bytes = result.get("converted_size_bytes") if converted else result["actual_size_bytes"]
    file_format = "safetensors" if converted else draft["file_format"]
    adapter = dict(draft.get("adapter") or {})
    if converted:
        adapter["source_payload"] = {
            "sha256": draft["sha256"],
            "size_bytes": draft["size_bytes"],
            "file_format": draft["file_format"],
            "conversion": "pickle_to_safetensors",
        }
    out = {
        "name": draft["name"],
        "type": draft["type"],
        "lane": draft["lane"],
        "sha256": sha256,
        "size_bytes": size_bytes,
        "size_bytes_exact": True,
        "file_format": file_format,
        "license": draft["license"],
        "permissions": draft.get("permissions") or [],
        "policy_flags": draft.get("policy_flags") or [],
        "adapter": adapter,
        "sources": draft_sources_for_output(draft, mirror),
        "status": draft.get("status") or "linked",
    }
    return out


def mirror_source(repo: str, revision: str, repo_path: str) -> dict[str, str]:
    return {
        "kind": "huggingface",
        "url": f"https://huggingface.co/datasets/{repo}/resolve/{revision}/{repo_path}",
        "repository": repo,
        "path": repo_path,
        "revision": revision,
    }


def hardlink_payload(source: Path, destination: Path, force: bool) -> None:
    if destination.exists():
        if not force:
            raise FileExistsError(destination)
        destination.unlink()
    destination.parent.mkdir(parents=True, exist_ok=True)
    os.link(source, destination)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validator-report", required=True, type=Path)
    parser.add_argument("--results-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--payload-upload-dir", required=True, type=Path)
    parser.add_argument("--mirror-repo", default="TracNetwork/openmayhem-parts-index")
    parser.add_argument("--mirror-revision")
    parser.add_argument("--mirror-prefix", default="payloads/w23/sha256")
    parser.add_argument("--origin-only-name", action="append", default=[])
    parser.add_argument("--origin-only-part-id", action="append", default=[])
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()

    manifest = json.loads(args.validator_report.read_text(encoding="utf-8"))
    drafts = {draft["part_id"]: draft for draft in manifest.get("drafts") or []}
    results: dict[str, dict[str, Any]] = {}
    for path in sorted(args.results_dir.glob("*.jsonl")):
        for row in read_jsonl(path):
            part_id = row.get("part_id")
            if (
                row.get("ok")
                and part_id in drafts
                and row.get("status") in {"mirrored", "converted"}
            ):
                results[part_id] = row
    missing = sorted(set(drafts) - set(results))
    if missing:
        raise SystemExit(f"missing verified staging results for {len(missing)} current part(s)")

    origin_only_names = DEFAULT_ORIGIN_ONLY_NAMES | set(args.origin_only_name)
    origin_only_ids = {part_id.lower() for part_id in args.origin_only_part_id}
    out_dir = args.output_dir.resolve()
    upload_dir = args.payload_upload_dir.resolve()
    if args.force:
        shutil.rmtree(out_dir, ignore_errors=True)
        shutil.rmtree(upload_dir, ignore_errors=True)
    (out_dir / "drafts").mkdir(parents=True, exist_ok=True)

    all_drafts: list[dict[str, Any]] = []
    mirrorable_drafts: list[dict[str, Any]] = []
    origin_only_drafts: list[dict[str, Any]] = []
    mirrorable: list[dict[str, Any]] = []
    origin_only: list[dict[str, Any]] = []
    blocked: list[dict[str, Any]] = []
    payload_manifest: list[dict[str, Any]] = []

    for part_id, draft in sorted(drafts.items(), key=lambda item: item[1]["name"]):
        result = results[part_id]
        converted = result.get("status") == "converted"
        payload = Path(result.get("converted_path") or result["download_path"])
        if not payload.is_file():
            raise SystemExit(f"payload is missing: {payload}")
        sha256 = result.get("converted_sha256") if converted else draft["sha256"]
        size_bytes = int(result.get("converted_size_bytes") or result["actual_size_bytes"])
        file_format = "safetensors" if converted else draft["file_format"]
        is_origin_only = (
            part_id.lower() in origin_only_ids
            or draft["name"] in origin_only_names
            or "civitai" in draft.get("source_kinds", [])
        )
        entry = {
            "old_part_id": part_id,
            "name": draft["name"],
            "type": draft["type"],
            "lane": draft["lane"],
            "payload": str(payload),
            "sha256": sha256,
            "size_bytes": size_bytes,
            "file_format": file_format,
            "converted": converted,
            "origin_only": is_origin_only,
        }
        mirror = None
        if is_origin_only and converted:
            entry["blocked_reason"] = (
                "origin-only source requires provider-side pickle-to-safetensors "
                "conversion before it can be provider-pulled"
            )
            entry["blocked_draft"] = provider_draft(draft, result, None)
            blocked.append(entry)
        elif is_origin_only:
            origin_only.append(entry)
            provider_entry = provider_draft(draft, result, None)
            origin_only_drafts.append(provider_entry)
            all_drafts.append(provider_entry)
        else:
            ext = extension_for(file_format, payload)
            repo_path = f"{args.mirror_prefix}/{sha256[:2]}/{sha256}.{ext}"
            upload_path = upload_dir / repo_path
            hardlink_payload(payload, upload_path, args.force)
            entry["repo_path"] = repo_path
            if args.mirror_revision:
                mirror = mirror_source(args.mirror_repo, args.mirror_revision, repo_path)
            mirrorable.append(entry)
            provider_entry = provider_draft(draft, result, mirror)
            mirrorable_drafts.append(provider_entry)
            all_drafts.append(provider_entry)
        payload_manifest.append(entry)

    (out_dir / "drafts" / "all.json").write_text(
        json.dumps(all_drafts, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (out_dir / "drafts" / "mirrorable.json").write_text(
        json.dumps(mirrorable_drafts, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (out_dir / "drafts" / "origin-only.json").write_text(
        json.dumps(origin_only_drafts, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (out_dir / "drafts" / "blocked.json").write_text(
        json.dumps(blocked, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (out_dir / "payload-manifest.json").write_text(
        json.dumps(payload_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    summary = {
        "ok": True,
        "current_drafts": len(drafts),
        "verified_results": len(results),
        "provider_ready_count": len(all_drafts),
        "mirror_count": len(mirrorable),
        "mirror_bytes": sum(int(item["size_bytes"]) for item in mirrorable),
        "origin_only_direct_count": len(origin_only),
        "blocked_origin_conversion_count": len(blocked),
        "output_dir": str(out_dir),
        "upload_dir": str(upload_dir),
        "mirror_revision": args.mirror_revision,
        "blocked": blocked,
    }
    (out_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
