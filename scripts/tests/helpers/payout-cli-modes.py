"""Expose the immutable plan fixtures through the separate Core command modes."""
import json
import os
from pathlib import Path
import sys

rail, mode, *args = sys.argv[1:]
rail = rail.removesuffix("-settlement")
fixtures = Path(os.environ["MOCK_" + rail.upper() + "_FIXTURES"])
journal = Path(os.environ["MOCK_EXTERNAL_EFFECT_STATE"]) / (rail + "-close.json")
name = {"success": "final", "no_work": "no_work-final", "below_threshold": "below_threshold-final"}.get(mode, mode)
original = json.loads((fixtures / (name + ".json")).read_text())
report = original.copy()
plan = original["settlement"]
result = original.get("feature_result") or {}
record = original.get("feature") or {"type": "targeted_payout_epoch_plan", "epoch": plan["epoch"],
          "rail": rail, "plan_root": plan["plan_root"], "value": plan}
report.update(feature=record, sim=False, submitted=True, freeze_only="--freeze-only" in args,
              selected_output_id=None, output_settlements=[], pending=True, feature_result=None,
              settlement_state=None, pending_outputs=[])
if "--freeze-only" in args and journal.is_file():
    # Closing in a preceding subprocess is now visible on a read-only replay.
    close = json.loads(journal.read_text())
    report.update(already_settled=close, settlement_state=close, submitted=False,
                  no_work=False, carry_forward=False, pending=False, nothing_to_settle=False)
elif "--output-id" in args:
    op = args[args.index("--output-id") + 1]
    assert any(o["economic_op_id"] == op for o in plan["outputs"])
    report.update(selected_output_id=op, pending_outputs=original.get("pending_outputs", []),
                  output_settlements=result.get("outputs", []))
elif "--close-only" in args:
    close = result.get("close")
    if close is not None:
        journal.parent.mkdir(exist_ok=True)
        journal.write_text(json.dumps(close))
        report.update(feature_result=result, settlement_state=close, pending=False)
else:
    assert "--freeze-only" in args
print(json.dumps(report))
