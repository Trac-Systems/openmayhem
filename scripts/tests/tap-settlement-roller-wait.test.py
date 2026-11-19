import fcntl
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


WRAPPER = Path(__file__).resolve().parents[1] / "ops/run-tap-settlement-roller.sh"
ROOT = "0x" + "a" * 64
START = 10000

# Only the spool loop, report binding and gates run. Neither Node entry point
# can be invoked. The second dry-run can model a cancelled/replaced proposal.
COMMANDS = """
flock() { python3 -c 'import fcntl; fcntl.flock(8, fcntl.LOCK_EX | fcntl.LOCK_NB)'; }
node() { [[ ${1##*/} == mainnet-proof.mjs ]] || exit 90; echo proof >> calls; echo '{}'; }
timeout() {
  case ${3##*/} in
    tap-finalized-proposal.mjs)
      echo "read-finalized-header" >> calls
      [[ ! -f refuse-header ]] || return 92
      echo '{"number":123,"timestamp":9999}' ;;
    verify-tap-sweep-report.mjs)
      if [[ $8 == confirmed && $# == 9 ]]; then
        [[ $(jq -r .number "$9") == 123 ]] || return 93
        echo verify-confirmed-header >> calls
      else
        echo "verify-${!#}" >> calls
      fi ;;
    tap-settlement-roller.mjs)
      if [[ ${!#} == --confirm ]]; then
        echo confirm >> calls
        cat report.json
      else
        now=$(cat clock)
        echo "dry $now" >> calls
        if (( now >= stop_at || now < start_at )) || [[ -e cancelled ]]; then
          touch stop
          if [[ -f next-report.json ]]; then
            mv next-report.json report.json
            cat dry.json
          else
            echo '{"blocked":true}'
          fi
        else
          cat dry.json
        fi
      fi ;;
    *) exit 91 ;;
  esac
}
sleep() {
  echo "poll $1" >> calls
  [[ ! -e stop ]] || exit 0
  echo "$(( $(cat clock) + $1 ))" > clock
}
"""

# Give the timer a wall clock and an independent elapsed-time clock. All
# changes are confined to Python children of this fixture in its temp dir.
CLOCK = """
import os, time
from pathlib import Path
wall = Path("clock")
elapsed = 0.0
real_sleep = time.sleep
def sleep(seconds):
    global elapsed
    assert 0 < seconds <= 3600
    if Path("hold").exists():
        Path("sleeper").write_text(str(os.getpid()))
        real_sleep(30)
    elapsed += seconds
    now = int(wall.read_text()) + seconds
    jump = Path("jump")
    if jump.exists():
        now += int(jump.read_text())
        jump.unlink()
    wall.write_text(str(int(now)))
time.sleep = sleep
time.time = lambda: float(wall.read_text())
time.monotonic = lambda: elapsed
"""


class TapPendingWaitTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)
        self.spool = self.dir / "spool"
        ready = self.spool / "ready"
        ready.mkdir(parents=True)
        bundle = {"rail": "tap", "epoch": 1, "epoch_apply_hash": "b" * 64, "params": {"fee_bps": 1500}}
        (ready / "epoch-1.receipts.json").write_text(json.dumps(bundle))
        self.write("clock", str(START))
        self.write("commands", COMMANDS)
        self.write("sitecustomize.py", CLOCK)
        self.write("dry.json", json.dumps({"root": ROOT, "propose_root_dry_run": {"ok": True}}))
        self.env = {
            "PATH": os.environ["PATH"], "BASH_ENV": str(self.dir / "commands"),
            "PYTHONPATH": str(self.dir), "MAYHEM_REPO": str(self.dir / "empty"),
            "MAYHEM_TAP_SETTLEMENT_SPOOL": str(self.spool), "MAYHEM_NETWORK": "mainnet",
            "MAYHEM_TAP_ETH_CHAIN_ID": "1", "MAYHEM_TAP_ETH_RPC": "unused",
            "MAYHEM_TAP_OPERATOR_ADDRESS": "unused",
            "MAYHEM_TAP_POOL_ADDRESS": "0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28",
            # Invalid secp256k1 scalars. Only the wrapper's shape check sees them.
            "MAYHEM_TAP_ROLLER_PRIVATE_KEY": "0x" + "0" * 64,
            "MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY": "0x" + "f" * 64,
            "start_at": str(START), "stop_at": str(START + 3600),
        }

    def write(self, name, value):
        (self.dir / name).write_text(value)

    def report(self, deadline, *, pending_deadline=None, **changes):
        pending = {"merkle_root": ROOT, "epoch": 1, "cumulative_spent_wei": "1",
                   "execute_after": deadline if pending_deadline is None else pending_deadline}
        report = {"epoch": 1, "root": ROOT, "pending_root": pending, "posted": False,
                  "awaiting_governance_delay": True, "canonical_preparation_confirmed": True,
                  "root_confirmed": False, "root_pending": True, "cumulative_spent_wei": "1",
                  "execute_after": deadline, **changes}
        self.write("report.json", json.dumps(report))

    def run_worker(self, *, archived=False):
        result = subprocess.run(["bash", WRAPPER], cwd=self.dir, env=self.env, capture_output=True,
                                text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        location = "processed" if archived else "working"
        self.assertTrue((self.spool / location / "epoch-1.receipts.json").is_file())
        if not archived:
            self.assertEqual(list((self.spool / "processed").iterdir()), [])
        return (self.dir / "calls").read_text().splitlines()

    def test_committed_fence_preserves_working_evidence_after_publisher_crash(self):
        anchor = "b" * 64
        name = "epoch-1-" + anchor
        working = self.spool / "working"
        working.mkdir()
        (self.spool / "ready/epoch-1.receipts.json").rename(working / (name + ".receipts.json"))
        report = working / (name + ".dry-run.json")
        report.write_bytes(b"original attempt evidence")
        deferred = self.spool / "deferred"
        deferred.mkdir()
        (deferred / "next-sweep.json").write_text(json.dumps({
            "epoch": 1, "anchor": anchor, "proof_sha256": "c" * 64, "reserved": True,
        }))
        self.write("stop", "")
        completed = subprocess.run(["bash", WRAPPER], cwd=self.dir, env=self.env,
                                   capture_output=True, text=True, timeout=10)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual((self.dir / "calls").read_text().splitlines(), ["proof", "poll 30"])
        self.assertEqual(report.read_bytes(), b"original attempt evidence")

    def test_full_delay_then_revalidate_changed_root(self):
        self.report(START + 3600)
        calls = self.run_worker()
        self.assertEqual(calls, ["proof", f"dry {START}", "verify-dry", "confirm", "verify-confirmed",
                                 f"dry {START + 3600}", "verify-dry", "poll 30"])

    def test_near_deadline_and_clock_steps(self):
        for remaining, jump, wake in [(7, 0, 7), (3600, 3600, 3630), (60, -300, -240)]:
            with self.subTest(remaining=remaining, jump=jump):
                self.write("clock", str(START))
                self.write("jump", str(jump))
                self.write("calls", "")
                self.report(START + remaining)
                self.env["stop_at"] = str(START + remaining)
                calls = self.run_worker()
                self.assertEqual([c for c in calls if c.startswith("dry ")], [f"dry {START}", f"dry {START + wake}"])

    def test_stale_or_outlying_deadline_uses_bounded_retry(self):
        self.env.update(MAYHEM_TAP_SETTLEMENT_INTERVAL_SECONDS="999999", stop_at=str(START + 1))
        for deadline in [START, START - 1, START + 3601, 10**400]:
            with self.subTest(deadline=deadline):
                self.write("clock", str(START))
                self.write("calls", "")
                self.report(deadline)
                self.assertIn(f"dry {START + 30}", self.run_worker())

    def test_unverified_report_uses_ordinary_retry(self):
        self.write("stop", "")
        for changes in [{"execute_after": True}, {"execute_after": "13600"}, {"execute_after": START + 1},
                        {"pending_deadline": float(START + 3600)}, {"canonical_preparation_confirmed": False}]:
            with self.subTest(changes=changes):
                self.write("calls", "")
                self.report(START + 3600, **changes)
                self.assertEqual(self.run_worker()[-1], "poll 30")
                self.assertFalse((self.spool / "working/epoch-1.settlement.json").exists())

    def test_expiry_runs_confirmation_and_resumes_normal_finality_polling(self):
        self.report(START + 3600)
        executed = {"root": ROOT, "epoch": 1, "execution_tx": "0x" + "d" * 64,
                    "root_confirmed": True, "awaiting_finality": True,
                    "operator_fee": {"predicted_claimable_wei": "0"}, "burn": {"predicted_claimable_wei": "0"}}
        self.write("next-report.json", json.dumps(executed))
        calls = self.run_worker()
        self.assertEqual(calls[-5:], [f"dry {START + 3600}", "verify-dry", "confirm", "verify-confirmed", "poll 30"])
        bound = json.loads((self.spool / "working/epoch-1.settlement.json").read_text())
        self.assertTrue(bound["awaiting_finality"])

    def test_stop_releases_lock_and_restart_rechecks_before_confirm(self):
        self.report(START + 3600)
        self.write("hold", "")
        with subprocess.Popen(["bash", WRAPPER], cwd=self.dir, env=self.env, stderr=subprocess.PIPE) as worker:
            try:
                for _ in range(300):
                    if (self.dir / "sleeper").exists():
                        break
                    time.sleep(0.01)
                self.assertTrue((self.dir / "sleeper").exists())
                worker.terminate()  # signal the wrapper PID alone
                worker.communicate(timeout=2)
            finally:
                worker.kill()
        with (self.spool / "roller.lock").open() as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.write("cancelled", "")
        self.write("calls", "")
        self.assertEqual(self.run_worker(), ["proof", f"dry {START}", "verify-dry", "poll 30"])

    def test_finalized_checkpoint_requires_fresh_header_before_archiving(self):
        effect = {"completed": True, "predicted_claimable_wei": "0", "remaining_claimable_wei": "0"}
        self.write("report.json", json.dumps({"epoch": 1, "root": ROOT, "posted": True,
            "root_confirmed": True, "execution_tx": "0x" + "d" * 64, "operator_fee": effect,
            "burn": effect, "tap_settlement_checkpoint": {"epoch": 1}}))
        working = self.spool / "working"
        working.mkdir()
        (working / "epoch-1.tap-rate.json").write_text("{}")
        self.write("stop", "")
        self.write("refuse-header", "")
        calls = self.run_worker()
        self.assertIn("read-finalized-header", calls)
        self.assertNotIn("verify-confirmed-header", calls)
        self.assertFalse((self.spool / "processed/latest.json").exists())
        (self.dir / "refuse-header").unlink()
        self.write("calls", "")
        calls = self.run_worker(archived=True)
        self.assertIn("verify-confirmed-header", calls)
        self.assertTrue((self.spool / "processed/epoch-1.receipts.json").is_file())


if __name__ == "__main__":
    unittest.main()
