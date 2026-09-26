---
type: Reference
title: "The Replicated Contract"
description: "intercom/contract/contract.js \u2014 the two dispatch lanes, operation families, state key layout, settlement math, and key constants."
tags: [architecture, contract, intercom, ledger, operations, state]
generated: { by: "codex/gpt-5", at: "2026-09-02T00:00:00+02:00"}
verified:
  - { by: "process:local-code-inspection", at: "2026-08-02T00:00:00+02:00"}
  - { by: "process:local-code-inspection", at: "2026-09-02T00:00:00+02:00"}
status: stable
stale_after: 2026-09-09T00:00:00+02:00
sources:
  - { id: "contract-source", resource: "../../../intercom/contract/contract.js", title: "Intercom Mayhem contract"}
  - { id: "proto-constants", resource: "../../../crates/mayhem-proto/src/lib.rs", title: "Rust protocol constants"}
---

# The Replicated Contract

`intercom/contract/contract.js` (24,690 lines, `CONTRACT_VERSION 21`). Two classes over `trac-peer`:
`MayhemProtocol` (tx routing/CLI, `protocol.js`) and `MayhemContract` (all logic).

## Two dispatch lanes
1. **`/tx` commands** — `mapTxCommand` maps a JSON `op` to a contract method + schema. Byte cap
   64,000. Many ops are mapped to null here and MUST come through the feature path.
2. **Feature records** — the `mayhem_feature` router (`contract.js:1293`), a large `if (value.op ===
   …)` dispatch calling `applyXxx` handlers; writes an idempotent result at `fr/<hash>`. Self-signed
   ops (consent, provider lifecycle, payout binding, spend reserve, deposits) are checked before the
   admin gate.

