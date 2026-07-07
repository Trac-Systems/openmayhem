# Mayhem

Mayhem is a local OpenAI-compatible gateway for a public, admin-canonical inference network. Users keep their tools pointed at `127.0.0.1`, providers contribute compute by joining approved enclaves, and the Trac ledger records the evidence: catalog anchors, terms, joins, receipts, disputes, and settlement roots.

The important boundary is simple: **the admin controls the economy and catalog; providers only opt in or out**. Providers cannot set prices, create canonical rooms, or submit arbitrary models. Users choose models and rails, send requests through the local gateway, and receive signed receipts for the work.

Prompts and model responses do not go through the ledger. They travel over direct Intercom sessions between the local gateway and selected providers. The contract is the public evidence layer that lets everyone audit what was offered, served, billed, disputed, and settled.

Deposit credits are admin-oracle evidence from watchers/paygate, not arbitrary
provider or user writes. The trust boundary and key separation are documented in
[docs/design/deposit-trust-boundary.md](docs/design/deposit-trust-boundary.md).

> [!TIP]
> If you are not comfortable installing developer tools by hand, open this repository in a frontier AI assistant and ask it to guide you through a real Mayhem install for your OS. Have it read this README, run the commands with you, explain each prompt before you approve it, and keep the terminal copy/paste paths and URLs visible. This is the normal install flow with an assistant beside you, not a separate simplified app.

## Quickstart

```bash
./install.sh --from-source
mayhem up --yes
curl http://127.0.0.1:11435/v1/models
opencode run --model mayhem/<model-id> "Say hello from Mayhem."
mayhem down
```

Provider machine:

```bash
./install.sh --from-source
mayhem up --provider --yes
mayhem provider health
mayhem down
```

`mayhem up` is the normal entry point. It starts the supervised Pear/Intercom peer, local bridge, gateway, and optional provider worker; prints copy/paste endpoint and dashboard URLs; and works from a terminal even when no browser is available.

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

## How Much Can You Trust A Provider?

Attestation tiers tell you what kind of trust evidence a provider has. They do not all mean the same thing, and a higher number is not always "everything below it plus more."

| Tier | Plain Meaning | Can The Provider Read My Prompt? |
|------|---------------|----------------------------------|
| Tier 1 | Runs the Mayhem software. Trust is mostly economic: if they cheat, probes, receipts, holdbacks, and slashing can cost them money. | Yes. |
| Tier 2 | Proven to be genuine Apple or NVIDIA hardware running the real Mayhem app. This helps stop fake hardware or fake-app claims. | Yes. |
| Tier 3 | Hardware confidential compute. Your prompt is protected even from the provider's own machine. This is the only tier where they should not be able to read what you send, and it is not available yet on our current hardware. | No. |
| Tier 4 | A real, identity-verified business that the Mayhem admin has KYB'd. You know who they are. | Yes. Tier 4 is identity, not prompt privacy. |

The honest shortcut is simple: **only Tier 3 means the provider cannot read your prompt**. **Tier 4 does not make a prompt private**; it means the admin knows the business behind the provider.

## User Walkthrough

Install Mayhem from a checkout:

```bash
./install.sh --from-source
```

Start Mayhem:

```bash
mayhem up --yes
```

`mayhem up` repairs first-run config, creates a wallet if needed, starts the Pear/Intercom peer through the bundled runtime, starts the local bridge and OpenAI-compatible gateway, health-checks everything, and prints copy/paste URLs. It never needs a browser; if a later flow offers a browser redirect, the URL is printed first.

```bash
curl http://127.0.0.1:11435/v1/models
```

What this does:

| Command | Purpose |
|---------|---------|
| `mayhem up --yes` | Starts the whole local user stack from one terminal and prints the OpenAI base URL plus dashboard URL. |
| `curl http://127.0.0.1:11435/v1/models` | Confirms the local OpenAI-compatible endpoint is answering. |
| `mayhem down` | Stops the supervised peer, bridge, gateway, and provider worker. |

List usable models:

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

Share one funded gateway with another machine or agent only after creating a bearer token:

