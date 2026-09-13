---
type: Reference
title: "Reservation Bands \u2014 Min-Ask and Max-Bid"
description: "How providers (min-ask floor) and users (max-bid ceiling) gate participation without ever naming the price, and why the market clears at one uniform price."
tags: [pricing, min-ask, max-bid, routing, bands]
timestamp: 2026-07-21T00:00:00Z
---

# Reservation Bands — Min-Ask and Max-Bid

The two-sided band (I3-F2). Both sides are **participation gates**, not price setters. Neither side
names a price; opting in or out changes `active_supply` / `active_demand`, which changes next
epoch's settled activity, which moves the one clearing price. Under a uniform clearing price truthful
bands are approximately optimal (no strategic shading). This is why the answer to "can I just charge
2× as a provider?" is: you set a floor, and if the market clears below it you sit out — you never
name the price directly. See [The Utilization-Indexed Pricing Controller](/market/pricing-controller.md).

## The comparison basis
Both bands compare against `rate_gate_basis_au` — the price of one standardized 1000-unit basket of
every priced unit (`Σ ceil(per_unit_au × 1000 / granularity)`), falling back to
`max(per_req_au, min_session_au)` for fixed-only schedules. Request volume never enters it. Example:
rate_map(20, 60) → basis 85 (including the derived cached rate of 5). Source
`crates/mayhem-gateway/src/pricing.rs`.

## Provider min-ask (rate floor)
- Heartbeat field `min_ask_au`. Routing eligibility rejects a route with
  `IneligibilityReason::ProviderMinAsk` when `min_ask_au > market_rate_au`
  (`crates/mayhem-gateway/src/provider_table.rs:954`).
- Being priced out is **not** a penalty — it just makes the provider ineligible to route until the
  clearing price rises to meet the ask (honest refusals are never reputation failures, I3-F5).
- Default **0** = "serve at whatever the market clears."
- Set per market: `mayhem provider min-ask set|get <enclave|model[:tier]> <AU>`; config key
  `provider.min_ask`.
- A higher ask may exclude a provider but does not itself raise the price. Provider count is evidence only; the controller responds to aggregate completed work. Unserved intent cannot substitute for signed settled usage.

## User max-bid (rate ceiling)
- `RequestRequirements.max_price_au`. Eligibility rejects a route with `IneligibilityReason::Price`
  when `market_rate_au > max_price_au`.
- Default **off** (pay market).
- Set via persistent config `user.max_price_au` / `mayhem config max-price`, or per request with
  header `X-Mayhem-Max-Price-Au`.
- A request whose ceiling excludes every route gets a clean 400 ("no provider route is at or below
  X-Mayhem-Max-Price-Au") — not a charge, not a provider failure. A high max-bid does **not** bid
  the price up; the user simply pays the clearing price and keeps the surplus.

## What the bands cannot do (a known limitation)
There is no order-book crossing. Participation bands gate the current common price. A provider that prices itself out does not create a price increase through a smaller provider denominator. One-provider markets can rise or fall after their first activity baseline epoch. With no settled activity after a positive baseline, prices fall within the hard bounds. See [the activity controller](pricing-controller.md).

## Routability
A market is "routable" iff it has ≥1 live eligible route: `availability() = "routable"` when
`route_count > 0`; canonical market info additionally requires `providers_online > 0`. A stale
`roomserve` ledger row can make a route count as routable without a fresh-heartbeat provider behind
it, but the session path filters on fresh (<60s TTL) heartbeats, so users cannot actually be routed
to a ghost. See [The Gateway](/architecture/gateway.md).

## Other routing filters (user-side, header-driven)
`X-Mayhem-Min-Att-Tier`, `X-Mayhem-Min-Ctx`, `X-Mayhem-Quant`, `X-Mayhem-Hedge`,
`X-Mayhem-Min-Tok-S`, `X-Mayhem-Max-Wait-Ms`. All are routing filters, not refunds: an unmet
condition fails the request up front; you are never silently served something worse or billed more.

The same eligibility function also enforces two probation gates on new providers:
`IneligibilityReason::ProbationPriceCap` (`probation_price_max_bps`,
`crates/mayhem-gateway/src/provider_table.rs:985`) and `ProbationConcurrentLimit`
(`probation_max_concurrent_sessions_per_user`, default 2, `provider_table.rs:977`).
