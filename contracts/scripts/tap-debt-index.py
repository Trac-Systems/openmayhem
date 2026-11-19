"""Pick one outstanding TAP recovery ticket by indexed seek. The caller owns both spool locks."""
import json
import os
from pathlib import Path
import re
import sqlite3
import sys

NAME = re.compile(r"epoch-([1-9][0-9]{0,15})-([0-9a-f]{64})")


def run(db, action, key, payload=None):
    if action == "put":
        match = NAME.fullmatch(key)
        ticket = json.loads(payload)
        if not match or ticket.get("epoch") != int(match[1]) or ticket.get("anchor") != match[2] or ticket.get("reserved") is not True or not re.fullmatch("[0-9a-f]{64}", ticket.get("proof_sha256", "")):
            raise ValueError("Invalid TAP debt ticket")
        encoded = json.dumps(ticket, sort_keys=True, separators=(",", ":"))
        with db:
            db.execute("INSERT OR IGNORE INTO pending VALUES (?, ?)", (key, encoded))
            if db.execute("SELECT ticket FROM pending WHERE name=?", (key,)).fetchone()[0] != encoded:
                raise ValueError("Conflicting TAP debt ticket")
        return
    if action == "delete":
        if not NAME.fullmatch(key):
            raise ValueError("Invalid TAP ticket name")
        with db:
            db.execute("DELETE FROM pending WHERE name=?", (key,))
        return
    if action == "pinned":
        if not NAME.fullmatch(key):
            raise ValueError("Invalid TAP pool slot")
        row = db.execute("SELECT name,ticket FROM pending WHERE name=?", (key,)).fetchone()
        if row is None:
            raise ValueError("TAP pool slot references a missing ticket")
    elif action == "after":
        if key and not NAME.fullmatch(key):
            raise ValueError("Invalid TAP debt cursor")
        row = db.execute("SELECT name,ticket FROM pending WHERE name>? ORDER BY name LIMIT 1", (key,)).fetchone()
        if not row:
            row = db.execute("SELECT name,ticket FROM pending ORDER BY name LIMIT 1").fetchone()
    else:
        raise ValueError("Unknown TAP ticket action")
    return {"name": row[0], "ticket": json.loads(row[1])} if row else None


if __name__ == "__main__":
    os.umask(0o077)
    action, filename, key = sys.argv[1:]
    path = Path(filename)
    if path.is_symlink() or path.name != "pending.sqlite3" or path.exists() and not path.is_file():
        raise ValueError("Invalid TAP debt index")
    # Job markers and canonical proofs determine payment. This index schedules work only.
    db = sqlite3.connect(path, timeout=5)
    try:
        db.execute("CREATE TABLE IF NOT EXISTS pending(name TEXT PRIMARY KEY,ticket TEXT NOT NULL) WITHOUT ROWID")
        print(json.dumps(run(db, action, key, sys.stdin.read(2049) if action == "put" else None)))
    finally:
        db.close()