```bash
mayhem tokens create --name laptop --budget 10/day --max-rate 60
mayhem use --bind 0.0.0.0:11435
```

`mayhem tokens create` prints `sk-mayhem-...` once and stores only a hash in your Mayhem home. Non-loopback binds refuse to start until at least one active token exists, then require `Authorization: Bearer sk-mayhem-...` on OpenAI-compatible requests. Loopback stays token-optional unless you add `--require-auth`. Tokens partition access, budgets, rate limits, model allowlists, and spend attribution; all usage still settles from the gateway owner's single balance and identity. WAN exposure should go through a TLS reverse proxy or Tailscale/VPN; the gateway itself serves plain HTTP.

For sensitive prompts, inspect available trust tiers before choosing a route:

```bash
mayhem models --gateway --min-att-tier 3
mayhem models --gateway --require-kyb
mayhem models --gateway --quant int4
```

`--min-att-tier 3` asks for prompt-private routing. `--require-kyb` asks for Tier 4 identity; it does not make prompts private. `--quant` filters live admin enclave routes by their pinned artifact bucket; it is not a separate price key. You can also send routing preferences through OpenAI-compatible request headers, for example `X-Mayhem-Min-Att-Tier: 3`, `X-Mayhem-Quant: int4`, `X-Mayhem-Hedge: 1`, or failover thresholds such as `X-Mayhem-Min-Tok-S`.

Inspect the live market price and set local user ceilings:

```bash
mayhem price show <model-id> --tier 1
mayhem config max-price 250000
mayhem config set --key min-ctx --value 128000
```

`mayhem config max-price` is a persistent default for the local gateway. Per-request clients can override it with `X-Mayhem-Max-Price-Mu`.
`min-ctx` is a persistent context-floor default for `mayhem use`; per-request clients can override it with `X-Mayhem-Min-Ctx`, and terminal users can run `mayhem use --min-ctx 128000`.

Stop Mayhem:

```bash
mayhem down
```

## Provider Walkthrough

Install Mayhem and start the provider stack:

```bash
./install.sh --from-source
mayhem up --provider --yes
```

`mayhem up --provider` starts the same local user stack and also supervises provider serving for the first feasible admin-created enclave. Providers do not create models, prices, or canonical rooms; they only opt into admin-created capacity.

Use the printed provider dashboard URL to watch enclave download, verify, seal, load, and serving progress.

List canonical models and provider state:

```bash
mayhem provider list
mayhem provider health
```

What this does:

| Command | Purpose |
|---------|---------|
| `mayhem up --provider --yes` | Starts the peer, bridge, gateway, and provider worker in one supervised local stack. |
| `mayhem provider list` | Shows this wallet's canonical admin-created enclave and room joins. |
| `mayhem provider health` | Checks ledger serving state, local heartbeats, and gateway route visibility. |
| `mayhem provider min-ask set <enclave-or-model:T1> <mu>` | Sets this provider's local floor for an admin-created market. |
| `mayhem provider limits set --max-concurrent <n> --accept-rate <n/min> --budget <mu|tokens>/<epoch|day>` | Sets local self-protection limits. |
| `mayhem provider drain` | Stops accepting new sessions while finishing in-flight sessions. |
| `mayhem provider earnings` | Shows this provider's earnings, holdback, and claimable balances. |

Inspect earnings and reputation:

```bash
mayhem earnings
mayhem reputation
```

Stop the provider stack:

```bash
mayhem down
```

Providers are paid from settlement evidence on the rail they accepted for the served session. Provider min-ask, limits, and drain are local opt-in controls; they do not create canonical models, rooms, or prices.

## Advanced / Manual Provider Start

The main path is `mayhem up --provider`. For debugging a provider worker without the supervisor, use the manual command after a local peer and bridge are already running:

```bash
mayhem provider start --enclave <admin-enclave-id> --rooms auto --serve-sessions
```

