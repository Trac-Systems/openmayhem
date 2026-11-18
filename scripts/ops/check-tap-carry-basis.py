"""Verify the distribution underlying a TAP carry against its archived checkpoint."""

import json
import re
import sys
from pathlib import Path


def check_last_root(processed, target_epoch, basis, prior=None):
    # latest can already be ahead during historical readback, or lag the
    # epoch archive if the roller stopped between the two publications.
    basis_epoch = basis["epoch"] if basis else 0
    latest = processed / "latest.json"
    if latest.exists():
        head = json.loads(latest.read_bytes())
        if head["epoch"] <= target_epoch and (not basis or head != prior):
            raise ValueError("carry does not use the latest finalized root")

    for path in processed.glob("epoch-*-*.settlement.json"):
        match = re.fullmatch(r"epoch-([1-9][0-9]*)-([0-9a-f]{64})\.settlement\.json", path.name)
        if not match or not basis_epoch <= int(match[1]) <= target_epoch:
            continue
        if basis and int(match[1]) == basis_epoch and match[2] == basis["epoch_apply_hash"]:
            continue
        item = json.loads(path.read_bytes())
        if (item.get("status") != "no_work" or item.get("outcome") != "carry"
                or item.get("posted") is not False or item.get("tap_settlement_checkpoint") is not None
                or item.get("root_confirmed") is True):
            raise ValueError("carry omits a finalized settlement in the epoch archive")


def verify(report, processed, bundle):
    if report.get("posted") is not False:
        raise ValueError("a carry must not have posted a payment")
    basis = report.get("carried_root")
    if basis is None:
        if (report.get("root") is not None or report.get("cumulative_spent_wei") != "0"
                or any(report.get(key) != [] for key in ("entries", "providers", "refunds"))):
            raise ValueError("nonempty distribution without a carry basis")
        if "tap_liability_sweep" in bundle:
            if (report.get("proofs") not in (None, {}) or report.get("provider_dust_recipient") is not None
                    or any(report.get(key, "0") != "0" for key in ("provider_claimed_wei", "buyer_refund_wei",
                           "total_claimed_wei", "provider_cap_wei", "provider_dust_wei"))):
                raise ValueError("nonempty distribution without a carry basis")
            check_last_root(processed, bundle["epoch"], None)
        else:
            latest = processed / "latest.json"
            if latest.exists():
                head = json.loads(latest.read_bytes())
                if head.get("root") and head["epoch"] <= bundle["epoch"]:
                    raise ValueError("carry omits an existing finalized root")
        return
    if not isinstance(basis, dict) or set(basis) != {"epoch", "epoch_apply_hash", "root", "cumulative_spent_wei"}:
        raise ValueError("invalid carry basis fields")
    epoch = basis["epoch"]
    anchor = basis["epoch_apply_hash"]
    if (type(epoch) is not int or not 0 < epoch < bundle["epoch"]
            or not re.fullmatch(r"[0-9a-f]{64}", str(anchor))
            or not re.fullmatch(r"0x[0-9a-f]{64}", str(basis["root"]))):
        raise ValueError("invalid prior epoch or root")
    archive = processed / f"epoch-{epoch}-{anchor}.settlement.json"
    prior = json.loads(archive.read_bytes())
    checkpoint = prior.get("tap_settlement_checkpoint") or {}
    if (prior.get("rail") != "tap" or prior.get("root_confirmed") is not True
            or checkpoint.get("root_confirmed") is not True
            or any(prior.get(k) != v or checkpoint.get(k) != v for k, v in basis.items())
            or any((prior.get(k) or {}).get("completed") is not True for k in ("operator_fee", "burn"))):
        raise ValueError("prior Ethereum settlement is incomplete or differs from carry basis")
    for key in ("root", "cumulative_spent_wei", "entries", "providers", "refunds"):
        if key not in prior or prior[key] != report.get(key):
            raise ValueError(f"carry changed finalized {key}")
    if "tap_liability_sweep" in bundle:
        check_last_root(processed, bundle["epoch"], basis, prior)
        for key in ("proofs", "provider_claimed_wei", "buyer_refund_wei", "total_claimed_wei",
                    "provider_cap_wei", "provider_dust_wei", "provider_dust_recipient"):
            if report.get(key) != prior.get(key):
                raise ValueError(f"carry changed finalized {key}")
        for prior_key, checkpoint_key in (("entries", "entries"), ("providers", "provider_entries"),
                                          ("refunds", "refunds")):
            if checkpoint.get(checkpoint_key) != prior[prior_key]:
                raise ValueError(f"archive differs from checkpoint {checkpoint_key}")
    for key in ("chain_id", "pool_address", "token_address"):
        value = (prior.get("tap_rate_lock") or {}).get(key)
        if value is None or value != checkpoint.get(key) or value != (report.get("tap_rate_lock") or {}).get(key):
            raise ValueError(f"carry changed {key}")


if __name__ == "__main__":
    try:
        verify(json.loads(Path(sys.argv[1]).read_bytes()), Path(sys.argv[2]),
               json.loads(Path(sys.argv[3]).read_bytes()))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        sys.exit(f"Invalid TAP carry basis: {exc}")
