import copy
import json
from pathlib import Path
import runpy
import tempfile
import unittest


verify = runpy.run_path(str(Path(__file__).resolve().parents[1] / "ops/check-tap-carry-basis.py"))["verify"]


class TapCarryBasisTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.processed = Path(temp.name)
        self.basis = {"epoch": 7, "epoch_apply_hash": "a" * 64, "root": "0x" + "b" * 64,
                      "cumulative_spent_wei": "12000"}
        token = {"chain_id": 1, "token_address": "token", "pool_address": "pool"}
        self.prior = {**self.basis, "rail": "tap", "root_confirmed": True,
                      "operator_fee": {"completed": True}, "burn": {"completed": True},
                      "entries": [{"account": "beneficiary", "cumulative_wei": "9000"}],
                      "providers": ["provider"], "refunds": [], "tap_rate_lock": token,
                      "tap_settlement_checkpoint": {**self.basis, **token, "root_confirmed": True}}
        self.prior["tap_settlement_checkpoint"].update({
            "entries": self.prior["entries"], "provider_entries": self.prior["providers"], "refunds": []})
        self.archive = self.processed / f"epoch-7-{self.basis['epoch_apply_hash']}.settlement.json"
        self.archive.write_text(json.dumps(self.prior))
        self.report = {**self.prior, "posted": False, "carried_root": self.basis}
        self.latest = self.processed / "latest.json"

    def test_finalized_distribution_survives_carry_and_later_root(self):
        self.latest.write_text(json.dumps({**self.prior, "epoch": 10}))
        original = self.latest.read_bytes()
        verify(self.report, self.processed, {"epoch": 8})
        self.assertEqual(self.latest.read_bytes(), original)

    def test_rejects_missing_archive_changed_beneficiary_or_payment(self):
        for changed in ({"entries": []}, {"refunds": ["another account"]}, {"posted": True}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                verify({**self.report, **changed}, self.processed, {"epoch": 8})
        wrong_chain = copy.deepcopy(self.prior)
        wrong_chain["tap_settlement_checkpoint"]["chain_id"] = 2
        self.archive.write_text(json.dumps(wrong_chain))
        with self.assertRaises(ValueError):
            verify(self.report, self.processed, {"epoch": 8})
        self.archive.unlink()
        with self.assertRaises(FileNotFoundError):
            verify(self.report, self.processed, {"epoch": 8})

    def test_carry_cannot_select_an_older_root_or_invent_a_pre_root_epoch(self):
        newer = {**self.prior, "epoch": 9, "epoch_apply_hash": "b" * 64}
        self.latest.write_text(json.dumps(newer))
        archived = self.processed / f"epoch-9-{newer['epoch_apply_hash']}.settlement.json"
        archived.write_text(json.dumps(newer))
        with self.assertRaises(ValueError):
            verify(self.report, self.processed, {"epoch": 10, "tap_liability_sweep": {}})
        # The current head can be beyond the replay epoch or missing after an
        # interrupted update. Neither case erases the historical settlement.
        for head in ({"epoch": 12}, None):
            if head:
                self.latest.write_text(json.dumps(head))
            else:
                self.latest.unlink()
            empty = {"posted": False, "cumulative_spent_wei": "0", "entries": [], "providers": [], "refunds": []}
            with self.assertRaises(ValueError):
                verify(empty, self.processed, {"epoch": 10, "tap_liability_sweep": {}})
            with self.assertRaises(ValueError):
                verify(self.report, self.processed, {"epoch": 10, "tap_liability_sweep": {}})
            verify(self.report, self.processed, {"epoch": 8, "tap_liability_sweep": {}})

    def test_archived_carries_do_not_replace_the_last_finalized_distribution(self):
        carry = {**self.report, "status": "no_work", "outcome": "carry", "root_confirmed": False,
                 "tap_settlement_checkpoint": None, "epoch": 8, "epoch_apply_hash": "c" * 64}
        path = self.processed / f"epoch-8-{carry['epoch_apply_hash']}.settlement.json"
        path.write_text(json.dumps(carry))
        self.latest.write_text(json.dumps(self.prior))
        verify(carry, self.processed, {"epoch": 8, "tap_liability_sweep": {}})
        verify(carry, self.processed, {"epoch": 9, "tap_liability_sweep": {}})
        for field, value in (("proofs", {"new": []}), ("provider_claimed_wei", "1")):
            with self.assertRaises(ValueError):
                verify({**carry, field: value}, self.processed, {"epoch": 8, "tap_liability_sweep": {}})
        self.assertEqual(json.loads(self.latest.read_bytes()), self.prior)

    def test_pre_root_carry_validates_before_first_settlement_only(self):
        report = {"posted": False, "root": None, "cumulative_spent_wei": "0",
                  "entries": [], "providers": [], "refunds": []}
        verify(report, self.processed, {"epoch": 5})
        self.latest.write_text(json.dumps(self.prior))
        verify(report, self.processed, {"epoch": 5})
        with self.assertRaises(ValueError):
            verify(report, self.processed, {"epoch": 8})


if __name__ == "__main__":
    unittest.main()
