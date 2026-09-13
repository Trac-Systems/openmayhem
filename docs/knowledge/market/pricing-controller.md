---
type: Reference
title: "The Activity Momentum Pricing Controller"
description: "Contract v25 prices follow aggregate signed settled work, with a previous-epoch baseline, bounded steps, and hard 25%-400% reference bands."
tags: [pricing, market, controller, contract, au]
timestamp: 2026-09-13T00:00:00Z
---

# The Activity Momentum Pricing Controller

Contract v25 compares each market's aggregate settled activity per second with the immediately previous epoch’s actual activity. Rising work raises its next price; falling work lowers it. Spend and active-provider counts remain settlement evidence but do not determine activity. There is no monetary target or minimum-provider gate.

## Signed work and calibration

Canonical final receipts supply the usage increment above their signed billing baseline. Only dimensions priced by the receipt's locked rate map contribute. Checkpoints, retries and already billed work do not count twice. The controller aggregates across providers within the existing enclave/context market; it never divides by provider count. Separate enclave or context markets retain independent histories.

An admin model reference may carry `activity_calibration` with schema version 1, an evidence `source_hash`, and sorted dimensions `{unit, units, work_us}`. Every priced unit must have a positive calibration. For text, input tokens are normalized by calibrated prefill capacity and output tokens by calibrated decode capacity. Media and workflows use calibrated reference work for their billed dimensions. Work is summed with integer arithmetic as `floor(count × work_us × 1000000 / units)` picoseconds per axis, then divided by epoch seconds. This estimates reference work, not actual provider GPU occupancy.

Existing model references without machine-readable calibration use `relative_dimension_vector_v1`: compare each signed dimension's per-second count against its own immediately previous epoch, then average those dimension ratios with equal weight. Zero/zero axes are omitted. This makes all existing model classes responsive without inventing throughput or adding incompatible raw units. A supplied partial or invalid calibration is rejected. Omission preserves an existing calibration; explicit `activity_calibration: null` clears it and returns to the dimension-vector mode.

## Epoch update

1. The first v25 epoch establishes a baseline and holds the current price. Changing the calibration or activity basis also establishes a fresh baseline for one epoch.
2. Compare the current activity rate against the immediately previous epoch’s actual rate. Ratios use basis points and cap at 50000 (5×); a positive current value above a zero baseline uses that cap. Zero activity below a positive previous epoch has ratio zero. Equal nonzero activity has ratio 10000. Initialized empty epochs, including zero-to-zero, use ratio zero so prices continue stepping down toward their hard floor.
3. Set the desired price to the current price multiplied by that ratio. Move toward it using `market_gain_bps` (default 5000) and the existing `market_max_step_bps` clamp (default 1000, or 10%). Integer rounding retains a minimum one-atto movement where required.
4. Clamp rate-map terms to 25%-400% of the model reference. Clamp fixed `per_req_au` and `min_session_au` to 25%-400% of their immutable admin seed; a zero seed stays zero. Hard safety corrections take precedence over the step limit.
5. Update the telemetry-only EMA with `market_ema_alpha_bps` (default 2500) and publish `market_activity_momentum` or `market_activity_hold` with the complete derivation. EMA never determines price direction.

For example, doubling calibrated work relative to the previous epoch produces momentum 20000. With the default gain and step, a price of 100 becomes 110. Halving activity produces a downward step. One provider behaves the same as many providers doing the same aggregate work. Stable nonzero work has momentum 10000 regardless of the price paid. Every initialized empty epoch causes a downward step until the hard floor. Hard bounds and integer price granularity still limit movement.

## Coverage, bounds and participation

`migrate_market_pricing` seeds every active base/context schedule into the bounded canonical activity index. Completed settlement and empty-epoch seals update indexed dormant markets with zero activity. There is no permanent thin-market freeze. Operators must complete every migration batch before resuming epoch settlement; see [the v25 rollout procedure](../../market-activity-pricing-v25.md).

The shipped mainnet configuration and template both use `price_min_bps=2500` and `price_max_bps=40000`. v25 rejects wider bounds and sanitizes historical unsafe active or pending parameter records before they can take effect. Admin seed updates retain existing scheduling and rate limits. Historical wider settings are not the v25 policy.

Min-ask and max-bid still gate participation. Unserved requests and unspent reservations do not create verified completed work: current signed evidence does not establish unmet demand or distinguish withholding from absent demand. Consequently a saturated market with flat completed work need not rise merely because a queue grows. Adding funded admitted/unmet demand requires a separately specified signed, deduplicated, expiring evidence protocol.

Funded wash activity can influence prices within the step and reference bounds. Signed settlement makes that activity accountable and costly; it cannot prove independent economic intent. Provider-advertised capacities, claims of demand and AU totals cannot directly change the controller.

Session price locks, rail conservation, evidence locks and deterministic consensus remain in force. See [price provenance](epochs-and-settlement.md) and [settlement proofs](../payments/settlement-and-fraud-proofs.md).
