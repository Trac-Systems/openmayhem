#!/usr/bin/env python3
"""Read-only service inventory and pre/post-maintenance acceptance gate.

Discover installed AND loaded OpenMayhem units, including sleeping wrappers.
Never infer the required service set from process names or a stopped baseline.
No secrets, environment variables or command lines are collected.
"""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

UNIT = re.compile(r"(?:openmayhem|mayhem)-[A-Za-z0-9_.@:-]+\.(?:service|timer)\Z")
PROPERTIES = (
    "Id", "LoadState", "UnitFileState", "ActiveState", "SubState", "Type",
    "Result", "Requires", "Wants", "PartOf", "TriggeredBy",
)
ENABLED = {"enabled", "enabled-runtime"}
USER_MANAGER = False


def systemctl(*args):
    return subprocess.run(
        ["systemctl", *( ["--user"] if USER_MANAGER else [] ), "--no-pager", *args], check=True, text=True,
        capture_output=True, timeout=30,
    ).stdout


def discover():
    names = set()
    for args in (
        ("list-unit-files", "--type=service", "--type=timer", "--no-legend"),
        ("list-units", "--all", "--type=service", "--type=timer", "--plain", "--no-legend"),
    ):
        for line in systemctl(*args).splitlines():
            words = line.lstrip("● ").split()
            if words and UNIT.fullmatch(words[0]):
                names.add(words[0])
    return sorted(names)


def inspect(names):
    units = {}
    # Bound each systemctl response and avoid per-process/per-receipt inspection.
    for offset in range(0, len(names), 64):
        raw = systemctl("show", "--property=" + ",".join(PROPERTIES), "--", *names[offset:offset + 64])
        for block in raw.strip().split("\n\n"):
            props = dict(line.split("=", 1) for line in block.splitlines() if "=" in line)
            name = props.get("Id")
            if name:
                units[name] = props
    return units


def expectations(units, required=(), paused=()):
    expected = {}
    for name, unit in units.items():
        if name in paused:
            expected[name] = "inactive"
        elif name in required or unit.get("UnitFileState") in ENABLED or unit.get("ActiveState") == "active":
            # Timer-driven oneshots are normally inactive between successful runs.
            expected[name] = "healthy" if unit.get("Type") == "oneshot" else "active"
    for name in required:
        expected.setdefault(name, "active")
    for name, unit in units.items():
        if name not in expected and unit.get("Type") == "oneshot" and any(
            expected.get(timer) == "active" for timer in unit.get("TriggeredBy", "").split()
        ):
            expected[name] = "healthy"
    return expected


def dependencies(unit):
    return (unit.get("Wants", "") + " " + unit.get("Requires", "")).split()


def expand_pause_dependencies(units, expected):
    """Inspect the complete pull-in graph only when verifying a deliberate pause."""
    todo = [name for name, state in expected.items() if state != "inactive"]
    seen = set()
    while todo:
        batch = set(todo) - seen
        if len(seen | batch) > 2048:
            raise ValueError("Dependency graph exceeds bounded pause audit")
        units.update(inspect(sorted(batch - units.keys())))
        seen.update(batch)
        todo = [dep for name in batch for dep in dependencies(units.get(name, {})) if dep not in seen]


def problems(units, expected):
    failures = []
    reachable = set()
    todo = [name for name, state in expected.items() if state != "inactive"]
    while todo:
        name = todo.pop()
        if name in reachable:
            continue
        reachable.add(name)
        todo.extend(dependencies(units.get(name, {})))
    for name, desired in sorted(expected.items()):
        unit = units.get(name, {})
        state = unit.get("ActiveState", "missing")
        if unit.get("LoadState") != "loaded":
            failures.append(f"{name}: missing or not loaded")
        elif desired == "inactive":
            if state != "inactive":
                failures.append(f"{name}: intentionally paused but {state}")
            # A written pause must survive parent starts; exclusion alone is unsafe.
            if unit.get("UnitFileState") != "disabled" or name in reachable:
                failures.append(f"{name}: pause needs disabled activation and no required-unit pull-in")
        elif desired == "active" and state != "active":
            failures.append(f"{name}: required active, found {state}")
        elif desired == "healthy" and (state not in {"active", "inactive", "activating"}
                                      or unit.get("Result", "success") != "success"):
            failures.append(f"{name}: oneshot unhealthy ({state}, {unit.get('Result', 'unknown')})")
    return failures



PAYOUT_WORKER = "mayhem-payout-worker.service"
PAYOUT_TIMER = "mayhem-payout-worker.timer"
PAYOUT_BEFORE_FAILURE = f"{PAYOUT_WORKER}: oneshot unhealthy (failed, exit-code)"


def local_machine_sha256():
    return hashlib.sha256(Path("/etc/machine-id").read_bytes()).hexdigest()