Schema validation: fastest-validator `$$strict` schemas + hand-rolled exact-key checks +
canonical-key recomputation (a feature's `key` must equal a deterministically derived key). Admin
identity is the `admin` state key; `requireAdmin` gates governance ops (the write-boundary law).

## Operation families
- **Governance (admin /tx):** `set_rules` (monotonic ver), `set_params` (scheduled, ≥1-day
  activation delay), `set_payments`, `set_ctx_brackets`.
- **Provider lifecycle (self-signed feature):** `register_provider` (starts active, rails=[fiat],
  probation), `set_provider_rails` (dual-lane: also accepted as a self-signed /tx,
  `contract.js:3547`), `tap_account_bind` (self-signed TAP address binding),
  `join_enclave`/`leave_enclave`, `join_room`/`leave_room`. Intents
  signed with domain `mayhem-provider-lifecycle`. There is no dedicated reactivation op — a provider
  that left re-activates by re-`join_enclave` (preserves `joined_at`). There is no push heartbeat op
  — liveness is proven via auditor `probe_result` with `probe_kind: uptime_tick` (6h cadence). (Live
  heartbeats for routing travel on the P2P bridge, not the contract — see [The Gateway](/architecture/gateway.md).)
- **Trust/accountability (admin /tx):** `set_provider_kyb` (Tier-4), `revoke_provider_kyb`,
  `ban_provider` (tombstones enclaves; reversible ban indexes for provider/device/fingerprint),
  `unban`, `device_rebind`.
- **Catalog/enclaves (admin /tx):** `set_model_ref` (reference au_usd pricing), `publish_catalog`,
  `register_enclave`, `update_enclave`, `set_enclave_min_tier`, `retire_enclave`, `open_room`,
  `close_room`.
- **Pricing:** `set_price` (au_usd, rate-limited 6h, validated per model-class rate-map units and the
  reference band vs modelref — contract default 0.25×–4×, but mainnet deploys 0.0001×–100×, see
  [The Utilization-Indexed Pricing Controller](/market/pricing-controller.md); writes an immutable price record per enclave/ver/ctx_bracket). Market
  auto-update `computeMarketPriceUpdates` runs during epochApply. See [The Utilization-Indexed Pricing Controller](/market/pricing-controller.md).
- **Reputation/audit:** `record_rep_event`, `anchor_reputation`, `auditor_register`/`auditor_slash`,
  `probe_result`, `tier3_bless_measurement`.
- **Epochs/settlement:** `epoch/apply` (admin, paged), `apply_targeted_epoch`, `epoch_commit`
  (permissionless, provisional for `challenge_epochs`=6), `epoch_seal_empty`, `fraud_proof`.
- **Disputes:** `dispute` (refundable $1 bond, timeout 168 epochs, max 8 open per opener
  `max_open_disputes_per_opener`; opener at fault forfeits 25% of the bond
  `dispute_opener_fault_forfeit_bps`=2500), `dispute_resolve` (deposit actions
  `refund|forfeit|partial_forfeit`), `dispute_expire`.
- **Admin relay:** `admin_contract_tx` — an admin-signed envelope relaying a contract /tx as a
  feature intent.
- **Rate oracles (admin feature):** `rate_oracle` (TNK/USD), `tap_rate_oracle` (TAP/USD); sources
  gate-spot/mexc-spot; staleness 45m.
- **Payout bindings (permissionless verified feature):** `publish_payout_context`,
  `schedule_payout_parameter`, `verify_stripe_payout`, `bind_provider_payout`. The sole writer
  appends valid bindings, but the authority is the provider signature plus target-wallet ownership
  proof or Stripe verification, not a human admin approval. See [Provider Payouts](/payments/payouts.md).
- **Spend reservations:** `spend_reserve`/`spend_reserve_targeted`.
- **Settlements (admin, treasury-signed):** `settle_targeted_tnk`, `settle_targeted_fiat`,
  `fiat_dust_sweep`.
- **Deposits/chargebacks:** `deposit_tnk`→`tnk_deposit`, `tap_deposit`/`tap_deposit_reversal`,
  `fiat_deposit`, `fiat_chargeback`.

## Settlement math
Per rail: `feeDelta` from `fee_bps` (cap 1500); TAP burn `TAP_BURN_BPS=1000` else 0;
`grossAfterFees = gross − fee`; `providerDelta = grossAfterFees − burn` credited to
`earn/<rail>/<provider>` with holdback buckets (new provider 168 epochs, normal 24).
`guardianCheckEpochApply` enforces conservation (balances never negative, provider earnings ≤
verified user spend). Full detail: [Epoch Settlement and Fraud Proofs](/payments/settlement-and-fraud-proofs.md).

Contract v21 applies receipt-bearing epochs as bounded, immutable pages and commits the epoch only
after every expected page has landed. Page indexes, byte bounds, hashes, totals, and the final-page
marker are contract-validated; retries are idempotent. This prevents a large retained receipt set
from being truncated, partially applied, or silently reinterpreted during settlement.

## State key layout (prefixes)
`admin`, `rules/*`, `params/*`, `payments/current`, `consent/*`, `prov/<id>`,
`serve/<provider>/<enclave>`, `roomserve/*`, `enclave/<id>`, `room/<id>`, `modelref/<model>`,
`catalog/release/<id>`, `tierpolicy/*`, `tier3/measurement/*`, `kyb/*`,
`ban/{provider,device,fingerprint}/*`, `auditor/*`, `rep/*`, `bal/<user>/<rail>`,
`hold/<rail>/<user>/<epoch>`, `earn/<rail>/<provider>`, `fee/<rail>/cum`, `burn/<rail>/cum`,
`disp/*`, `epoch/{apply/state,commit,seal,challenge}/*`, `ev/{dep,use,earn,fee,price,probe,slash,
fraud}/*`, `rate/latest`, `tap/rate/latest`, `payout/*`, `settle/*`, `dep/*`, `fr/<hash>`.

## Key constants
`CONTRACT_VERSION 27`, `SESSION_RECEIPT_SCHEMA_VERSION 12`, `SIGNING_MESSAGE_VERSION 2`,
`epoch_seconds 3600`, `challenge_epochs 6`, holdback 24 / new-provider 168, fee cap 1500 bps, TAP
burn 1000 bps, fraud slash 10000 bps, dispute-lost slash 2000 bps, payout_min $1. Market: low/high
utilization thresholds 2000/8000 bps, fixed step 1000 bps, hard reference bounds 2500–40000 bps. Provider count does not gate pricing. Rails exactly
{fiat, tap, tnk}. **No staking** — dispute deposits + reputation holdbacks are the only economic
bonds. Ctx brackets le8k/le32k/le128k/le256k/gt256k. No secrets in these files — only public
constants and signing-domain strings.

Contract v27 uses signed provider compute time and execution-slot capacity to derive absolute utilization for every model class. Prices move by a fixed 10% at the inclusive 20% and 80% thresholds while retaining the existing seed bounds. Receipt schema 12 carries the evidence; model recalibration is not required. Recovery accepts retained signed receipt evidence from contract versions 23 through 26. See [the coordinated upgrade procedure](../../market-utilization-pricing-v27.md).
