#!/usr/bin/env python3
"""Verify that the Comfy cheatsheet covers the signed workflow surface."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


REQUIRED_SECTIONS = [
    "## Calibration Acceptance Gate",
    "## Outcome Classes",
    "## Class Fit Matrix",
    "## Required Part Sets",
    "## All Signed Parts In This Index",
]


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def strip_scalar(value: str) -> str:
    value = value.strip()
    if value.startswith(("'", '"')) and value.endswith(("'", '"')) and len(value) >= 2:
        return value[1:-1]
    return value


def load_parts(path: Path):
    text = path.read_text(encoding="utf-8")
    try:
        data = json.loads(text)
        parts = data.get("parts", [])
        return {
            "kind": "json",
            "parts": parts,
            "missing": lambda part, cheatsheet: part["part_id"] not in cheatsheet,
            "label": lambda part: part["part_id"],
        }
    except json.JSONDecodeError:
        pass

    rows = []
    current = None
    for raw_line in text.splitlines():
        if not raw_line.strip() or raw_line.lstrip().startswith("#"):
            continue
        match = re.match(r"^-\s+name:\s*(.+?)\s*$", raw_line)
        if match:
            if current:
                rows.append(current)
            current = {"name": strip_scalar(match.group(1))}
            continue
        if current is None:
            continue
        match = re.match(r"^\s+([A-Za-z0-9_-]+):\s*(.*?)\s*$", raw_line)
        if match:
            current[match.group(1)] = strip_scalar(match.group(2))
    if current:
        rows.append(current)

    parts = [row for row in rows if row.get("status") == "linked" and row.get("name")]
    return {
        "kind": "yaml",
        "parts": parts,
        "missing": lambda part, cheatsheet: part["name"] not in cheatsheet,
        "label": lambda part: part["name"],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cheatsheet", default="COMFY-CHEATSHEET.md")
    parser.add_argument("--parts-index", required=True)
    parser.add_argument("--outcome-grid", default="catalog/comfy/outcome-classes-v1.json")
    args = parser.parse_args()

    cheatsheet = Path(args.cheatsheet)
    parts_index = Path(args.parts_index)
    outcome_grid = Path(args.outcome_grid)
    text = cheatsheet.read_text(encoding="utf-8")

    missing_sections = [section for section in REQUIRED_SECTIONS if section not in text]
    index = load_parts(parts_index)
    missing_parts = [
        index["label"](part)
        for part in index["parts"]
        if index["missing"](part, text)
    ]

    grid = load_json(outcome_grid)
    rows = grid.get("classes") or grid.get("outcome_classes") or []
    missing_classes = [row["class_id"] for row in rows if row["class_id"] not in text]

    errors = []
    if missing_sections:
        errors.append(f"missing sections: {', '.join(missing_sections)}")
    if missing_parts:
        errors.append(f"missing signed part IDs: {', '.join(missing_parts)}")
    if missing_classes:
        errors.append(f"missing outcome classes: {', '.join(missing_classes)}")

    if errors:
        for error in errors:
            print(error)
        return 1

    print(
        "Comfy cheatsheet coverage ok: "
        f"{len(index['parts'])} {index['kind']} signed parts, {len(rows)} outcome classes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
