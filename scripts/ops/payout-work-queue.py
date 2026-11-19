#!/usr/bin/env python3
"""Index outstanding epoch work under the payout worker's existing exclusive lock.

The index schedules attempts. Only the canonical state and the verified evidence
markers can authorize transfers or establish that a payment completed.
"""

import argparse
import os
from pathlib import Path
import re
import sqlite3
import sys

RAILS = ("fiat", "tnk", "tap")
EPOCH_DIR = re.compile(r"epoch-([1-9][0-9]*)-([0-9a-f]{64})")
MAX_EPOCH = 2**53 - 1
BATCH = 4


def parse_work(path):
    match = EPOCH_DIR.fullmatch(path.name)
    if not match or int(match[1]) > MAX_EPOCH or path.is_symlink() or not path.is_dir():
        raise ValueError("invalid payout work directory: " + path.name)
    return int(match[1]), match[2]


def synchronize(db, path):
    epoch, anchor = parse_work(path)
    for rail in RAILS:
        marker = path / (rail + ".complete")
        if marker.is_symlink() or (marker.exists() and not marker.is_file()):
            raise ValueError("invalid payout marker: " + str(marker))
        if marker.is_file():
            db.execute("DELETE FROM pending WHERE rail=? AND epoch=? AND anchor=?", (rail, epoch, anchor))
        else:
            existing = db.execute("SELECT 1 FROM pending WHERE rail=? AND epoch=? AND anchor=?",
                                  (rail, epoch, anchor)).fetchone()
            if not existing:
                db.execute("INSERT INTO pending(rail,epoch,anchor) VALUES(?,?,?)", (rail, epoch, anchor))


def open_queue(root):
    database = root / "pending-work.sqlite3"
    if database.is_symlink():
        raise ValueError("payout queue must be a regular file")
    db = sqlite3.connect(database, timeout=10)
    version = db.execute("PRAGMA user_version").fetchone()[0]
    if version == 0:
        # Stream just directory names and marker existence once when importing the
        # legacy spool. No receipt contents, ledger history or network reads.
        with db:
            db.execute("BEGIN IMMEDIATE")
            db.execute("""CREATE TABLE pending(
              ticket INTEGER PRIMARY KEY AUTOINCREMENT, rail TEXT NOT NULL,
              epoch INTEGER NOT NULL, anchor TEXT NOT NULL, UNIQUE(rail,epoch,anchor))""")
            db.execute("CREATE INDEX by_turn ON pending(rail,ticket)")
            with os.scandir(root) as entries:
                for entry in entries:
                    if EPOCH_DIR.fullmatch(entry.name):
                        synchronize(db, root / entry.name)
            db.execute("PRAGMA user_version=1")
    elif version != 1:
        raise ValueError("unknown payout recovery index version")
    return db


def select(db, root, current, rail):
    epoch, anchor = parse_work(current)
    # A different anchor for the same epoch is ambiguous until authenticated.
    conflict = db.execute("SELECT 1 FROM pending WHERE rail=? AND epoch=? AND anchor!=? LIMIT 1",
                          (rail, epoch, anchor)).fetchone()
    future = db.execute("SELECT 1 FROM pending WHERE rail=? AND epoch>? LIMIT 1", (rail, epoch)).fetchone()
    if conflict or future:
        raise ValueError("retained payout epoch/anchor conflicts with canonical head")
    if rail == "tap":
        # Roots in the same Ethereum pool have a cumulative predecessor. Keep
        # committed roots ordered; outstanding individual claims need no retry.
        sql = "SELECT ticket,epoch,anchor FROM pending WHERE rail=? AND epoch<? ORDER BY epoch,anchor LIMIT ?"
    else:
        sql = "SELECT ticket,epoch,anchor FROM pending WHERE rail=? AND epoch<? ORDER BY ticket LIMIT ?"
    rows = db.execute(sql, (rail, epoch, BATCH)).fetchall()
    names = []
    with db:
        for ticket, old_epoch, old_anchor in rows:
            name = f"epoch-{old_epoch}-{old_anchor}"
            directory = root / name
            if directory.is_symlink() or not directory.is_dir():
                raise ValueError("indexed payout work is missing or not a real directory: " + name)
            marker = directory / (rail + ".complete")
            if marker.is_symlink() or (marker.exists() and not marker.is_file()):
                raise ValueError("invalid payout marker: " + str(marker))
            if marker.is_file():
                db.execute("DELETE FROM pending WHERE ticket=?", (ticket,))
                continue
            if rail != "tap":
                # Rotate before dispatch. A crash loses no work; the entry
                # remains reachable and the external idempotency key is unchanged.
                db.execute("DELETE FROM pending WHERE ticket=?", (ticket,))
                db.execute("INSERT INTO pending(rail,epoch,anchor) VALUES(?,?,?)", (rail, old_epoch, old_anchor))
            names.append(name)
    return names


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("sync", "select", "has-prior"))
    parser.add_argument("work_dir", type=Path)
    parser.add_argument("rail", nargs="?", choices=RAILS)
    args = parser.parse_args()
    current = args.work_dir.absolute()
    epoch, _ = parse_work(current)
    if (args.action == "sync") != (args.rail is None):
        parser.error("select/has-prior require exactly one rail")
    os.umask(0o077)
    db = open_queue(current.parent)
    try:
        if args.action == "sync":
            with db:
                synchronize(db, current)
        elif args.action == "select":
            for name in select(db, current.parent, current, args.rail):
                sys.stdout.buffer.write(os.fsencode(current.parent / name) + b"\0")
        else:
            previous = db.execute("SELECT 1 FROM pending WHERE rail=? AND epoch<? LIMIT 1",
                                  (args.rail, epoch)).fetchone()
            print("yes" if previous else "no")
    finally:
        db.close()


if __name__ == "__main__":
    main()
