# Mayhem

Mayhem lets you use admin-approved AI enclaves through a local OpenAI-compatible endpoint while providers supply compute over Trac Intercom. Users run one local gateway, point tools such as opencode at it, and receive signed receipts for the work. Providers opt into canonical enclaves and rooms that the Mayhem admin created; they do not set prices, create canonical rooms, or submit arbitrary models.

The contract is the public evidence ledger. It records canonical catalog anchors, admin-created enclaves and rooms, provider opt-ins, prices, rules, balances, receipts, disputes, and settlement roots. The actual prompts and model responses travel over direct provider sessions, not through the ledger.

Deposit credits are admin-oracle evidence from watchers/paygate, not arbitrary
provider or user writes. The trust boundary and key separation are documented in
[docs/design/deposit-trust-boundary.md](docs/design/deposit-trust-boundary.md).

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

For sensitive prompts, inspect available trust tiers before choosing a route:

```bash
mayhem models --gateway --min-att-tier 3
mayhem models --gateway --require-kyb
```

`--min-att-tier 3` asks for prompt-private routing. `--require-kyb` asks for Tier 4 identity; it does not make prompts private. You can also send routing preferences through OpenAI-compatible request headers, for example `X-Mayhem-Min-Att-Tier: 3`, `X-Mayhem-Hedge: 1`, or failover thresholds such as `X-Mayhem-Min-Tok-S`.

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

Inspect earnings and reputation:

```bash
mayhem earnings
mayhem reputation
```

Stop the provider stack:

```bash
mayhem down
```

Providers are paid from settlement evidence on the rail they accepted for the served session.

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

All prices are denominated in `mu_usd`, integer micro-USD. The canonical ledger rails are exactly `fiat`, `tap`, and `tnk`; each rail has separate user balances, provider earnings, and operator-fee buckets. Money never crosses rails during settlement.

| Rail | Use |
|------|-----|
| `fiat` | Fiat checkout rail. Stripe is the current processor for this rail and credits `bal/<user>/fiat`. |
| `tap` | TAP crypto rail. Users deposit TAP and providers settle TAP earnings from `earn/tap/<provider>`. |
| `tnk` | TNK crypto rail. Users deposit TNK and providers settle TNK earnings from `earn/tnk/<provider>`. |

Providers choose which admin-supported rails they accept; they do not set prices, submit models, or create canonical rooms. Each served session produces signed receipt evidence bound to one rail. The hot path avoids per-token ledger writes; receipts roll into epoch settlement roots.

## Dashboards

`mayhem up` serves loopback-only local dashboards:

| Dashboard | Path | Shows |
|-----------|------|-------|
| User | `/mayhem/dashboard` | Balance, sessions, spend history, local gateway status, and model catalog. |
| Provider | `/mayhem/dashboard/provider` | Enclave status, live sessions, earnings, reputation/holdback, and claim commands. |

The server binds `127.0.0.1` only, uses short-lived local tokens, and serves assets locally.

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

Operator, provider, and user docs live in `docs/operator-runbook.md`, `docs/provider-guide.md`, and `docs/user-guide.md`. The iteration plans and trackers in `docs/` record the implementation history.
