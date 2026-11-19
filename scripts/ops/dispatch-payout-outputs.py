#!/usr/bin/env python3
"""Drive independent outputs from an authenticated Core payout plan.

The local index controls scheduling only. Core owns reservations, processor
idempotency and receipts. A worker may finish an epoch only on a canonical close.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys

OP_ID = re.compile(r"[0-9a-f]{64}")
BATCH = 4


class InvalidEvidence(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise InvalidEvidence(reason)


def checked_plan(report, rail, epoch, anchor):
    require(isinstance(report, dict), "Core report has an invalid shape")
    plan = report.get("settlement")
    require(report.get("ok") is True and report.get("epoch") == epoch and
            isinstance(plan, dict) and plan.get("op") == "prepare_targeted_payout_epoch" and
            plan.get("rail") == rail and plan.get("epoch") == epoch and
            plan.get("epoch_apply_hash") == anchor and
            OP_ID.fullmatch(str(plan.get("plan_root"))) and isinstance(plan.get("outputs"), list),
            "Core returned a different epoch plan")
    skipped = report.get("skipped_providers")
    require(isinstance(skipped, list) and all(isinstance(item, dict) and item.get("blocking") is False
            for item in skipped), "plan reports an unresolved blocking omission")
    for item in skipped:
        amount = item.get("au")
        minimum = item.get("payout_min_au")
        require(item.get("reason") == "liability is below canonical payout_min_au and remains carried forward"
                and isinstance(amount, str) and re.fullmatch(r"[1-9][0-9]*", amount)
                and isinstance(minimum, str) and re.fullmatch(r"[1-9][0-9]*", minimum)
                and 0 < int(amount) < int(minimum), "nonblocking carry evidence omits payable work")
    ids = []
    for index, output in enumerate(plan["outputs"]):
        require(isinstance(output, dict) and type(output.get("output_index")) is int and
                output["output_index"] == index and OP_ID.fullmatch(str(output.get("economic_op_id"))),
                "invalid canonical payout output")
        ids.append(output["economic_op_id"])
    require(len(set(ids)) == len(ids), "duplicate payout output")
    if report.get("already_settled") is None:
        feature = report.get("feature")
        require(report.get("submitted") is True and isinstance(feature, dict) and
                feature.get("type") == "targeted_payout_epoch_plan" and feature.get("epoch") == epoch and
                feature.get("rail") == rail and feature.get("plan_root") == plan["plan_root"] and
                feature.get("value") == plan, "the frozen plan has no confirmed record")
    return plan


def open_schedule(work_dir, rail, plan):
    target = work_dir / (rail + "-outputs.sqlite3")
    require(not target.is_symlink(), "invalid payout output journal")
    db = sqlite3.connect(target)
    with db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("CREATE TABLE IF NOT EXISTS identity(plan_root TEXT PRIMARY KEY)")
        db.execute("""CREATE TABLE IF NOT EXISTS pending(
            ticket INTEGER PRIMARY KEY AUTOINCREMENT, op TEXT UNIQUE NOT NULL, idx INTEGER NOT NULL)""")
        row = db.execute("SELECT plan_root FROM identity").fetchone()
        if row is None:
            db.execute("INSERT INTO identity VALUES(?)", (plan["plan_root"],))
            db.executemany("INSERT INTO pending(op,idx) VALUES(?,?)",
                           ((out["economic_op_id"], out["output_index"]) for out in plan["outputs"]))
        else:
            require(row[0] == plan["plan_root"], "retained schedule conflicts with the frozen Core plan")
    return db


def check_receipt(receipt, plan, index, op):
    rail = plan["rail"]
    require(isinstance(receipt, dict), "invalid Core receipt")
    value = receipt.get("value")
    require(isinstance(value, dict) and receipt.get("type") == f"targeted_{rail}_output_settlement" and
            receipt.get("epoch") == plan["epoch"] and receipt.get("rail") == rail and
            receipt.get("economic_op_id") == op and value.get("economic_op_id") == op and
            value.get("epoch") == plan["epoch"] and value.get("rail") == rail and
            value.get("epoch_apply_hash") == plan["epoch_apply_hash"] and
            value.get("plan_root") == plan["plan_root"] and value.get("output_index") == index and
            value.get("op") == f"settle_targeted_{rail}_output" and
            (rail != "fiat" or OP_ID.fullmatch(str(value.get("attempt_id")))), "receipt is for a different payout")


def pending_item(report, output):
    rows = report.get("pending_outputs")
    require(report.get("pending") is True and isinstance(rows, list) and len(rows) == 1,
            "selected payout has neither an exact receipt nor an outstanding reason")
    row = rows[0]
    require(isinstance(row, dict) and row.get("output_index") == output["output_index"] and
            row.get("economic_op_id") == output["economic_op_id"] and
            row.get("reason") in {"waiting_for_funds", "dispatch_error", "needs_reconciliation"}, "unbound pending payout")
    item = {k: row[k] for k in ("output_index", "economic_op_id", "reason")}
    if item["reason"] == "waiting_for_funds":
        funding = row.get("funding")
        require(isinstance(funding, dict) and output.get("role") == "provider" and
                funding.get("currency") == output.get("source_currency") and
                funding.get("required_minor") == output.get("source_amount_minor"),
                "funding check disagrees with selected payout")
        keys = ("required_minor", "available_minor", "pending_minor", "shortfall_minor")
        require(all(isinstance(funding.get(k), str) and re.fullmatch(r"-?(0|[1-9][0-9]*)", funding[k])
                    for k in keys), "invalid processor funding amounts")
        short = max(0, int(funding["required_minor"]) - int(funding["available_minor"]))
        require(int(funding["required_minor"]) > 0 and short == int(funding["shortfall_minor"]) and
                funding.get("sufficient") is (short == 0) and
                (short > 0 or funding.get("processor_rejected") is True), "inconsistent processor shortfall")
        item["funding"] = {k: funding[k] for k in (*keys, "currency", "sufficient")}
        if funding.get("processor_rejected") is True:
            item["funding"]["processor_rejected"] = True
    return item


def drive(work_dir, rail, epoch, anchor, invoke):
    frozen = invoke(["--freeze-only", "--submit"])
    plan = checked_plan(frozen, rail, epoch, anchor)
    require(frozen.get("freeze_only") is True and frozen.get("selected_output_id") is None and
            frozen.get("sim") is False,
            "Core did not confirm freeze-only mode")
    if frozen.get("already_settled") is not None:
        return 0, frozen
    db = open_schedule(work_dir, rail, plan)
    try:
        pending = []
        failed = False
        # Select once. Rotating a single failed output must never select it
        # twice in the same timer wake, even if it is the last one left.
        batch = db.execute("SELECT ticket,op,idx FROM pending ORDER BY ticket LIMIT ?", (BATCH,)).fetchall()
        for ticket, op, index in batch:
            require(0 <= index < len(plan["outputs"]) and plan["outputs"][index]["economic_op_id"] == op,
                    "retained output schedule disagrees with Core")
            with db:
                db.execute("DELETE FROM pending WHERE ticket=?", (ticket,))
                db.execute("INSERT INTO pending(op,idx) VALUES(?,?)", (op, index))
            try:
                result = invoke(["--output-id", op, "--submit-transfer", "--submit"])
                require(checked_plan(result, rail, epoch, anchor) == plan and result.get("selected_output_id") == op,
                        "selected output changed its frozen plan")
                if result.get("already_settled") is not None:
                    return 0, result
                receipts = result.get("output_settlements")
                require(isinstance(receipts, list) and len(receipts) <= 1, "unexpected receipts for selected payout")
                if receipts:
                    require(result.get("pending_outputs") == [], "conflicting receipt and pending information")
                    check_receipt(receipts[0], plan, index, op)
                    with db:
                        db.execute("DELETE FROM pending WHERE op=?", (op,))
                else:
                    item = pending_item(result, plan["outputs"][index])
                    pending.append(item)
                    failed |= item["reason"] != "waiting_for_funds"
            except (InvalidEvidence, subprocess.SubprocessError) as exc:
                # Keep the original effect identity active and attempt siblings.
                # Never retain raw HTTP errors, which may contain secrets.
                cause = str(exc) if isinstance(exc, InvalidEvidence) else "Core dispatch command failed"
                print(f"{rail}: output {index} requires reconciliation: {cause}", file=sys.stderr)
                pending.append({"output_index": index, "economic_op_id": op, "reason": "unverified_result"})
                failed = True
        remaining = db.execute("SELECT EXISTS(SELECT 1 FROM pending)").fetchone()[0]
        if remaining:
            return 1 if failed else 2, {"schema_version": 1, "type": "outstanding_payout_outputs",
                "rail": rail, "epoch": epoch, "epoch_apply_hash": anchor, "plan_root": plan["plan_root"],
                "pending": True, "pending_outputs": pending}
        # Closing is a separate Core operation with NO processor or MSB send
        # permission. Core independently verifies every required receipt.
        try:
            close = invoke(["--close-only", "--submit"])
            require(checked_plan(close, rail, epoch, anchor) == plan and close.get("pending") is not True,
                    "Core did not close the exact epoch")
            if close.get("already_settled") is None:
                close = invoke(["--freeze-only", "--submit"])
            require(checked_plan(close, rail, epoch, anchor) == plan and close.get("already_settled") is not None,
                    "canonical close not visible")
        except (InvalidEvidence, subprocess.SubprocessError):
            # A crash or restored local spool can retain a stale projection.
            # The authenticated close wins. Rebuild only after that disagreement.
            # Rechecking each original operation is idempotent at the processor.
            with db:
                db.executemany("INSERT INTO pending(op,idx) VALUES(?,?)",
                    ((out["economic_op_id"], out["output_index"]) for out in plan["outputs"]))
            raise
        return 0, close
    finally:
        db.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work_dir", type=Path)
    parser.add_argument("rail", choices=("tnk", "fiat"))
    parser.add_argument("epoch", type=int)
    parser.add_argument("anchor")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    require(args.command and OP_ID.fullmatch(args.anchor), "invalid worker arguments")
    os.umask(0o077)

    def invoke(flags):
        raw = subprocess.run([*args.command, *flags], check=True, stdout=subprocess.PIPE)
        try:
            return json.loads(raw.stdout)
        except (UnicodeError, json.JSONDecodeError) as exc:
            raise InvalidEvidence("Core returned invalid JSON") from exc
    status, report = drive(args.work_dir, args.rail, args.epoch, args.anchor, invoke)
    json.dump(report, sys.stdout, separators=(",", ":"))
    print()
    return status


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as exc:
        # The CLI can take private paths and authentication options.
        cause = str(exc) if isinstance(exc, InvalidEvidence) else type(exc).__name__
        print("Payout dispatch could not verify its inputs: " + cause, file=sys.stderr)
        sys.exit(1)
