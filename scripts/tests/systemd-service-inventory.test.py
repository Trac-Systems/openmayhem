#!/usr/bin/env python3
"""Exercise acceptance using a fake systemctl, with no live-service mutations."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "ops/systemd-service-inventory.py"
STACK = "mayhem-stack.service"
TAP = "mayhem-tap-deposit.service"
TIMER = "mayhem-payout-worker.timer"
ONESHOT = "mayhem-payout-worker.service"


def unit(name, state="active", enabled="enabled", kind="simple", **extras):
    return {"Id": name, "LoadState": "loaded", "ActiveState": state,
            "UnitFileState": enabled, "Type": kind, "Result": "success", **extras}


class InventoryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="mayhem-inventory-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.fixture = self.root / "units.json"
        self.baseline = self.root / "baseline.json"
        fake = self.root / "systemctl"
        fake.write_text('''#!/usr/bin/env python3
import json,os,sys
x=json.load(open(os.environ['INVENTORY_TEST_FIXTURE']))
a=sys.argv[1:];a.remove('--no-pager')
if '--user' in a:a.remove('--user')
cmd=a.pop(0)
if cmd in ['list-units','list-unit-files']:
 for name,u in x.items():
  # A sleeping wrapper has no matching process, but remains an installed unit.
  if cmd=='list-unit-files' or u['ActiveState']=='active': print(name,'enabled' if cmd=='list-unit-files' else 'loaded active running')
elif cmd=='show':
 for n in a:
  if n.startswith('--'):continue
  u=x.get(n,{'Id':n,'LoadState':'not-found'})
  print('\\n'.join(k+'='+v for k,v in u.items())+'\\n')
else:
 raise RuntimeError('Unexpected or mutating systemctl operation: '+cmd)
''')
        fake.chmod(0o755)
        self.env = {**os.environ, "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
                    "INVENTORY_TEST_FIXTURE": str(self.fixture)}

    def run_gate(self, units, *args):
        self.fixture.write_text(json.dumps(units))
        r = subprocess.run(["python3", str(SCRIPT), *map(str, args)], env=self.env,
                           text=True, capture_output=True, timeout=10)
        return r, json.loads(r.stdout) if r.stdout else None

    def test_sleeping_enabled_wrapper_cannot_be_omitted_or_blessed(self):
        r, data = self.run_gate({STACK: unit(STACK), TAP: unit(TAP, "inactive")})
        self.assertEqual(r.returncode, 1)
        self.assertEqual(data["expected"][TAP], "active")
        self.assertIn("found inactive", " ".join(data["failures"]))

    def test_restore_gate_preserves_required_set_even_if_disabled(self):
        units = {STACK: unit(STACK), TAP: unit(TAP)}
        r, _ = self.run_gate(units, "--output", self.baseline)
        self.assertEqual(r.returncode, 0, r.stderr)
        units[TAP] = unit(TAP, "inactive", "disabled")
        r, data = self.run_gate(units, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 1)
        self.assertTrue(any(TAP in f for f in data["failures"]))
        del units[TAP]
        r, data = self.run_gate(units, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 1)
        self.assertIn(TAP + ": missing or not loaded", data["failures"])

    def test_idle_oneshot_is_normal_but_inactive_timer_is_not(self):
        units = {STACK: unit(STACK), TIMER: unit(TIMER, kind=""),
                 ONESHOT: unit(ONESHOT, "inactive", "static", "oneshot", TriggeredBy=TIMER)}
        r, _ = self.run_gate(units)
        self.assertEqual(r.returncode, 0, r.stderr)
        units[TIMER]["ActiveState"] = "inactive"
        r, data = self.run_gate(units)
        self.assertEqual(r.returncode, 1)
        self.assertEqual(len(data["failures"]), 1)

    def test_enabled_timer_does_not_hide_its_failed_worker(self):
        units = {TIMER: unit(TIMER, kind=""), ONESHOT: unit(
            ONESHOT, "failed", "static", "oneshot", TriggeredBy=TIMER, Result="exit-code")}
        r, data = self.run_gate(units)
        self.assertEqual(r.returncode, 1)
        self.assertIn("oneshot unhealthy", " ".join(data["failures"]))

    def test_active_static_worker_is_required_after_restore(self):
        units = {TAP: unit(TAP, enabled="static")}
        r, _ = self.run_gate(units, "--output", self.baseline)
        self.assertEqual(r.returncode, 0)
        units[TAP]["ActiveState"] = "failed"
        r, _ = self.run_gate(units, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 1)

    def test_explicit_pause_must_remain_inactive(self):
        units = {STACK: unit(STACK), TAP: unit(TAP, "inactive", "disabled")}
        r, _ = self.run_gate(units, "--paused-unit", TAP, "--output", self.baseline)
        self.assertEqual(r.returncode, 0, r.stderr)
        units[TAP]["ActiveState"] = "active"
        r, data = self.run_gate(units, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 1)
        self.assertIn("intentionally paused but active", " ".join(data["failures"]))

    def test_pause_exclusion_cannot_hide_an_enabled_or_pulled_in_worker(self):
        units = {STACK: unit(STACK), TAP: unit(TAP, "inactive")}
        r, _ = self.run_gate(units, "--paused-unit", TAP)
        self.assertEqual(r.returncode, 1)
        units[TAP]["UnitFileState"] = "disabled"
        units[STACK]["Wants"] = TAP
        r, _ = self.run_gate(units, "--paused-unit", TAP)
        self.assertEqual(r.returncode, 1)

    def test_empty_inventory_and_broken_baseline_are_rejected(self):
        r, _ = self.run_gate({})
        self.assertEqual(r.returncode, 2)
        r, _ = self.run_gate({TAP: unit(TAP, "inactive")}, "--output", self.baseline)
        self.assertEqual(r.returncode, 1)
        r, _ = self.run_gate({TAP: unit(TAP)}, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 2)

    def test_pause_checks_transitive_dependencies_outside_mayhem_names(self):
        helper = "custom-worker-group.target"
        units = {STACK: unit(STACK, Wants=helper), TAP: unit(TAP, "inactive", "disabled"),
                 helper: unit(helper, "inactive", "static", kind="", Wants=TAP)}
        r, data = self.run_gate(units, "--paused-unit", TAP)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn(helper, data["units"])
        self.assertIn("no required-unit pull-in", " ".join(data["failures"]))

    def test_named_required_missing_unit_is_rejected(self):
        r, data = self.run_gate({STACK: unit(STACK)}, "--required-unit", TAP)
        self.assertEqual(r.returncode, 1)
        self.assertIn(TAP + ": missing or not loaded", data["failures"])

    def test_user_scope_is_explicit_and_cannot_reuse_system_baseline(self):
        units = {STACK: unit(STACK)}
        r, data = self.run_gate(units, "--user", "--output", self.baseline)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(data["scope"], "user")
        r, _ = self.run_gate(units, "--baseline", self.baseline)
        self.assertEqual(r.returncode, 2)
        r, _ = self.run_gate(units, "--user", "--baseline", self.baseline)
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
