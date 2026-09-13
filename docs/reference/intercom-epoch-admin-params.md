# Intercom Contract Epoch/Admin Parameters

These values are admin-governed through `set_params`. Providers and users do not set them.

The contract state is the source of truth. Runtime consumers that derive epoch evidence from
timestamps must read `params/<key>` (or use `read_params`) and fall back only to the contract
default when no parameter record exists yet.

The source-level map is `PARAM_DEFINITIONS` in `intercom/contract/contract.js`, exported as
`contractParamDefinitions()` for audits and generated inventory checks. The admin operating map is
also exported as `contractEpochAdminParamKeys()` / `contractEpochAdminParamDefinitions()` so tests
can assert the live map, not a hand-maintained prose list. The values listed here are defaults
and bounds, not provider-settable terms.

The current writable source map is exported by `contractEpochAdminParamKeys()` in `intercom/contract/contract.js`; deprecated compatibility records listed below are readable but cannot be set under v25.

## Epoch, Settlement, And Governance

| Parameter | Default | Bounds | Purpose |
|---|---:|---:|---|
| `epoch_seconds` | `3600` | `60 .. 86400` | Settlement epoch length used by rollup/watchers. |
| `challenge_epochs` | `6` | `0 .. 1000000` | Commit/fraud-proof challenge window. |
| `holdback_epochs` | `24` | `0 .. 1000000` | Earnings holdback before provider payout maturity. |
| `min_tier_notice_epochs` | `24` | `1 .. 1000000` | Minimum notice window before an admin enclave min-tier policy can take effect. |
| `max_apply_batch` | `2000` | `1 .. 9007199254740991` | Max `debits.length + earnings.length` per `epochApply` page. Larger epochs use `page` / `last_page`; the active admin param is the runtime cap. |
| `max_market_usage_entries` | `5000` | `0 .. 9007199254740991` | Max `market_usage.length` per `epochApply` page. |
| `max_tnk_settlement_outputs` | `5000` | `1 .. 9007199254740991` | Max TNK settlement transfer outputs accepted for one epoch settlement; the active admin param is the runtime cap. |
| `max_fiat_settlement_outputs` | `5000` | `1 .. 9007199254740991` | Max fiat settlement transfer outputs accepted for one epoch settlement; the active admin param is the runtime cap. |
| `param_activation_delay_seconds` | `86400` | `0 .. 2592000` | Governance delay for future `set_params` changes. |
| `rules_grace_seconds` | `1209600` | `0 .. 31536000` | Rules/app compatibility grace window. |
| `rate_staleness_seconds` | `2700` | `60 .. 86400` | TNK/TAP oracle freshness window. |

## Market Controller

| Parameter | Default | Bounds | Purpose |
|---|---:|---:|---|
| `price_rate_limit_seconds` | `21600` | `0 .. 31536000` | Admin seed `P0` change throttle only; market floats are exempt. |
| `market_target_utilization_bps` | historical | read-only | Deprecated since v25; ignored by activity pricing. |
| `market_ema_alpha_bps` | `2500` | `1 .. 10000` | Telemetry activity EMA weight per epoch; does not determine price direction. |
| `market_gain_bps` | `5000` | `1 .. 10000` | Dampening gain toward the desired price. |
| `market_max_step_bps` | `1000` | `1 .. 10000` | Per-epoch max price movement clamp. |
| `market_cold_start_min_providers` | historical | read-only | Deprecated since v25; ignored by activity pricing. |
| `market_provider_epoch_target_au` | historical | read-only | Deprecated since v25; ignored by activity pricing. |
| `market_max_utilization_bps` | historical | read-only | Deprecated since v25; ignored by activity pricing. |
| `market_below_target_discount_bps` | historical | read-only | Deprecated since v25; ignored by activity pricing. |
| `market_above_target_slope_bps` | historical | read-only | Deprecated since v25; ignored by activity pricing. |

## Runtime Consumers

