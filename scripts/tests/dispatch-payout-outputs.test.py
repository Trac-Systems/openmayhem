import importlib.util
import sqlite3
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "ops" / "dispatch-payout-outputs.py"
spec = importlib.util.spec_from_file_location("dispatch", SCRIPT)
dispatch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dispatch)
ANCHOR = 'a' * 64


class ProcessorFixture:
    def __init__(self, rail, epoch, count):
        self.plan = {"op": "prepare_targeted_payout_epoch", "rail": rail, "epoch": epoch,
                     "epoch_apply_hash": ANCHOR, "plan_root": format(epoch, '064x'),
                     "outputs": [{"output_index": i, "economic_op_id": format(100 * epoch + i, '064x'),
                                  "role": "provider", "source_amount_minor": "4", "source_currency": "eur"}
                                 for i in range(count)]}
        self.confirmed = {}
        self.sent = []
        self.selected = []
        self.unavailable = {0}
        self.lost_response = set()
        self.bad = set()
        self.closed = False
        self.close_attempts = 0

    def __call__(self, flags):
        p = self.plan
        rail, epoch = p["rail"], p["epoch"]
        record = {"type": "targeted_payout_epoch_plan", "rail": rail, "epoch": epoch,
                  "plan_root": p["plan_root"], "value": p}
        report = {"ok": True, "sim": False, "settlement": p, "skipped_providers": [], "epoch": epoch,
                  "submitted": False, "already_settled": {"epoch": epoch} if self.closed else None,
                  "feature": record, "pending": not self.closed, "selected_output_id": None,
                  "output_settlements": [], "pending_outputs": []}
        if flags == ["--freeze-only", "--submit"]:
            return {**report, "freeze_only": True}
        if flags == ["--close-only", "--submit"]:
            self.close_attempts += 1
            assert len(self.confirmed) == len(p["outputs"])
            self.closed = True
            return {**report, "pending": False}
        assert flags[0] == "--output-id" and flags[2:] == ["--submit-transfer", "--submit"]
        op = flags[1]
        output = next(o for o in p["outputs"] if o["economic_op_id"] == op)
        i = output["output_index"]
        self.selected.append(i)
        report["selected_output_id"] = op
        if i in self.unavailable:
            item = {"output_index": i, "economic_op_id": op, "reason": "dispatch_error"}
            if rail == "fiat":
                item.update(reason="waiting_for_funds", funding={"currency": "eur", "required_minor": "4",
                            "available_minor": "1", "shortfall_minor": "3", "pending_minor": "0",
                            "sufficient": False})
            return {**report, "pending_outputs": [item]}
        if op not in self.confirmed:
            self.sent.append(op)
            self.confirmed[op] = {"type": f"targeted_{rail}_output_settlement", "rail": rail, "epoch": epoch,
                "economic_op_id": op, "value": {"op": f"settle_targeted_{rail}_output", "rail": rail, "epoch": epoch,
                "epoch_apply_hash": ANCHOR, "plan_root": p["plan_root"], "output_index": i,
                "economic_op_id": op, "attempt_id": "b" * 64}}
        if i in self.lost_response:
            self.lost_response.remove(i)
            raise subprocess.CalledProcessError(1, ["core"])
        if i in self.bad:
            return {**report, "output_settlements": [{"value": {}}]}
        return {**report, "output_settlements": [self.confirmed[op]]}


class IsolationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def run_epoch(self, core):
        epoch = core.plan["epoch"]
        work = self.root / str(epoch)
        work.mkdir(exist_ok=True)
        return dispatch.drive(work, core.plan["rail"], epoch, ANCHOR, core)

    def test_stalled_output_does_not_block_siblings_or_newer_epochs_and_retry_pays_once(self):
        for rail in ["fiat", "tnk"]:
            with self.subTest(rail=rail):
                old = ProcessorFixture(rail, 6, 9)
                old.lost_response.add(1)
                self.assertEqual(self.run_epoch(old)[0], 1)
                self.assertEqual(old.selected, [0, 1, 2, 3])
                self.assertEqual(len(old.sent), 3)
                newer = ProcessorFixture(rail, 7, 1)
                newer.unavailable.clear()
                self.assertEqual(self.run_epoch(newer)[0], 0)
                self.assertTrue(newer.closed)
                self.assertFalse(old.closed)
                self.run_epoch(old)
                self.run_epoch(old)
                self.assertEqual(len(old.sent), 8)
                self.assertFalse(old.closed)
                self.assertEqual(old.close_attempts, 0)
                old.unavailable.clear()
                self.assertEqual(self.run_epoch(old)[0], 0)
                self.assertTrue(old.closed)
                self.assertEqual(len(set(old.sent)), 9)
                self.assertEqual(len(old.sent), 9)
                self.assertEqual(self.run_epoch(old)[0], 0)
                self.assertEqual(old.close_attempts, 1)

    def test_failed_receipt_validation_preserves_that_output_and_siblings_progress(self):
        core = ProcessorFixture("fiat", 2, 2)
        core.unavailable.clear()
        core.bad.add(0)
        status, wait = self.run_epoch(core)
        self.assertEqual(status, 1)
        self.assertEqual(wait["pending_outputs"][0]["reason"], "unverified_result")
        self.assertEqual(len(core.sent), 2)
        self.assertFalse(core.closed)
        core.bad.clear()
        self.assertEqual(self.run_epoch(core)[0], 0)
        self.assertEqual(len(core.sent), 2)

    def test_canonical_close_rejection_repairs_stale_local_projection(self):
        core = ProcessorFixture("fiat", 8, 2)
        self.run_epoch(core)
        with sqlite3.connect(self.root / "8" / "fiat-outputs.sqlite3") as db:
            db.execute("DELETE FROM pending")
        def reject_missing(flags):
            if "--close-only" in flags:
                raise subprocess.CalledProcessError(1, ["core"])
            return core(flags)
        with self.assertRaises(subprocess.CalledProcessError):
            dispatch.drive(self.root / "8", "fiat", 8, ANCHOR, reject_missing)
        core.unavailable.clear()
        self.assertEqual(self.run_epoch(core)[0], 0)
        self.assertEqual(len(core.sent), 2)

    def test_conflicting_frozen_plan_cannot_reuse_local_output_index(self):
        core = ProcessorFixture("tnk", 3, 5)
        self.run_epoch(core)
        initial = core.sent.copy()
        core.plan["plan_root"] = "d" * 64
        with self.assertRaises(dispatch.InvalidEvidence):
            self.run_epoch(core)
        self.assertEqual(initial, core.sent)


if __name__ == "__main__":
    unittest.main()
