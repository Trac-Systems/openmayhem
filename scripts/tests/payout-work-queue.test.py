"""Exercise the payout scheduler through its installed worker entry point."""
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest

QUEUE = Path(__file__).resolve().parents[1] / "ops" / "payout-work-queue.py"


class PayoutWorkQueueTests(unittest.TestCase):
    def setUp(self):
        spool = tempfile.TemporaryDirectory()
        self.addCleanup(spool.cleanup)
        self.root = Path(spool.name)

    def epoch(self, n, anchor=None):
        path = self.root / f"epoch-{n}-{anchor or format(n, '064x')}"
        path.mkdir()
        return path

    def invoke(self, action, path, *args, check=True):
        return subprocess.run([sys.executable, QUEUE, action, path, *args],
                              check=check, capture_output=True)

    def select(self, path, rail):
        listing = self.invoke("select", path, rail).stdout
        return [Path(os.fsdecode(name)) for name in listing.split(b"\0") if name]

    def test_stuck_epochs_rotate_across_restarts_and_rails_independently(self):
        outstanding = {self.epoch(i) for i in range(1, 13)}
        head = self.epoch(13)
        self.invoke("sync", head)
        first = self.select(head, "fiat")
        second = self.select(head, "fiat")
        third = self.select(head, "fiat")
        self.assertEqual([len(first), len(second), len(third)], [4, 4, 4])
        self.assertEqual(set(first + second + third), outstanding)
        # A stopped process after selection did not discard the unpaid epochs.
        self.assertEqual(self.select(head, "fiat"), first)
        self.assertEqual(self.select(head, "tnk"), first)
        self.assertEqual([p.name.split("-")[1] for p in self.select(head, "tap")],
                         ["1", "2", "3", "4"])
        self.assertEqual([p.name.split("-")[1] for p in self.select(head, "tap")],
                         ["1", "2", "3", "4"])

    def test_success_retires_only_completed_rail_and_sync_is_idempotent(self):
        prior, head = self.epoch(7), self.epoch(8)
        self.invoke("sync", head)
        (prior / "fiat.complete").write_text("verified by worker")
        self.invoke("sync", prior)
        self.invoke("sync", prior)
        self.assertEqual(self.select(head, "fiat"), [])
        self.assertEqual(self.select(head, "tnk"), [prior])
        self.assertEqual(self.invoke("has-prior", head, "tap").stdout.strip(), b"yes")
        self.assertEqual(self.invoke("has-prior", head, "fiat").stdout.strip(), b"no")
        with sqlite3.connect(self.root / "pending-work.sqlite3") as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM pending").fetchone()[0], 5)

    def test_conflicting_heads_or_missing_evidence_cannot_dispatch(self):
        old, head = self.epoch(7), self.epoch(8)
        self.invoke("sync", head)
        self.assertNotEqual(self.invoke("select", old, "fiat", check=False).returncode, 0)
        conflict = self.epoch(8, "f" * 64)
        self.invoke("sync", conflict)
        self.assertNotEqual(self.invoke("select", head, "fiat", check=False).returncode, 0)
        (old / "tnk.complete").symlink_to(self.root / "missing")
        self.assertNotEqual(self.invoke("sync", old, check=False).returncode, 0)

    def test_deletion_of_index_recovers_outstanding_work_from_durable_spool(self):
        prior, head = self.epoch(7), self.epoch(8)
        (prior / "tap.complete").write_text("verified by worker")
        self.invoke("sync", head)
        (self.root / "pending-work.sqlite3").unlink()
        self.assertEqual(self.select(head, "tap"), [])
        self.assertEqual(self.select(head, "fiat"), [prior])


if __name__ == "__main__":
    unittest.main()