| Consumer | Admin params read | Notes |
|---|---|---|
| Intercom contract `epochApply` | `epoch_seconds`, `fee_bps`, `max_apply_batch`, `max_market_usage_entries`, `holdback_epochs`, `new_provider_holdback_epochs`, `probation_successful_sessions`, `challenge_epochs`, `canary_probe_holdback_bps`, `canary_probe_release_min_passes` | Applies epoch debits/earnings and provider holdback maturity. New/probationary providers receive the graduated long holdback; once their clean-session counter reaches the active threshold, new earnings use the normal holdback. The apply hash, apply state, and `ev/{dep,use,earn,fee,price}` roots bind the active `epoch_seconds`. |
| Intercom contract `setEnclaveMinTier` / `joinEnclave` | `min_tier_notice_epochs` | Admin min-tier policies must be scheduled at least this many epochs ahead. Join checks the effective policy against the applied epoch state; existing serving rows are not tombstoned by a notice. |
| Intercom contract `tnkSettlement` | `max_tnk_settlement_outputs`, `holdback_epochs`, `new_provider_holdback_epochs`, `probation_successful_sessions`, `challenge_epochs`, `canary_probe_holdback_bps`, `canary_probe_release_min_passes`, `rate_staleness_seconds` | Caps the number of epoch settlement transfer outputs by active admin param, refreshes provider holdbacks against active epoch locks/probe gates, and requires a fresh TNK oracle rate. |
| Intercom contract `fiatSettlement` | `max_fiat_settlement_outputs`, `holdback_epochs`, `new_provider_holdback_epochs`, `probation_successful_sessions`, `challenge_epochs`, `canary_probe_holdback_bps`, `canary_probe_release_min_passes` | Caps the number of Stripe settlement evidence outputs by active admin param and refreshes provider holdbacks/probe gates before marking whole-cent fiat earnings as transferred. |
| Intercom contract `epochCommit` / `fraudProof` | `challenge_epochs`, `epoch_seconds`, `fraud_slash_bps` | Stores `provisional_until_epoch`; challenges remain epoch-count based, not wall-clock based. Commit hashes/records and fraud-proof replay bind the active epoch timing. Provider-committer penalties read the active admin slash percentage. |
| Intercom contract probe/dispute slashing | `fraud_slash_bps`, `dispute_lost_slash_bps` | Canary mismatch, fraud proof, direct dispute-loss reputation events, and dispute resolution all read active admin slash percentages at the event timestamp. |
| Intercom contract rate gates | `rate_staleness_seconds` | TNK/TAP oracle freshness window. |
| Intercom contract market tick | `epoch_seconds`, all `market_*` params | Price derivation evidence records the active constants and epoch timing used for replay. |
| Context bracket governance | `param_activation_delay_seconds` plus `ctx_brackets` schedule | Admin-only `setCtxBrackets` publishes versioned `current`/`pending` tables. Gateway spend vouchers and providers now read `ctx_brackets` from contract state and settle against the pinned table version instead of a hardcoded runtime table. |
| Canonical gateway launched by `mayhem use` | `epoch_seconds`, `ctx_brackets` | Gateway-generated reputation-event commands derive their contract epoch from the active admin value, not a fixed one-hour epoch. Gateway session vouchers derive `ctx_bracket`/`ctx_bracket_table_ver` from the active admin context table. |
| Fiat paygate | `epoch_seconds` | Stripe evidence derives `fiat_deposit` / `fiat_chargeback` epochs from the active admin contract value. Service config `contract.epoch_seconds` is a fallback before a contract param record exists, not the live authority. |
| Admin CLI fee shortcut | `param_activation_delay_seconds` | `mayhem admin fee set` derives its default `effective_at` from the active contract delay. Offline copy/paste mode may fall back to the genesis default `86400` only when no peer RPC state can be read; submit mode fails early if the active delay cannot be read. |
| CLI smoke/audit helpers | `epoch_seconds`, `holdback_epochs`, `challenge_epochs`, `rate_staleness_seconds`, probe params | Terminal reports and TNK settlement planning use the active contract values. The Stripe sandbox credit smoke seeds a non-default local `params/epoch_seconds` and verifies `ev/dep/<epoch>` against that active value. |

## Audit Result

The Intercom contract does not require a repo change to tune epoch economics. No ungoverned
epoch-economy magic numbers remain in the contract: the formerly suspicious constants are
defaults/bounds only and are admin-overridable through `set_params`. The 2026-07-07 follow-up
removed the remaining hidden 5000-entry schema cap from epoch apply and TNK settlement paths,
added explicit `max_market_usage_entries`, and made the active admin params the runtime caps.
The latest audit widened the exported source map to cover the full admin operating parameter set,
including probation, auditor, and payout-threshold controls, so docs and tests cannot silently
omit admin-critical knobs.
The regression tests
`intercom/tests/contract-params.test.js` assert that every epoch/per-epoch operating value in
the exported epoch admin map is registered in `PARAM_DEFINITIONS`, rejected for non-admin
callers, delayed by the active governance window, and read back from active contract state after
activation.
Runtime evidence producers must read the active contract value and may fall back only when the
parameter record does not exist yet.

