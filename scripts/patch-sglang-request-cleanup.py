#!/usr/bin/env python3
"""Build the signed overlay from the exact qualified SGLang source.

An interrupted async generator raises BaseException after scheduler dispatch.
Discarding its tokenizer state first makes abort_request's existence guard
skip the scheduled request. Abort while the state still exists, then clean up.
This script prepares a file; it never edits a running runtime.
"""
import argparse
import hashlib
from pathlib import Path

SOURCE_SHA256 = "92b42e5b0445111c60dc61751de4e9843d9dc8f2631c2509899be0ce680ee3ef"
OLD = "        for rid in rids:\n            self.rid_to_state.pop(rid, None)\n"
NEW = """        for rid in rids:
            # Cancellation may arrive after dispatch. The abort helper checks
            # rid_to_state, so signal the scheduler before discarding the entry.
            # Completed/missing requests remain no-ops; never abort all requests.
            self.abort_request(rid)
            self.rid_to_state.pop(rid, None)
"""


def patch(source: bytes) -> bytes:
    if hashlib.sha256(source).hexdigest() != SOURCE_SHA256:
        raise ValueError("SGLang tokenizer manager differs from the pinned source")
    text = source.decode("utf-8")
    if text.count(OLD) != 1:
        raise ValueError("Expected exactly one request cleanup block")
    result = text.replace(OLD, NEW).encode("utf-8")
    compile(result, "tokenizer_manager.py", "exec")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    data = patch(args.source.read_bytes())
    with args.destination.open("xb") as output:
        output.write(data)
