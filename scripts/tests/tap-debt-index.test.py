"""The payout timer should inspect one debt even when many earlier tickets exist."""
import importlib.util
import json
import sqlite3
import unittest
from pathlib import Path

source = Path(__file__).resolve().parents[2] / "contracts/scripts/tap-debt-index.py"
spec = importlib.util.spec_from_file_location("tap_debt_index", source)
index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index)


class FencedDebtIndex(unittest.TestCase):
    def test_seek_wraparound_pin_and_idempotent_registration(self):
        db = sqlite3.connect(":memory:")
        self.addCleanup(db.close)
        db.execute("CREATE TABLE pending(name TEXT PRIMARY KEY,ticket TEXT NOT NULL) WITHOUT ROWID")
        names = [f"epoch-{i}-{'f' * 64}" for i in range(1, 2049)]
        db.executemany("INSERT INTO pending VALUES (?,?)", ((name, '{}') for name in names))
        names.sort()
        self.assertEqual(index.run(db, "after", names[-2])["name"], names[-1])
        self.assertEqual(index.run(db, "after", names[-1])["name"], names[0])
        steps = [row[-1] for row in db.execute(
            "EXPLAIN QUERY PLAN SELECT name,ticket FROM pending WHERE name>? ORDER BY name LIMIT 1",
            (names[-2],),
        )]
        self.assertTrue(any("SEARCH" in step and "PRIMARY KEY" in step for step in steps), steps)
        self.assertEqual(index.run(db, "pinned", names[-1])["ticket"], {})
        index.run(db, "delete", names[-1])
        with self.assertRaisesRegex(ValueError, "missing ticket"):
            index.run(db, "pinned", names[-1])
        anchor = 'e' * 64
        extra = f'epoch-3000-{anchor}'
        body = dict(epoch=3000, anchor=anchor, reserved=True, proof_sha256='a' * 64)
        for _ in range(2):
            index.run(db, "put", extra, json.dumps(body))
        with self.assertRaisesRegex(ValueError, "Conflicting"):
            index.run(db, "put", extra, json.dumps(dict(body, proof_sha256='b' * 64)))
        self.assertEqual(index.run(db, "pinned", extra)["ticket"], body)


if __name__ == "__main__":
    unittest.main()
