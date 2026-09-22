---
type: Reference
title: "Configuration Surface and Admin Parameters"
description: "Map of every configuration surface (CLI, env, TOML, headers, contract admin params), where the generated census lives, and which contract ops consume which admin params."
tags: [configuration, admin-params, knobs, contract, governance, operations]
generated: { by: "human:muffin", at: "2026-07-21T00:00:00Z"}
verified:
  - { by: "process:local-check", at: "2026-08-02T00:00:00+02:00"}
  - { by: "process:source-and-generated-reference-check", at: "2026-09-02T12:00:00+02:00"}
status: needs-generator-fix
stale_after: 2026-09-09
sources:
  - { id: "knob-script", resource: "../../../scripts/knob-inventory.mjs", title: "Knob inventory script"}
  - { id: "contract", resource: "../../../intercom/contract/contract.js", title: "Intercom contract"}
---

# Configuration Surface and Admin Parameters

This is a map, not the census. The census is generated: `docs/reference/knob-inventory.md` and
`.json`, produced by `node scripts/knob-inventory.mjs --write` and verified by
`node scripts/knob-inventory.mjs --check` (a clean check is the B6 proof that code and reference
match). Never hand-edit the generated files. The check requires a built debug CLI at
`target/debug/mayhem`; if that binary is missing, the script fails before comparing inventory
content. Build the debug CLI or run the release workflow before trusting exact counts.

## Taxonomy and census

Per the checked-in generated summary table:

| Surface | Count | Where |
|---|---:|---|
| CLI command pages | 198 | `docs/reference/knob-inventory.md` "CLI Commands And Flags" — every `mayhem` subcommand with per-flag default and when-to-change guidance |
| CLI options/arguments | 2031 | same section (do not duplicate this anywhere; point at the file) |
| Environment variables | 788 | "Environment Variables" section |
| TOML config keys | 78 | "TOML Config Keys" (`config.toml`, `mayhemd-up.toml`) |
| Mayhem HTTP headers | 32 | "Mayhem HTTP Headers" (e.g. `x-mayhem-usage`), with gateway source line citations |
| Intercom contract admin params | 43 | "Intercom Contract Admin Params" — but see the count caveat below |
| Operational defaults | 341 | "Operational Defaults" |

The inventory can lag the contract schema. Source-of-truth order is `PARAM_DEFINITIONS` and `contractEpochAdminParamKeys()` in `intercom/contract/contract.js`, then generated inventory, then prose. v25 removes six deprecated monetary-utilization knobs from writable keys while retaining historical reads. The contract parameter tests assert registered, admin-only, delayed activation and readback semantics.

## Contract admin params, grouped

All are admin-governed through `set_params`; providers and users cannot set them. Changes stage
through the active `param_activation_delay_seconds` (default 86400). Runtime consumers read
`params/<key>` (or `read_params`) from contract state and fall back to the contract default only
when no record exists. Full default/bounds tables: `docs/reference/intercom-epoch-admin-params.md`.

**Epoch, settlement, governance** — `epoch_seconds` (3600), `challenge_epochs` (6),
`holdback_epochs` (24), `min_tier_notice_epochs` (24), `max_apply_batch` (2000),
`max_market_usage_entries` (5000), `max_tnk_settlement_outputs` / `max_fiat_settlement_outputs`
(5000), `param_activation_delay_seconds` (86400), `rules_grace_seconds` (1209600),
`rate_staleness_seconds` (2700).

**Market controller** (see [The Utilization Pricing Controller](../market/pricing-controller.md)) —
`price_rate_limit_seconds` governs admin seed changes, while hard bounds remain
`price_min_bps` (2500) / `price_max_bps` (40000). Contract v27 applies fixed protocol rules:
utilization at or above 80% raises price 10%, utilization at or below 20% lowers price 10%, and
the middle band holds. The former utilization target, EMA, gain, configurable step,
provider-dollar target, minimum-provider gate, utilization cap and curve-slope knobs are readable
historical fields and reject new updates. Run the bounded admin `migrate_market_pricing` plan before
resuming settlement after upgrade.

**Trust and economics** — `fee_bps` (1500, hard-capped), probation set
(`probation_successful_sessions`, `probation_seconds`,
`probation_max_concurrent_sessions_per_user`, `probation_price_max_bps`,
`probation_weight_bps`), `new_provider_holdback_epochs` (168), auditor gates
(`auditor_min_reputation_bps`, `auditor_min_age_seconds`), canary/probe set
(`canary_match_min_bps`, `canary_probe_holdback_bps`, `canary_probe_release_min_passes`,
`probe_reward_au`, `uptime_tick_seconds`), slashing (`fraud_slash_bps`,
`dispute_lost_slash_bps`), dispute economics (`dispute_deposit_au`, `dispute_timeout_epochs`,
`max_open_disputes_per_opener`, `dispute_opener_fault_forfeit_bps`), and `payout_min_au`. Note:
the prose doc still lists `canary_probe_release_min_passes` default 1; the contract default is 2.

## Which contract ops read which params

Condensed from the "Runtime Consumers" table in `docs/reference/intercom-epoch-admin-params.md`
(the authority for detail; see [The Replicated Contract](/architecture/contract.md)):

| Consumer | Params read |
|---|---|
| `epochApply` | `epoch_seconds`, `fee_bps`, `max_apply_batch`, `max_market_usage_entries`, holdback + probation + probe-gate set, `challenge_epochs` |
| `setEnclaveMinTier` / `joinEnclave` | `min_tier_notice_epochs` |
| `tnkSettlement` | `max_tnk_settlement_outputs`, holdback/probe set, `rate_staleness_seconds` |
| `fiatSettlement` | `max_fiat_settlement_outputs`, holdback/probe set |
| `epochCommit` / `fraudProof` | `challenge_epochs`, `epoch_seconds`, `fraud_slash_bps` |
| probe/dispute slashing | `fraud_slash_bps`, `dispute_lost_slash_bps` |
| oracle rate gates | `rate_staleness_seconds` |
| market tick | `epoch_seconds` + all `market_*` params |
| ctx bracket governance (`setCtxBrackets`) | `param_activation_delay_seconds` + versioned `ctx_brackets` tables (records, not scalars, so not `set_params`) |
| gateway (`mayhem use`) | `epoch_seconds`, `ctx_brackets` |
| fiat paygate | `epoch_seconds` (service TOML value is a pre-record fallback only) |
| `mayhem admin fee set` | `param_activation_delay_seconds` for its default `effective_at` |

## Intentionally hardcoded safety boundaries

Not knobs, by design: schema/signing versions, the fixed rail identifiers `fiat`/`tap`/`tnk`,
the `au_usd` denomination, `MAX_OPERATOR_FEE_BPS = 1500` (the `fee_bps` ceiling), evidence byte
caps, `EPOCH_ROOT_KEYS`/`EPOCH_TOTAL_KEYS`, and Merkle/hash domain strings. Changing these would
change replay semantics, not economics, so they stay in code. The standing rule: when one of
these shapes carries an operating value, that value gets exposed as an admin param instead.
Simulation-only defaults (e.g. `intercom/scripts/market-sim.mjs`) drive no contract state.
