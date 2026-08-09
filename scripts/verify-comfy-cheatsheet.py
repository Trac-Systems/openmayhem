#!/usr/bin/env python3
"""Verify that the Comfy cheatsheet covers the signed workflow surface."""

from __future__ import annotations

import argparse
import json
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cheatsheet", default="comfy-cheatsheet.md")
    parser.add_argument("--parts-index", required=True)
    parser.add_argument("--outcome-grid", default="catalog/comfy/outcome-classes-v1.json")
    args = parser.parse_args()

    cheatsheet = Path(args.cheatsheet)
    parts_index = Path(args.parts_index)
    outcome_grid = Path(args.outcome_grid)
    text = cheatsheet.read_text(encoding="utf-8")

    missing_sections = [section for section in REQUIRED_SECTIONS if section not in text]
    index = load_json(parts_index)
    missing_parts = [part["part_id"] for part in index.get("parts", []) if part["part_id"] not in text]

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
        f"{len(index.get('parts', []))} signed parts, {len(rows)} outcome classes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
