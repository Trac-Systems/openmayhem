#!/usr/bin/env python3
"""Fetch Civitai version-file metadata for Comfy corpus rows.

Input is a JSON list containing at least version_id and sha256. Output records
the matching file name, download URL, sizeKB, and scan fields without printing
or storing the API token.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any
from urllib.request import Request, urlopen


def read_token(path: Path) -> str:
    token = path.read_text(encoding="utf-8").strip()
    if not token:
        raise SystemExit(f"token file is empty: {path}")
    return token


def matching_file(item: dict[str, Any], files: list[dict[str, Any]]) -> tuple[dict[str, Any] | None, int]:
    wanted = str(item.get("sha256") or "").upper()
    matches = []
    for file in files:
        hashes = file.get("hashes") or {}
        if str(hashes.get("SHA256") or hashes.get("sha256") or "").upper() == wanted:
            matches.append(file)
    if matches:
        return matches[0], len(matches)
    if len(files) == 1:
        return files[0], 0
    return None, 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Fetch Civitai model-version file metadata.")
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--token-file", required=True, type=Path)
    args = parser.parse_args()

    token = read_token(args.token_file)
    items = json.loads(args.input.read_text(encoding="utf-8"))
    results = []
    for item in items:
        url = f"https://civitai.com/api/v1/model-versions/{item['version_id']}"
        request = Request(
            url,
            headers={
                "Authorization": f"Bearer {token}",
                "User-Agent": "openmayhem-comfy-corpus/1",
            },
        )
        try:
            with urlopen(request, timeout=45) as response:
                body = json.load(response)
            files = body.get("files") or []
            file, match_count = matching_file(item, files)
            if file is None:
                results.append(
                    {
                        **item,
                        "ok": False,
                        "file_count": len(files),
                        "match_count": match_count,
                        "error": "no unique file matched sha256",
                    }
                )
                continue
            results.append(
                {
                    **item,
                    "ok": True,
                    "file_count": len(files),
                    "match_count": match_count,
                    "file_name": file.get("name"),
                    "size_kb": file.get("sizeKB"),
                    "download_url": file.get("downloadUrl") or item.get("download_url"),
                    "pickle_scan_result": file.get("pickleScanResult"),
                    "virus_scan_result": file.get("virusScanResult"),
                }
            )
        except Exception as error:  # noqa: BLE001 - result file should name failed rows.
            results.append({**item, "ok": False, "error": str(error)})

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(results, indent=2, sort_keys=True), encoding="utf-8")
    summary = {
        "ok": all(result.get("ok") for result in results),
        "count": len(results),
        "matched": sum(1 for result in results if result.get("match_count")),
        "output": str(args.output),
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