Re-audit note, 2026-07-07: the remaining `3600`/`86400` epoch-like literals outside
`PARAM_DEFINITIONS` in `intercom/contract/protocol.js` are command examples only. The live
contract paths read active values through `activeParamsAt(...)`, and the generated knob inventory
must stay current with `node scripts/knob-inventory.mjs --check`.

`EPOCH_ROOT_KEYS`, `EPOCH_TOTAL_KEYS`, signing/schema versions, fixed rail identifiers, and
Merkle/hash domain strings are deterministic protocol shape. They are intentionally hardcoded and
not admin-adjustable because changing them would change replay semantics, not epoch economics.

`epoch_seconds` is evidence-bound rather than used as a wall-clock rejection gate for every
backfilled/admin-applied epoch. The explicit `epoch` remains the settlement bucket, while commit,
apply, root, and price-derivation records prove which admin timing value was active.

Context bracket tables are not `set_params` scalar values because they are ordered records, not
single numbers. They are still admin-controlled: the contract accepts only admin
`setCtxBrackets`, stores each version under `ctx_brackets/v/<ver>`, stages activation through the
active `param_activation_delay_seconds`, and validates spend reservations against the active table
while receipts/fraud proofs validate the pinned table version.

## Trust And Economics

| Parameter | Default | Bounds | Purpose |
|---|---:|---:|---|
| `fee_bps` | `1500` | `0 .. 1500` | Operator fee, hard-capped at `1500`. |
| `dispute_deposit_au` | `1000000` | `1 .. 9007199254740991` | Dispute bond. |
| `payout_min_au` | `1000000` | `0 .. 9007199254740991` | Minimum payout/settlement threshold. |
| `probation_successful_sessions` | `50` | `0 .. 1000000` | Sessions needed before probation can clear. |
| `new_provider_holdback_epochs` | `168` | `0 .. 1000000` | Long graduated holdback for providers that have not cleared probation; this delays first payouts, never changes the fee. |
| `probation_seconds` | `604800` | `0 .. 31536000` | Time needed before probation can clear. |
| `probation_max_concurrent_sessions_per_user` | `2` | `1 .. 1000000` | Probation concurrent-session cap. |
| `probation_price_max_bps` | `10000` | `0 .. 1000000` | Probation price cap relative to reference. |
| `probation_weight_bps` | `5000` | `0 .. 10000` | Routing weight while on probation. |
| `auditor_min_reputation_bps` | `8000` | `0 .. 10000` | Minimum auditor reputation. |
| `auditor_min_age_seconds` | `2592000` | `0 .. 315360000` | Minimum auditor age. |
| `canary_match_min_bps` | `9000` | `0 .. 10000` | Canary match threshold. |
| `canary_probe_holdback_bps` | `0` | `0 .. 10000` | Extra probe-gated holdback share. |
| `canary_probe_release_min_passes` | `1` | `0 .. 1000000` | Probe passes needed for gated release. |
| `probe_reward_au` | `5000` | `0 .. 9007199254740991` | Auditor probe reward. |
| `uptime_tick_seconds` | `21600` | `60 .. 2592000` | Uptime probe cadence. |
| `fraud_slash_bps` | `10000` | `0 .. 10000` | Canary mismatch and fraud-proof provider penalty. |
| `dispute_lost_slash_bps` | `2000` | `0 .. 10000` | Provider-fault dispute-loss penalty. |
| `price_min_bps` | `2500` | `2500 .. 40000` | Admin seed price lower bound versus model reference. |
| `price_max_bps` | `40000` | `2500 .. 40000` | Admin seed price upper bound versus model reference; must be at least `price_min_bps`. |

## Intentionally Hardcoded Safety Boundaries

These are not operating knobs: schema versions, fixed rail names (`fiat`, `tap`, `tnk`), `au_usd` denomination, hard ceilings such as `MAX_OPERATOR_FEE_BPS = 1500`, and evidence byte caps. They protect deterministic replay and protocol shape rather than tuning epoch economics. When one of these has an operating value, expose that operating value through an admin param.

Simulation-only values such as `intercom/scripts/market-sim.mjs`'s default epoch length do not drive contract state or production evidence.

Contract v25 removes the six deprecated monetary-utilization fields from writable epoch parameters. Active pricing uses activity EMA alpha, gain, step clamp and hard bounds 2500–40000 bps. Both mainnet manifests use exactly 2500/40000. Existing unsafe active/pending records are suppressed and audited by `migrate_market_pricing`. See [the v25 upgrade](../market-activity-pricing-v25.md).
