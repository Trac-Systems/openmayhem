#!/usr/bin/env python3
"""Close canonical receipt ingress; print the immutable first cutoff timestamp."""
import json
import os
import subprocess
import sys
import time
import urllib.parse
import urllib.request


def freeze_epoch(epoch, rpc, binary, admin_home):
    key = f"epoch/freeze/{epoch}"

    def read():
        query = urllib.parse.urlencode({"key": key, "confirmed": "true"})
        with urllib.request.urlopen(f"{rpc}/state?{query}", timeout=10) as response:
            record = json.load(response)
        if record.get("confirmed") is not True or record.get("key") not in (None, key):
            raise RuntimeError("epoch freeze response is not confirmed canonical state")
        value = record.get("value")
        if value is not None and (
            value.get("type") != "epoch_receipt_freeze"
            or value.get("epoch") != epoch
            or type(value.get("at")) is not int
            or not 0 <= value["at"] <= 9007199254740991
            or value.get("receipt_index", {}).get("epoch") != epoch
        ):
            raise RuntimeError("invalid canonical epoch freeze")
        return value

    current = read()
    if current is None:
        # This authenticated admin command uses the free Feature path. Retrying
        # an unknown result cannot incur a TNK fee, and the contract deduplicates
        # by epoch while preserving the original timestamp and receipt boundary.
        result = subprocess.run([
            binary, "admin", "epoch-freeze", "--home", admin_home,
            "--rpc-url", rpc, "--epoch", str(epoch), "--at", str(int(time.time())),
            "--submit", "--json",
        ], capture_output=True, text=True, timeout=90)
        deadline = time.monotonic() + 30
        while current is None:
            current = read()
            if current is not None:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"epoch freeze is still unconfirmed (CLI exit {result.returncode})")
            time.sleep(1)
    return current["at"]


if __name__ == "__main__":
    epoch = int(sys.argv[1])
    if not 0 < epoch < 9007199254740991:
        raise SystemExit("invalid epoch")
    print(freeze_epoch(
        epoch,
        os.environ.get("MAYHEM_RPC_URL", "http://127.0.0.1:49223/v1").rstrip("/"),
        os.environ.get("MAYHEM_BIN", "/opt/mayhem/source/target/release/mayhem"),
        os.environ.get("MAYHEM_ADMIN_HOME", "/opt/mayhem/.mayhem-local/live-home"),
    ))