def validate_before_approval(baseline, snapshot_sha256, evidence, machine_sha256):
    """Accept one owner-approved, failed writer BEFORE as input to a strict AFTER."""
    if not isinstance(evidence, dict) or set(evidence) != {
        "approval", "role", "scope", "unit", "failure", "baseline_sha256", "machine_sha256",
    }:
        raise ValueError("Invalid BEFORE approval evidence")
    if (not isinstance(evidence["approval"], str) or not evidence["approval"].strip()
            or len(evidence["approval"]) > 256
            or evidence["role"] != "writer" or evidence["scope"] != "system"
            or evidence["unit"] != PAYOUT_WORKER or evidence["failure"] != PAYOUT_BEFORE_FAILURE
            or not re.fullmatch(r"[a-f0-9]{64}", snapshot_sha256)
            or not re.fullmatch(r"[a-f0-9]{64}", machine_sha256)
            or evidence["baseline_sha256"] != snapshot_sha256 or evidence["machine_sha256"] != machine_sha256):
        raise ValueError("BEFORE approval does not match the writer, manager, unit and snapshot")
    expected, units = baseline["expected"], baseline["units"]
    worker, timer = units.get(PAYOUT_WORKER, {}), units.get(PAYOUT_TIMER, {})
    if (baseline.get("schema_version") != 1 or baseline.get("scope") != "system"
            or baseline.get("failures") != [PAYOUT_BEFORE_FAILURE]
            or problems(units, expected) != [PAYOUT_BEFORE_FAILURE]
            or expected.get(PAYOUT_WORKER) != "healthy" or expected.get(PAYOUT_TIMER) != "active"
            or any(worker.get(k) != v for k, v in {"Id": PAYOUT_WORKER, "LoadState": "loaded", "Type": "oneshot",
                   "ActiveState": "failed", "SubState": "failed", "Result": "exit-code"}.items())
            or PAYOUT_TIMER not in worker.get("TriggeredBy", "").split()
            or timer.get("Id") != PAYOUT_TIMER or timer.get("UnitFileState") not in ENABLED
            or timer.get("LoadState") != "loaded" or timer.get("ActiveState") != "active"):
        raise ValueError("BEFORE does not show the sole approved, timer-driven writer oneshot failure")



def main():
    global USER_MANAGER
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--user", action="store_true", help="Inspect the current user's service manager instead of the system manager")
    parser.add_argument("--baseline", type=Path, help="Previously captured inventory; preserve its required/paused set")
    parser.add_argument("--approved-before-oneshot-evidence", type=Path,
                        help="Bind the original failed writer BEFORE to the owner-approved machine, snapshot and oneshot")
    parser.add_argument("--output", type=Path, help="Create a private inventory; refuses to overwrite an existing file")
    parser.add_argument("--required-unit", action="append", default=[])
    parser.add_argument("--paused-unit", action="append", default=[], help="Explicitly paused, disabled unit; checked inactive")
    args = parser.parse_args()
    USER_MANAGER = args.user
    scope = "user" if args.user else "system"
    for name in args.required_unit + args.paused_unit:
        if not UNIT.fullmatch(name):
            parser.error("Invalid OpenMayhem service/timer name")
    if set(args.required_unit) & set(args.paused_unit):
        parser.error("A unit cannot be both required and paused")
    approved = None
    if args.approved_before_oneshot_evidence and (not args.baseline or args.user):
        parser.error("Approved BEFORE evidence requires a system-manager --baseline")
    old = {}
    if args.baseline:
        original = args.baseline.read_bytes()
        baseline = json.loads(original)
        if args.approved_before_oneshot_evidence:
            approved = json.loads(args.approved_before_oneshot_evidence.read_bytes())
            validate_before_approval(baseline, hashlib.sha256(original).hexdigest(), approved,
                                     local_machine_sha256())
        elif baseline.get("schema_version") != 1 or baseline.get("failures") != []:
            parser.error("Baseline must be a successful version-1 acceptance snapshot")
        if baseline.get("scope") != scope:
            parser.error("Baseline must belong to the same system/user manager scope")
        old = baseline["expected"]
        if any(not UNIT.fullmatch(n) or v not in {"active", "inactive", "healthy"} for n, v in old.items()):
            parser.error("Invalid baseline expectations")
        if args.paused_unit or args.required_unit:
            parser.error("Do not alter approved expectations during post-maintenance acceptance")
    names = sorted(set(discover()) | set(old) | set(args.required_unit) | set(args.paused_unit))
    if not names:
        parser.error("No OpenMayhem services found; refusing empty acceptance")
    units = inspect(names)
    expected = expectations(units, args.required_unit, args.paused_unit)
    expected.update(old)
    if "inactive" in expected.values():
        expand_pause_dependencies(units, expected)
    failures = problems(units, expected)
    result = {"schema_version": 1, "scope": scope, "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "units": units, "expected": expected, "failures": failures}
    if approved is not None:
        # The original failed observation remains visible. No entry is removed from AFTER failures.
        result["approved_before_oneshot"] = approved
        worker = units.get(PAYOUT_WORKER, {})
        if (worker.get("Type") != "oneshot" or worker.get("Result") != "success"
                or PAYOUT_TIMER not in worker.get("TriggeredBy", "").split()):
            failures.append(f"{PAYOUT_WORKER}: restored oneshot lacks its timer relationship or successful result")
    encoded = json.dumps(result, sort_keys=True, indent=2) + "\n"
    if args.output:
        import os
        with os.fdopen(os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as handle:
            handle.write(encoded)
    print(encoded, end="")
    return 1 if failures else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"Service inventory failed ({type(error).__name__}); no acceptance recorded.", file=sys.stderr)
        sys.exit(2)