| Command | Purpose |
|---------|---------|
| `mayhem models` | Reads the ledger `catalog/current` anchor, fetches signed admin catalog JSON, verifies it, and lists approved catalog content without requiring a repo update. |
| `provider start` | Creates provider opt-in evidence for an existing admin enclave/room and starts serving direct sessions. |
| `--rooms auto` | Selects matching admin-created canonical rooms. Provider-created Intercom rooms are not canonical ledger rooms. |

## Catalog And Enclaves

The public source repo is Mayhem as a whole, not only `intercom/`. The `intercom/` subtree is the embedded Pear/Intercom contract and P2P runtime that Mayhem uses for canonical ledger state and transport.

The model catalog is admin-canonical. Users and providers should discover current catalog content through `mayhem models`, which reads the ledger anchor and verifies the signed catalog release. The local `catalog/` directory is a source and release input, not the only discovery path.

Enclave model bundles are separate release artifacts. They are not committed as repo blobs; production catalog entries point at admin-approved downloads with hashes, sizes, signatures, and provenance.

## What We Actually Sell At Launch

This is the launch sellable surface. Dev catalog entries may exist for smoke work, but users and providers should not treat them as launch products.

<!-- MAYHEM-LAUNCH-SURFACE:START -->
| Model ID | Class | Routes | Artifacts / engines | Verified path | Launch attestation |
|----------|-------|--------|---------------------|---------------|--------------------|
| `qwen/qwen2.5-1.5b-instruct@small` | Text chat, JSON, tools | `/v1/chat/completions`, `/v1/completions` | `gguf-q4_k_m` / llama.cpp; `nvfp4` / trt-llm; `vllm-fp16` / vllm | I3-E11/I3-E14 real GGUF, TensorRT-LLM, and vLLM chat/tool paths | Tier 1 launch |
| `meta/llama-3.1-8b-instruct@4bit` | Text chat, JSON, tools | `/v1/chat/completions`, `/v1/completions` | `gguf-q4_k_m` / llama.cpp; `mlx-4bit` / mlx | I3-E10 catalog/backend compatibility and launch source checks | Tier 1 launch |
| `google/gemma-3-12b-it@4bit` | Text chat, JSON, tools | `/v1/chat/completions`, `/v1/completions` | `gguf-q4_k_m` / llama.cpp; `mlx-4bit` / mlx | I3-E10 catalog/backend compatibility and launch source checks | Tier 1 launch |
| `deepseek/deepseek-r1-distill-qwen-14b@4bit` | Text chat, JSON, tools | `/v1/chat/completions`, `/v1/completions` | `gguf-q4_k_m` / llama.cpp | I3-E10 catalog/backend compatibility and launch source checks | Tier 1 launch |
| `baai/bge-small-en-v1.5@gguf-q8_0` | Embedding | `/v1/embeddings` | `gguf-q8_0` / llama.cpp | I3-E6 real embedding path | Tier 1 launch |
| `huggingfacetb/smolvlm2-256m-video-instruct@gguf-q8_0` | Vision chat | `/v1/chat/completions`, `/v1/completions` | `gguf-q8_0` / llama.cpp | I3-E7/E12/E13 real vision chat path | Tier 1 launch |
| `concedo/sdxs-512-tinysd-distilled@gguf-q8_0` | Image generation | `/v1/images/generations` | `gguf-q8_0` / stable-diffusion.cpp | I3-E8 real image-generation path | Tier 1 launch |
| `openai/whisper-tiny-en@ggml` | Speech to text | `/v1/audio/transcriptions` | `ggml-tiny-en` / whisper.cpp | I3-E9 real STT path | Tier 1 launch |
| `rhasspy/piper-en-us-lessac-low@onnx` | Text to speech | `/v1/audio/speech` | `onnx-lessac-low` / piper | I3-E9 real TTS path | Tier 1 launch |
<!-- MAYHEM-LAUNCH-SURFACE:END -->

Higher trust tiers are not sold at launch until the hardware quote task proves them on real hardware. Tool support is route-specific: llama.cpp and vLLM routes can serve tool/JSON paths where listed; MLX and TensorRT-LLM routes de-advertise tools until their real constrained-decoding paths exist.

## Model Classes And Routes

