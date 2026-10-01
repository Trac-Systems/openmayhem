---
type: Reference
title: "Frozen-Reference Paid-Demand Pricing"
description: "Contract 29 prices signed paid demand against a frozen activity reference within the 25%–400% calibrated-price band."
tags: [pricing, market, controller, contract, au]
timestamp: 2026-10-01T00:00:00Z
---

# Frozen-reference paid-demand pricing

Each economic enclave/context market has independent bounded state. Runtime architecture, provider count, execution-slot claims and money spent do not determine price direction. Signed settled paid-unit increments are the input for all model classes. Existing session price locks and FIAT/TNK/TAP accounting remain unchanged.

## Reference and target

The first 72 usable observations, beginning with positive paid work, form a frozen reference. Paid units are normalized to hourly rates. Each axis uses the nearest-rank 75th percentile of its positive reference-window rates. Average the normalized axes, then take the nearest-rank 95th percentile of positive aggregate observations as the busy reference. Fewer than twelve positive observations marks thin evidence; it does not invoke a model-specific correction.

For subsequent observations, average paid-unit ratios against the frozen effective axis references and clip the result to 0–1 before retaining it. Let `q` be the mean of the last six known values and `s` the sum of the last 72 known values divided by 72:

`multiplier = min(0.25 + 3.75*q, 1 + 3*s*s)`

Scale the calibrated model-reference rate map directly by this multiplier. Fixed request/minimum terms scale their immutable admin seed; zero terms remain zero. Round to atomic units and clamp inward to the 25%–400% hard band. No ordinary percentage-step cap and no compounding of previous prices applies.

At the default one-hour epoch, 72 hours at the frozen busy rate can reach 400%. From that history, one known empty hour gives 337.5%, and six empty hours give 25%. A steady half-reference rate tends to 175%, rather than re-centering at 100%. The quantized currency terms can remain unchanged for movements below their atomic resolution.

## Evidence and limitations

Complete zero settled work contributes zero, including during outages with no fulfilled work. Missing/unusable evidence holds the current price and does not advance the known-value windows. A newly positive unit absent from the frozen reference also holds and records `unrecognized_paid_axis`; it requires an explicit future governed basis transition, never automatic retuning. Settlement epochs, not execution timestamps, attribute demand. This proves fulfilled work, not queues, independent intent, profitability or lost demand.

State contains at most 72 reference observations or 72 clipped values and sixteen axes. Normal updates read one exact demand-state key per market and do not scan receipt or price history. Schema-4 derivations expose the reference version, reference, status, target, controller-state hash, calibrated currency-reference version, locked accounting evidence and resulting quote.

## Upgrade

Contract 29 requires a coordinated contract cutover. Stop transaction producers while all participants change versions, preserve canonical stores and prove the indexer's signed prefix before resuming. Retained versions 23–28 execute their historical contract logic for authenticated replay. Their signed receipt evidence remains recoverable; receipt schema 12 is unchanged.

At a completed epoch boundary, resolve prior nonempty price commitments, inventory all active price schedules, and run `migrate_market_pricing` to validate bounds and index all markets. The offline `intercom/scripts/prepare-reference-demand-upgrade.mjs` prepares unsigned `bootstrap_market_demand` commands from a complete canonical export. The contract re-reads at most 144 exact historical derivations per market and validates identities, price version and boundary. The trusted admin's complete-export review guarantees first-reference/latest-history selection; the contract does not scan all history to prove that selection.

Bootstrap does not reprice immediately. The next known ordinary observation applies the target. Partial history continues reference formation; absent history is not fabricated. Bootstrap cannot overwrite an already-active reference. Preserve session locks, seed lineage and all currency rails throughout the upgrade.
