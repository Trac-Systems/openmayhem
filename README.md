# Mayhem

Mayhem lets you use admin-approved AI enclaves through a local OpenAI-compatible endpoint while providers supply compute over Trac Intercom. Users run one local gateway, point tools such as opencode at it, and receive signed receipts for the work. Providers opt into canonical enclaves and rooms that the Mayhem admin created; they do not set prices, create canonical rooms, or submit arbitrary models.

The contract is the public evidence ledger. It records canonical catalog anchors, admin-created enclaves and rooms, provider opt-ins, prices, rules, balances, receipts, disputes, and settlement roots. The actual prompts and model responses travel over direct provider sessions, not through the ledger.

> [!TIP]
> If you are not comfortable installing developer tools by hand, open this repository in a frontier AI assistant and ask it to guide you through a real Mayhem install for your OS. Have it read this README, run the commands with you, explain each prompt before you approve it, and keep the terminal copy/paste paths and URLs visible. This is the normal install flow with an assistant beside you, not a separate simplified app.

## How It Works

```text
admin publishes signed catalog + terms
        |
        v
provider joins an admin enclave and room
        |
        v
user starts local gateway at 127.0.0.1
        |
        v
OpenAI client sends a request to the local gateway
        |
        v
gateway chooses a provider, opens a direct Intercom session, and streams output
        |
        v
provider and user co-sign receipts
        |
        v
epoch settlement rolls receipts into claimable provider earnings
```

Mayhem has three planes:

| Plane | What It Does |
|-------|--------------|
| Control plane | The admin signs catalog releases, creates enclaves and rooms, sets prices/rules, and can ban providers. |
| Data plane | The local gateway opens direct provider sessions through Intercom and exposes `/v1/chat/completions`, `/v1/embeddings`, images, and audio-compatible routes. |
| Evidence plane | The contract ledger records public facts: catalog anchors, provider joins/leaves, deposits, receipts/roots, disputes, and settlement evidence. |

The gateway routes for quality, not just availability. It tracks TTFT, throughput, stalls, errors, cooloffs, hedge probes, and circuit breakers, then prefers healthier providers for the next request.

## Roles

| Role | What They Control | What They Cannot Do |
|------|-------------------|---------------------|
| User | Chooses a model, runs a local gateway, pays through supported rails, and can pin minimum attestation tier. | Cannot change prices, canonical rooms, or provider terms. |
| Provider | Runs approved Mayhem software and opts into admin-created enclaves/rooms they can serve. | Cannot set prices, create canonical rooms, or submit arbitrary models to the canonical catalog. |
| Admin | Creates and signs enclaves/catalog entries, opens rooms, sets prices/rules, publishes catalog anchors, and bans providers when needed. | Does not hold provider payout funds in the TAP claim path. |
| Auditor | Runs canary probes and submits signed probe evidence. | Cannot slash without contract-valid, signed, catalog-bound evidence. |

## User Walkthrough

Install Mayhem from a checkout:

```bash
./install.sh --from-source
```

Set up a user wallet and local config:

```bash
mayhem setup --role user
```

Start the local gateway. Keep this terminal open:

```bash
mayhem use \
  --sc-bridge-url "$MAYHEM_SC_BRIDGE_URL" \
  --sc-bridge-token "$MAYHEM_SC_BRIDGE_TOKEN"
```

What this does:

| Command | Purpose |
|---------|---------|
| `mayhem use` | Reads canonical contract state, verifies admin-created model routes, starts the local OpenAI-compatible endpoint, and prints copy/paste dashboard URLs. |
| `--sc-bridge-url` / `--sc-bridge-token` | Let the gateway open direct provider sessions over Intercom. |

In a second terminal, list usable models:

```bash
mayhem models --gateway
```

Check balance and gateway health:

```bash
mayhem balance
mayhem status
```

Run an OpenAI-compatible client:

```bash
opencode run --model mayhem/<model-id> "Say hello from Mayhem."
```

For sensitive prompts, inspect available trust tiers before choosing a route:

```bash
mayhem models --gateway --min-att-tier 3
mayhem models --gateway --require-kyb
```

