#!/usr/bin/env python3
"""Exercise the pinned runtime's actual cleanup/abort methods without a GPU."""
import ast
import importlib.util
import json
from pathlib import Path
import sys
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location(
    "cleanup_patch", Path(__file__).with_name("patch-sglang-request-cleanup.py")
)
patcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(patcher)


def manager(source):
    tree = ast.parse(source)
    original = next(n for n in tree.body if isinstance(n, ast.ClassDef)
                    and n.name == "TokenizerManager")
    methods = [n for n in original.body if isinstance(n, ast.FunctionDef)
               and n.name in {"abort_request", "_discard_pending_req_states"}]
    assert len(methods) == 2
    cls = ast.ClassDef(name="Manager", bases=[], keywords=[], body=methods,
                      decorator_list=[])
    scope = {"AbortReq": SimpleNamespace,
             "logger": SimpleNamespace(warning=lambda *args: None)}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[cls], type_ignores=[])),
                 "pinned-runtime-methods", "exec"), scope)
    obj = scope["Manager"]()
    obj.server_args = SimpleNamespace(tokenizer_worker_num=1)
    obj.enable_metrics = False
    obj.rid_to_state = {"cancelled": object(), "sibling": object()}
    obj.running = {"cancelled", "sibling"}
    obj.aborts = []

    def dispatch(request):
        assert not request.abort_all
        assert request.rid in obj.rid_to_state, "abort must precede state cleanup"
        obj.aborts.append(request.rid)
        obj.running.discard(request.rid)

    obj._dispatch_to_scheduler = dispatch
    return obj


def main(source):
    baseline = manager(source)
    baseline._discard_pending_req_states(SimpleNamespace(is_single=True, rid="cancelled"))
    baseline.abort_request("cancelled")  # delayed streaming disconnect handler
    assert "cancelled" in baseline.running and not baseline.aborts

    fixed_source = patcher.patch(source)
    fixed = manager(fixed_source)
    fixed._discard_pending_req_states(SimpleNamespace(is_single=True, rid="cancelled"))
    fixed.abort_request("cancelled")
    assert fixed.running == {"sibling"} and fixed.aborts == ["cancelled"]
    assert set(fixed.rid_to_state) == {"sibling"}

    batch = manager(fixed_source)
    batch._discard_pending_req_states(SimpleNamespace(is_single=False,
        rid=["cancelled", "missing", "cancelled"]))
    assert batch.running == {"sibling"} and batch.aborts == ["cancelled"]

    undispatched = manager(fixed_source)
    undispatched.running.discard("cancelled")
    undispatched._discard_pending_req_states(SimpleNamespace(rid="cancelled"))
    assert undispatched.running == {"sibling"}

    completed = manager(fixed_source)
    completed.rid_to_state.pop("cancelled")
    completed.running.discard("cancelled")
    completed._discard_pending_req_states(SimpleNamespace(rid="cancelled"))
    assert completed.running == {"sibling"} and not completed.aborts

    try:
        patcher.patch(source + b"\n")
    except ValueError:
        pass
    else:
        raise AssertionError("Changed runtime source was accepted")
    print(json.dumps({"baselineLeakReproduced": True, "fixedCancellation": True,
        "siblingPreserved": True, "partialBatch": True, "undispatched": True,
        "completedNoop": True, "unknownSourceRejected": True}))


if __name__ == "__main__":
    main(Path(sys.argv[1]).read_bytes())