Catalog entries carry a `model_class` and admin-defined `rate_map`. Text models price token units, embeddings price embedding/input units, image models price image/step units, and audio routes price their own metered dimensions. The gateway exposes the matching OpenAI-compatible route families:

| Class | Routes |
|-------|--------|
| Text generation | `/v1/chat/completions`, `/v1/completions`, including tools, JSON mode, streaming, and vision input when the catalog says the enclave supports it. |
| Embedding | `/v1/embeddings` |
| Image generation | `/v1/images/generations` |
| Audio | `/v1/audio/speech`, `/v1/audio/transcriptions` |

The contract and gateway settle usage through the generic metered map instead of assuming every model is prompt/completion tokens.

## Payments And Receipts

All prices are denominated in `mu_usd`, integer micro-USD. The canonical ledger rails are exactly `fiat`, `tap`, and `tnk`; each rail has separate user balances, provider earnings, and operator-fee buckets. Money never crosses rails during settlement. `mayhem price show <model-id>` prints the live route-level market price and its published derivation when available.

| Rail | Use |
|------|-----|
| `fiat` | Fiat checkout rail. Stripe is the current processor for this rail and credits `bal/<user>/fiat`. |
| `tap` | TAP crypto rail. Users deposit TAP and providers settle TAP earnings from `earn/tap/<provider>`. |
| `tnk` | TNK crypto rail. Users deposit TNK and providers settle TNK earnings from `earn/tnk/<provider>`. |

Providers choose which admin-supported rails they accept; they do not set prices, submit models, or create canonical rooms. Each served session produces signed receipt evidence bound to one rail. The hot path avoids per-token ledger writes; receipts roll into epoch settlement roots.

## Dashboards

`mayhem up` serves local dashboards on loopback by default:

| Dashboard | Path | Shows |
|-----------|------|-------|
| User | `/mayhem/dashboard` | Balance, sessions, spend history, local gateway status, and model catalog. |
| Provider | `/mayhem/dashboard/provider` | Enclave status, live sessions, earnings, reputation/holdback, and claim commands. |

The default server bind is `127.0.0.1`. A user gateway can be shared on a LAN with `mayhem use --bind <addr:port>` only when hashed bearer tokens are configured; non-loopback startup prints copy/paste URLs plus a plain HTTP/TLS-or-VPN notice. Dashboard pages still use their short-lived dashboard session token, and assets are served locally.

## Reference

The full knob inventory is generated from the current code, not maintained by hand:

| Reference | Covers |
|-----------|--------|
| [docs/reference/knob-inventory.md](docs/reference/knob-inventory.md) | Every public CLI help page and flag from the local binaries, plus source-scanned environment variables, TOML config keys, Mayhem HTTP headers, and operational defaults. |
| [docs/operator-runbook.md](docs/operator-runbook.md) | Admin/operator procedures, key handling, settlement, catalog publication, and production rehearsals. |
| [docs/provider-guide.md](docs/provider-guide.md) | Provider serving workflow and operational checks. |
| [docs/user-guide.md](docs/user-guide.md) | User gateway, payments, balances, and client setup. |

Regenerate and verify the inventory after changing knobs:

```bash
cargo build -p mayhem-cli -p mayhemd -p mayhem-enclave -p mayhem-gateway -p mayhem-paygate
node scripts/knob-inventory.mjs --write
node scripts/knob-inventory.mjs --check
```

## Updates And Versioning

`mayhem update` stages release artifacts only after verifying the signed release manifest, SHA-256s, and release signing key. Applying a staged update has a delay window, health check, and rollback path.

Contract-changing releases advertise `CONTRACT_VERSION`. Out-of-sync nodes receive an explicit `UPGRADE_REQUIRED` signal instead of drifting into invalid-signature failures, and receipt/signing schema migrations run through declared version hooks.

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

Standalone `mayhem-gateway` is a raw development smoke binary and must be started with `--dev-embedded-catalog`. Production/user/provider flows should use `mayhem up` and `mayhem down`.

The iteration plans and trackers in `docs/` record the implementation history.
