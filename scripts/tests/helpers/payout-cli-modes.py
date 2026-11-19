"""Expose the immutable plan fixtures through the separate Core command modes."""
import json
import os
from pathlib import Path
import signal
import sys

rail, *args = sys.argv[1:]
rail = rail.removesuffix("-settlement")
epoch = int(args[args.index("--epoch") + 1])


def setting(name, default=None):
    key = f"MOCK_{rail.upper()}_{name}"
    return os.environ.get(f"{key}_{epoch}", os.environ.get(key, default))


def record_path(key):
    return Path(os.environ["MOCK_EXTERNAL_EFFECT_STATE"]) / (key + ".json")


def confirm(key, value):
    path = record_path(key)
    envelope = dict(key=key, value=value, confirmed=True, signed_length=123)
    if path.is_file():
        assert json.loads(path.read_text()) == envelope, f"conflicting mock record {key}"
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(envelope))


def transfer(output):
    # Model processor idempotency separately from confirmation. An interrupted
    # CLI call can have a durable effect and still have no Core receipt.
    effect = record_path(f"effect/{rail}/{output['economic_op_id']}")
    with open(os.environ["MOCK_PAYOUT_EVENT_LOG"], "a") as events:
        print(f"{rail}:canonical-preparation-readback", file=events)
        if effect.is_file():
            assert json.loads(effect.read_text()) == output
            print(f"{rail}:external-transfer-recovered", file=events)
            return
        effect.parent.mkdir(parents=True, exist_ok=True)
        effect.write_text(json.dumps(output))
        print(f"{rail}:external-transfer", file=events)
    if os.environ.get("MOCK_INTERRUPT_AFTER_OP") == output["economic_op_id"]:
        # The test starts this worker in its own session. Kill exactly that
        # session's process group, including the shell and Python dispatcher.
        group = int(os.environ["MOCK_PAYOUT_TEST_GROUP"])
        if not (group > 1 and group == os.getpgrp() == os.getsid(0)):
            raise RuntimeError("interruption requires a dedicated payout test session")
        os.killpg(group, signal.SIGKILL)
    if os.environ.get("MOCK_CRASH_AFTER_EXTERNAL_TRANSFER") == rail:
        print(f"simulated crash after {rail} external transfer", file=sys.stderr)
        sys.exit(75)


mode = setting("MODE", "success")
if mode == "command_error":
    sys.exit(1)
fixtures = Path(setting("FIXTURES"))
name = {"success": "final", "needs_reconciliation": "final", "no_work": "no_work-final",
        "below_threshold": "below_threshold-final"}.get(mode, mode)
original = json.loads((fixtures / (name + ".json")).read_text())
report = original.copy()
plan = original["settlement"]
result = original.get("feature_result") or {}
record = original.get("feature") or {"type": "targeted_payout_epoch_plan", "epoch": plan["epoch"],
          "rail": rail, "plan_root": plan["plan_root"], "value": plan}
plan_key = f"payout/epoch-plan/{rail}/{epoch}"
close_key = f"settle/targeted/{rail}/{epoch}"
assert "--submit" in args
assert ("--submit-transfer" in args) == ("--output-id" in args)
report.update(feature=record, sim=False, submitted=True, freeze_only="--freeze-only" in args,
              selected_output_id=None, output_settlements=[], pending=True, feature_result=None,
              settlement_state=None, pending_outputs=[])
if "--freeze-only" in args:
    confirm(plan_key, record)
    if record_path(close_key).is_file():
        close = json.loads(record_path(close_key).read_text())["value"]
        report.update(already_settled=close, settlement_state=close, submitted=False,
                      no_work=False, carry_forward=False, pending=False, nothing_to_settle=False)
elif "--output-id" in args:
    op = args[args.index("--output-id") + 1]
    output = next(o for o in plan["outputs"] if o["economic_op_id"] == op)
    assert json.loads(record_path(plan_key).read_text())["value"] == record
    report["selected_output_id"] = op
    if mode == "needs_reconciliation":
        report["pending_outputs"] = [dict(economic_op_id=op, output_index=output["output_index"], reason=mode)]
    elif original.get("pending_outputs"):
        report["pending_outputs"] = original["pending_outputs"]
    else:
        transfer(output)
        receipts = [receipt for receipt in result.get("outputs", [])
                    if receipt["value"]["output_index"] == output["output_index"]]
        for receipt in receipts:
            confirm(f"{close_key}/output/{receipt['economic_op_id']}", receipt)
        report["output_settlements"] = receipts
elif "--close-only" in args:
    close = result.get("close")
    if close is not None:
        assert all(record_path(f"{close_key}/output/{o['economic_op_id']}").is_file() for o in plan["outputs"])
        confirm(close_key, close)
        report.update(feature_result=result, settlement_state=close, pending=False)
else:
    raise AssertionError("expected a payout command mode")
print(json.dumps(report))