You can also send routing preferences through OpenAI-compatible request headers, for example `X-Mayhem-Min-Att-Tier: 3`, `X-Mayhem-Hedge: 1`, or failover thresholds such as `X-Mayhem-Min-Tok-S`.

## Provider Walkthrough

Install Mayhem and set up a provider wallet:

```bash
./install.sh --from-source
mayhem setup --role provider
```

List canonical models and provider-eligible routes:

```bash
mayhem models
mayhem provider list
```

Join an admin-created enclave and room:

```bash
mayhem provider start --enclave <admin-enclave-id> --rooms auto --serve-sessions
```

What this does:

| Command | Purpose |
|---------|---------|
| `mayhem models` | Reads the ledger `catalog/current` anchor, fetches signed admin catalog JSON, verifies it, and lists approved catalog content without requiring a repo update. |
| `provider start` | Creates provider opt-in evidence for an existing admin enclave/room and starts serving direct sessions. |
| `--rooms auto` | Selects matching admin-created canonical rooms. Provider-created Intercom rooms are not canonical ledger rooms. |

Inspect provider state:

```bash
mayhem provider health
mayhem earnings
mayhem reputation
```

Opt out:

```bash
mayhem provider stop
```

Providers are paid from settlement evidence. On the TAP rail, providers claim their own cumulative earnings from the escrow proof path; Mayhem does not push custodial payouts.

## Catalog And Enclaves

The public source repo is Mayhem as a whole, not only `intercom/`. The `intercom/` subtree is the embedded Pear/Intercom contract and P2P runtime that Mayhem uses for canonical ledger state and transport.

The model catalog is admin-canonical. Users and providers should discover current catalog content through `mayhem models`, which reads the ledger anchor and verifies the signed catalog release. The local `catalog/` directory is a source and release input, not the only discovery path.

Enclave model bundles are separate release artifacts. They are not committed as repo blobs; production catalog entries point at admin-approved downloads with hashes, sizes, signatures, and provenance.

## Payments And Receipts

All prices are denominated in `mu_usd`, integer micro-USD. Payment rails credit that same unit:

| Rail | Use |
|------|-----|
| TAP | Crypto payment and provider claim rail. Users deposit TAP; providers claim TAP from settlement proofs. |
| Stripe | Fiat checkout rail that credits the same `mu_usd` balance. |
| TNK | Trac Network gas for paid ledger operations, not the user/provider payment unit. |

Each served session produces signed receipt evidence. The hot path avoids per-token ledger writes; receipts roll into epoch settlement roots.

## Dashboards

`mayhem use` serves loopback-only local dashboards:

| Dashboard | Path | Shows |
|-----------|------|-------|
| User | `/mayhem/dashboard` | Balance, sessions, spend history, local gateway status, and model catalog. |
| Provider | `/mayhem/dashboard/provider` | Enclave status, live sessions, earnings, reputation/holdback, and claim commands. |

The server binds `127.0.0.1` only, uses short-lived local tokens, and serves assets locally.

## Install

From this source checkout:

```bash
./install.sh --from-source
```

On Windows PowerShell:

```powershell
.\install.ps1 -FromSource
```

For release artifacts, use the prebuilt archive plus SHA-256 sidecar:

```bash
./install.sh --artifact-url <archive-url> --sha256 <archive-sha256>
```

Installers print a copy/paste `PATH` command even when they update your shell profile. Browser-opening commands, such as hosted payment checkout, print the copy/paste URL before attempting to open a browser.

## Development

```bash
scripts/dev-net.sh --cleanup
cargo build --workspace
MAYHEM_RUN_INTERCOM_TESTS=1 cargo test -p mayhem-bridge --test sc_bridge -- --nocapture
```

Standalone `mayhem-gateway` is a raw development smoke binary and must be started with `--dev-embedded-catalog`. Production/user flows should use `mayhem use`.

Operator, provider, and user docs live in `docs/operator-runbook.md`, `docs/provider-guide.md`, and `docs/user-guide.md`. The iteration plans and trackers in `docs/` record the implementation history.
