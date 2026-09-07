#!/usr/bin/env python3
"""Install the pinned vLLM 0.24 recurrent-prefix speculative boundary correction.

Used by every Mayhem vLLM provider runtime setup. Source:
vllm-project/vllm#43650, adapted to 0.24's drop_eagle_block argument.
Only the exact reviewed original or patched module is accepted.
"""
import argparse
import ast
import hashlib
import json
import importlib.metadata
import tempfile
from pathlib import Path

ORIGINAL_SHA256 = "22e3677acf0e769461bdbf278df031d6e362c97aa1ba7b862c071b76d47d8914"
PATCHED_SHA256 = "b9433c9f93fe231325f22f48baf5059146e15dd733f39b2c512bf245cc8d79cf"

MARKER = "# mayhem: exclude speculative recurrent state from prefix lookup"

def patched_source(source):
    if MARKER in source:
        return source
    tree = ast.parse(source)
    manager = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "MambaManager")
    finder = next(n for n in manager.body if isinstance(n, ast.FunctionDef) and n.name == "find_longest_cache_hit")
    assert "drop_eagle_block" in [a.arg for a in finder.args.args]
    lines = source.splitlines(keepends=True)
    candidates = [i for i in range(finder.lineno - 1, finder.end_lineno)
                  if lines[i].strip() == "max_num_blocks = max_length // block_size"]
    assert len(candidates) == 1, "Unexpected vLLM Mamba lookup; source review required"
    i = candidates[0]
    indent = lines[i][:len(lines[i]) - len(lines[i].lstrip())]
    lines.insert(i + 1, indent + MARKER + "\n" + indent +
                 "if drop_eagle_block and max_num_blocks > 0:\n" + indent +
                 "    max_num_blocks -= 1\n")
    result = "".join(lines)
    ast.parse(result)
    return result

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--site-packages", type=Path)
    parser.add_argument("--backup", type=Path, required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    package = importlib.metadata.distribution("vllm") if args.site_packages is None else None
    if package is not None and package.version not in ("0.24.0", "0.24.0+mayhem.specmeta1"):
        raise RuntimeError("Unknown vLLM prefix runtime version; source review required")
    site_packages = args.site_packages or Path(package.locate_file(""))
    path = site_packages / "vllm/v1/core/single_type_kv_cache_manager.py"
    old = path.read_text()
    digest = hashlib.sha256(old.encode()).hexdigest()
    if digest not in (ORIGINAL_SHA256, PATCHED_SHA256):
        raise RuntimeError("Unrecognized vLLM cache manager; source review required")
    new = patched_source(old)
    if hashlib.sha256(new.encode()).hexdigest() != PATCHED_SHA256:
        raise RuntimeError("vLLM prefix correction digest mismatch")
    evidence = {"path": str(path), "before": hashlib.sha256(old.encode()).hexdigest(),
                "after": hashlib.sha256(new.encode()).hexdigest(), "changed": old != new,
                "applied": args.apply}
    if args.apply and old != new:
        args.backup.mkdir(parents=True, exist_ok=True, mode=0o700)
        backup = args.backup / (evidence["before"] + ".py")
        if backup.exists():
            if hashlib.sha256(backup.read_bytes()).hexdigest() != ORIGINAL_SHA256:
                raise RuntimeError("Existing vLLM prefix backup does not match")
        else:
            with backup.open("x") as f:
                f.write(old)
            backup.chmod(0o600)
        with tempfile.NamedTemporaryFile(mode="w", prefix=".mayhem-prefix-", dir=path.parent, delete=False) as f:
            f.write(new)
            temporary = Path(f.name)
        try:
            temporary.chmod(path.stat().st_mode & 0o777)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
        (args.backup / "applied.json").write_text(json.dumps(evidence, indent=2))
    print(json.dumps(evidence))

if __name__ == "__main__":
    main()
