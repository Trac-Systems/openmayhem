---
type: Reference
title: "The Utilization Pricing Controller"
description: "Contract v27 changes prices from signed provider slot utilization, with fixed 10% steps and hard 25%-400% reference bands."
tags: [pricing, market, controller, contract, au]
timestamp: 2026-09-20T00:00:00Z
---

# The Utilization Pricing Controller

Contract v27 changes each enclave/context market price once per settled epoch from its absolute utilization:

- utilization at or above 80%: increase every price term by 10%;
- utilization at or below 20%: decrease every price term by 10%;
- utilization between 20% and 80%: keep the price unchanged.

The boundary values are inclusive. Repeated high-utilization epochs keep raising the price and repeated low-utilization or empty epochs keep lowering it until an existing bound stops movement. The controller does not compare the current hour with the previous hour and has no dollar revenue target.

## Signed utilization evidence

Every schema-12 final receipt commits two provider-measured values:

- `compute_ms`: elapsed execution time for that request; checkpoint values may only increase;
- `capacity_slots`: the execution concurrency offered by that provider for the signed attempt; it is immutable within the attempt.

The buyer acknowledges the signed provider receipt. Settlement sums `compute_ms` from canonical final receipts and, for each distinct provider observed in a market during the epoch, takes the maximum signed `capacity_slots`. Available slot time is:

`capacity_slot_count × epoch_seconds × 1000`

Utilization is `min(100%, compute_ms / available_slot_time)`. A provider with two slots therefore needs twice as much aggregate compute time as a one-slot provider to reach the same utilization. Text, embeddings, images, audio, video and workflows use the same evidence and thresholds. Catalog `activity_calibration` metadata remains valid, but no longer controls price direction and no recalibration is required for this upgrade.

Only canonical settled work enters utilization. Checkpoints, duplicate receipts, redispatch baselines and already billed work do not count twice. Unserved requests and abandoned reservations do not create utilization evidence.

Receipts retained from before contract v27 do not contain signed slot-time fields. They still settle normally. If an epoch contains one, that market records a `legacy_receipt_hold_v1` derivation and keeps its price unchanged for that epoch rather than inventing utilization. The next epoch containing only schema-12 receipts resumes the normal rule automatically.

## Price update and bounds

The selected multiplier, 9000, 10000 or 11000 basis points, is applied directly to every rate-map term plus `per_req_au` and `min_session_au`. Integer rounding preserves a minimum one-atto term when a nonzero price moves.

The result remains clamped to 25%-400% of the immutable admin seed. A zero fixed-term seed stays zero. Admin seed scheduling, session price locks, provider min asks, buyer max bids, rail conservation and fraud-proof roots remain unchanged.

The price derivation records schema 3, the signed compute total, capacity slot count, utilization, selected multiplier, epoch duration, previous price, seed bounds and result. Empty indexed markets use zero utilization and receive the same 10% downward step.

## Migration and operation

`migrate_market_pricing` must run at a completed epoch boundary after all old nonempty price commitments are resolved. It indexes every active base/context market, preserves current prices and seeds, retains the 25%-400% bounds, and records the 20%/80%/10% policy. The offline generator is `intercom/scripts/prepare-market-activity-upgrade.mjs`.

The upgrade requires contract v27 and receipt schema 12 across the writer, gateways, providers and settlement workers. Contract v27 can settle retained signed receipt evidence from contract versions 23 through 26 without rewriting it; the affected market holds price for that settlement epoch because those receipts have no signed slot-time data. Do not mix contract versions while accepting new work.

Utilization proves completed slot occupancy, not queued unmet demand. A fully queued provider can stay below 80% if jobs fail before producing canonical receipts. Signed evidence makes completed activity accountable; it does not prove independent economic intent.
