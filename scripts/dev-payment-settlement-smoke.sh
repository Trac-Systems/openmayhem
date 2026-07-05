#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/dev-payment-settlement-smoke.sh

This smoke is retired. It used the old custodial TNK push-payout path and
contract payoutConfirm / ev/pay evidence, which was removed in I2-C3.

Use the active non-custodial TAP and epoch evidence checks instead:
  node --test contracts/tests/tap-noncustodial-loop.test.mjs
  npm test --prefix contracts
  scripts/dev-epoch-receipt-smoke.sh
USAGE
}

usage >&2
exit 64
