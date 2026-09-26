---
type: Reference
title: "Epoch Settlement and Fraud Proofs"
description: "epochApply \u2014 per-rail conservation, the 15% fee and 10% TAP burn, holdback maturation, evidence roots \u2014 plus over-credit and price fraud proofs, disputes, and the MSB read caveat."
tags: [settlement, epochApply, fee, burn, holdback, fraud-proof, dispute, msb]
generated: { by: "human:muffin", at: "2026-07-21T00:00:00Z"}
verified:
  - { by: "process:source-sweep", at: "2026-08-02T00:00:00+02:00"}
  - { by: "process:source-sweep", at: "2026-09-02T12:00:00+02:00"}
status: stable
stale_after: 2026-09-09
sources:
  - { id: "proto", resource: "../../../crates/mayhem-proto/src/lib.rs", title: "Mayhem proto constants"}
  - { id: "contract", resource: "../../../intercom/contract/contract.js", title: "Intercom contract"}
---

# Epoch Settlement and Fraud Proofs

## Session billing before settlement
Deposit on any rail → USD credit in `bal/<user>/<rail>`. A session opens with a user-signed
**spend voucher** locking the price version and full rate terms (see
[Epochs, Price Lock, and Price Provenance](/market/epochs-and-settlement.md)), plus a provider-co-signed **balance reservation**
(`spendReserve`) that adds `max_spend_au` to the per-epoch hold and rejects if reservations would
exceed the user's rail balance ("Insufficient unreserved credit balance"). This is the pre-auth that
closed the free-inference hole (I3-A10): authorization is enforced provider-side where a modified
client can't reach it. The provider then streams work under monotone signed **receipts**
(`SESSION_RECEIPT_SCHEMA_VERSION = 11`) binding session/seq/user/provider/enclave/model, the locked
price fields, usage, `au_owed_cum`, and `prompt_hash`. Receipts are the evidence leaves behind the
epoch usage root.

## epochApply
Admin-submitted and bounded through `targeted_receipt_pages_v1` (contract v21): commit plus page 0
land atomically, subsequent pages are contiguous and idempotent, and the final page carries the
final roots and totals. The recompute step reserves final-page evidence bytes before slicing pages;
retries resume the exact pending page and reject snapshot, apply-hash, gap, or total drift. Within
that envelope, receipt application remains capped by `max_apply_batch` (default 2000):
- **Per-rail conservation:** aggregated user debits must equal provider gross earnings per rail; with
  market usage supplied, market demand must equal gross earnings too.
- **Reservation binding + balance floor:** epoch debits validate against the pre-auth reservations;
  each user balance must cover its debit — no negative balances, no minting.
- **Fee:** `fee_au = gross_au × fee_bps / 10000`, `fee_bps` default **1500 (15%)** with a hard
  ceiling `MAX_OPERATOR_FEE_BPS = 1500` (admin can only lower it). Accrues to `fee/<rail>/cum`. The
  fee funds gas sponsorship and is auto-collected every epoch, never manually withdrawn.
- **TAP burn:** only on the tap rail, `TAP_BURN_BPS = 1000` (10%) of gross → `burn/tap/cum`. TAP
  providers net 75% (gross − 15% fee − 10% burn), mirroring the on-chain split. Fiat/TNK providers
  net 85%.
- **Provider accrual + holdback:** net = gross − fee − burn, into `earn/<rail>/<provider>` with
  `total_au`/`held_au`/holdback buckets. Locked epochs: `holdback_epochs` default 24, new providers
  `new_provider_holdback_epochs` default 168 (one week); release additionally gated by canary-probe
  status and open disputes. Payable = total − held − paid_cum.
- **Roots:** the final page carries per-epoch evidence roots `dep, use, earn, fee, price` validated
  against recomputed totals. The market price controller ([The Utilization-Indexed Pricing Controller](/market/pricing-controller.md)) runs
  inside the same apply.

## Fraud proofs (permissionless, no admin discretion)
Every epoch commit is provisional for `challenge_epochs` (default 6). `fraudProof` accepts two
reasons:
- **Over-credit** (`validateOverCreditFraudProof`): a challenger presents a provider-signed receipt
  whose true `au_owed_cum` contradicts the committed usage; the claimed amount must exceed the actual,
  match the committed `use_au` total, and re-derive the committed usage root when substituted.
- **Price derivation**: proves the committed price root doesn't match the deterministic recomputation
  of the price controller.

**Proof scope:** over-credit retains its single-receipt commitment restriction. v25 price derivation proofs cover one market with at most 128 canonical final receipt heads: commitment verifies the frozen signed usage root and pins the expected work-based derivation. A later calibration change cannot turn an honest commitment into fraud. Larger epochs use bounded receipt pages and consensus-derived prices, not externally asserted price roots. Resolve old nonempty monetary-price commitments before upgrading; their AU-only evidence is not reinterpreted as workload. See [the upgrade procedure](../../market-activity-pricing-v25.md).

Consequences are automatic: the commit is voided, the committer is banned, and a registered-provider
committer is slashed at `fraud_slash_bps` (default 10000 = 100% of held earnings) with provider ban
and enclave tombstone. The challenger runs as a supervised `mayhemd` child
(`mayhem up --fraud-challenger`) using a dedicated hot key with no admin authority. Running the
challenger and verifying receipt signatures in the roller were the fixes for audit finding **C1**
(the only unprivileged fund-theft path — the TAP roller once built payout leaves from unsigned
`au_owed_cum`). See [Security Audits and Posture](/security/audits.md).

## Disputes (subjective, admin-adjudicated)
Separate from fraud proofs: any party opens `dispute()` posting a `dispute_deposit_au` bond
(default $1); resolution is admin-adjudicated with `dispute_lost_slash_bps` default 20%. This is the
only payment path requiring admin judgment; the bond auto-refunds on timeout
(`dispute_timeout_epochs` = 168). Further knobs: `max_open_disputes_per_opener` = 8 caps concurrent
disputes per opener, and an opener found at fault forfeits `dispute_opener_fault_forfeit_bps` = 2500
(25% of the bond) — deposit resolution actions are `refund|forfeit|partial_forfeit`
(`contract.js:59-61,112-114`).

## Catch-up safety
The 2026-07-09 replay/catch-up audit verdict: "CATCH-UP SAFE on every rail." Every watcher persists a
durable cursor and dedupe set; the Intercom Autobase log re-applies deterministically; external
replay is blocked by seen-keys / intent-consume per rail. Never skip an epoch — apply N fully before
N+1; idempotency keys make restart safe.

## Reading a TNK balance on real Trac mainnet (operational caveat)
Easy to get wrong. The real Trac mainnet MSB is channel **`0000trac0network0msb0mainnet0000`**
(bootstrap `acbc3a43…`) — NOT the app subnet's `tk-local-msb-v1`. Sourcing `mayhem-live.env`
overrides the mainnet defaults and silently queries the wrong bus. A reader MUST sync (wait for
validators + replicate the signed Autobase log) or it returns a false `0`. Proven reader: SAUCE
`../tokenized-knowledge/trac/peer-local/msb-balance.mjs` with `.env.mainnet`. The 2026-07-09
zero-balance observation was a historical pre-launch snapshot and must not be used as current
treasury state. Query the canonical MSB for the exact balance fields needed; do not scan the full
ledger or infer current liquidity from this page. See rule R8 in
[Rules, Custody, and Credentials](/00-rules-and-credentials.md).
