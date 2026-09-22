---
type: Reference
title: "Epochs, Price Lock, and Price Provenance"
description: "The hourly epoch as the unit of settlement and price recomputation, the per-session price lock, and how every published price carries a recomputable derivation."
tags: [epoch, settlement, price-lock, provenance, fraud-proof]
timestamp: 2026-07-21T00:00:00Z
---

# Epochs, Price Lock, and Price Provenance

## The epoch
An epoch is the settlement window, `epoch_seconds` default **3600s (1 hour)**, admin-adjustable
60–86400. It is the unit of: evidence root computation, market-price recomputation, reputation
folds, and holdback maturation. At the end of each epoch all signed receipts settle, and that
settlement doubles as the signed workload input for the next epoch's price. See
[The Utilization-Indexed Pricing Controller](/market/pricing-controller.md) and [Epoch Settlement and Fraud Proofs](/payments/settlement-and-fraud-proofs.md).

## Per-session price lock (I3-F3)
This is the load-bearing fix that makes a floating price safe. At session open, the actual resolved
terms — `price_ver`, full `locked_rate_map`, `locked_per_req_au`, `locked_min_session_au`,
`served_ctx`, ctx bracket — are frozen into the user-signed spend voucher. The contract validates
the locked terms byte-for-byte against the stored versioned price record `price/{enclave}/v/{ver}`
(must be admin-set-role, matching ctx bracket). Settlement, fraud proofs, and balance checks all
validate against the **locked** rate forever, never the current floating price. Open sessions
survive the market `price.ver` advancing; no completed session is ever recomputed at a different
price. Without the lock the float would invalidate in-flight sessions; without the float the lock
would freeze the market — they exist as a pair (CONCEPTS.md §2).

## Price provenance (I3-F8) — every price is recomputable
Each completed bounded epoch hashes consensus-derived price updates into its market price evidence root. Derivations bind signed compute time, execution-slot capacity, utilization, epoch duration, thresholds, multiplier, seed, previous terms and result. The ordinary roller commits an empty external price root; the contract computes the actual market-price evidence after validating all canonical receipt pages. Empty seals also bind their zero-utilization price evidence.

## Price fraud proof
Explicit nonempty price commitments require one market and at most 128 canonical final receipt heads. At commit time the contract verifies the frozen signed usage root and pins its expected utilization derivation. A challenger can prove a contradictory price root within the challenge window without relying on later mutable price state. Larger epochs use bounded consensus computation. Historical monetary-price commitments must be resolved before upgrading; see [the v27 rollout procedure](../../market-utilization-pricing-v27.md).

## Where the money math lives
Settlement itself (`epochApply`) runs the fee (15%), the TAP burn (10%), per-rail conservation
(debits == earnings), holdback maturation, and writes the evidence roots `dep, use, earn, fee,
price`. The market price controller runs inside the same apply. Full detail:
[Epoch Settlement and Fraud Proofs](/payments/settlement-and-fraud-proofs.md).
