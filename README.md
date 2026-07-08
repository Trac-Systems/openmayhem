# OpenMayhem

**Sell inference from any machine — a gaming PC or a confidential-compute rack. Buy inference at a price no company sets.**

OpenMayhem is a peer-to-peer AI inference marketplace. Providers plug in machines and earn on every token they serve. That can be a gaming PC that sits idle overnight, a Mac, a homelab box, or a rack of confidential-compute H100s in a datacenter. Casual and professional operators sell in the same market, and the trust tiers price the difference between them. Users point any OpenAI-compatible client at `127.0.0.1` and buy at whatever the market currently charges, paying by card or on-chain. There is no cloud in the middle. Requests travel over encrypted peer-to-peer sessions, and a public ledger records prices, receipts, and settlements, so anyone can check what actually happened.

The CLI binary is `mayhem`. The network runs on [Trac Network](https://www.tracsystems.io/trac-network), which carries the peer-to-peer transport and the replicated contract that every node verifies identically.

Community: **[r/Open_Mayhem](https://www.reddit.com/r/Open_Mayhem/)** — setup help, provider earnings talk, announcements.

```text
your OpenAI client ──▶ local gateway (127.0.0.1) ──▶ encrypted P2P session ──▶ a provider's machine
                                     │                                                │
                                     └──────────── signed receipts ◀──────────────────┘
                                                        │
                                          epoch settlement on the Trac ledger
                                          (prices, earnings, evidence — public)
```

**Why people run it:**

- **A real market for tokens.** No central pricing. Every model at every trust tier is its own market. Prices float on supply and demand each epoch, and each epoch's price is published together with the inputs that produced it, so you can recompute it yourself.
- **One command on any machine.** `mayhem up --provider` probes your hardware, picks an engine, fetches a model that fits, and starts earning. Works the same on a Windows gaming PC that only serves evenings and on a Linux fleet serving ten models around the clock.
- **Room for every kind of provider.** Casuals set a daily budget and forget about it. Professionals run multi-model fleets, verify their business identity (Tier 4), or bring confidential-compute hardware and sell Tier 3, the only tier where prompts stay private. Higher trust clears at higher prices.
- **Built for agentic work.** Standard OpenAI-compatible routes — chat, tools, JSON mode, streaming, vision, embeddings, image generation, speech in and out. The 8B–14B instruct models that agent loops actually run on serve from consumer GPUs and Macs, with tool calling verified per route.
- **Pay how you already pay.** Card via Stripe by default. If you'd rather pay on-chain, TAP (Ethereum) and TNK (Trac) work too; OpenRouter gives its users the same choice, and it's supported here to a similar extent. A payment option, not a crypto project. Everything is priced in dollars and network gas is sponsored, so you never need to hold a token to use or provide.
- **Four trust tiers.** From community hardware up to confidential compute. Each tier is its own market with its own price, and the tier table below says exactly what each one does and doesn't protect.
- **Evidence over trust.** Receipts are signed, work settles in public epochs, prices carry their derivation. The ledger is open and it is the source of truth.

---

## Quickstart

**Get the code first.** Either clone the repository and build from source:

```bash
git clone https://github.com/Trac-Systems/openmayhem.git
cd openmayhem
```

Or skip the checkout and install the latest release artifact, verified against its SHA-256 sidecar (both values are on the [releases page](https://github.com/Trac-Systems/openmayhem/releases/latest)):

```bash
./install.sh --artifact-url <archive-url> --sha256 <archive-sha256>
```

Every command below assumes you are in the repository directory (or have `mayhem` on your `PATH` after installing).

**Use the network** (buy inference):

```bash
./install.sh --from-source        # Windows: .\install.ps1 -FromSource
mayhem up --yes
curl http://127.0.0.1:11435/v1/models
```

Point any OpenAI client at `http://127.0.0.1:11435/v1` and go.

The installer ships a checksum-pinned [opencode](https://opencode.ai) coding agent (skip with `--skip-opencode` if you have your own). One command wires it to the gateway — it registers a `mayhem` provider in `~/.config/opencode/opencode.json` and fills its model list live from `/v1/models`, leaving any other providers you have configured untouched:

```bash
mayhem opencode                   # wire (or repair) opencode for the local gateway
opencode run --model mayhem/<model-id> "Say hello from OpenMayhem."
```

When the catalog changes, `mayhem opencode` re-syncs the model list.

**Provide to the network** (earn on your hardware):

```bash
./install.sh --from-source
mayhem up --provider --yes
mayhem provider health
```

`mayhem up` starts everything supervised (peer, bridge, gateway, and the serving worker with `--provider`), health-checks each part, and prints endpoint and dashboard URLs you can copy. No browser required. `mayhem down` stops everything.

> [!TIP]
> **Let an AI assistant install it for you.** If you don't want to touch a terminal alone, open this repository in a coding agent (Claude Code, Codex, Cursor, opencode) and paste one of the prompts below. The agent reads this README and drives the real install with you.

**Agent prompt — user setup:**

```text
Read the README of this repository. I want to USE OpenMayhem to run AI models
(not provide compute). Install it for my operating system, run `mayhem up`,
verify the gateway answers on http://127.0.0.1:11435/v1/models, then help me
buy my first credits with `mayhem pay stripe` and run one test chat completion
against a model from `mayhem models --gateway`. Explain each command before
you run it and show me the dashboard URL at the end.
```

**Agent prompt — provider setup:**

```text
Read the README of this repository. I want to PROVIDE compute to OpenMayhem
and earn with this machine. Check my hardware first and tell me which models
fit. Install the software, run `mayhem up --provider`, help me choose which
payment rails to accept with `mayhem provider rails set`, set sensible
self-protection limits with `mayhem provider limits set`, and confirm I am
serving with `mayhem provider health`. Explain what my expected earnings
depend on, and show me the provider dashboard URL.
```

---

## What runs where

OpenMayhem serves from anything with compute, consumer or datacenter. The engine is chosen automatically per artifact; you never pick a backend by hand.

| Hardware | On macOS | On Linux | On Windows |
|---|---|---|---|
| **NVIDIA GPU** (consumer, CUDA 12+) | — | llama.cpp (CUDA), vLLM, TensorRT-LLM | llama.cpp (CUDA), vLLM |
| **NVIDIA datacenter GPU** (H100/H200 class) | — | llama.cpp (CUDA), vLLM, TensorRT-LLM; with SEV-SNP + GPU CC mode: attests **Tier 3** | — |
| **Apple Silicon** (M1–M5, unified memory) | llama.cpp (Metal), MLX | — | — |
| **NVIDIA GB10 / DGX Spark class** (unified memory) | — | TensorRT-LLM (NVFP4), vLLM | — |
| **AMD GPU** (ROCm / Vulkan) | — | llama.cpp | llama.cpp |
| **CPU only** (AVX2 x86_64, NEON arm64) | llama.cpp | llama.cpp | llama.cpp |

No GPU? CPU-only machines still serve. Embeddings, small text models, and speech in/out are realistic on a plain CPU. Linux with an NVIDIA card is the fullest-featured platform, since all three GPU engines run there. A confidential-compute host (an AMD SEV-SNP machine with an H100-class GPU in CC mode, rented or on-prem) is just a Linux provider that can additionally prove Tier 3.

**Dependencies per OS** — the installer handles these, listed here so you know what lands on your machine:

| OS | Needed | Notes |
|---|---|---|
| macOS | Xcode Command Line Tools, Rust toolchain, Node.js | `install.sh` checks and prompts; Metal ships with the OS |
| Linux | `build-essential`/`gcc`, Rust toolchain, Node.js; NVIDIA driver + CUDA 12 for GPU serving | Python 3.10+ only if a vLLM/TensorRT artifact is selected |
| Windows | Visual Studio Build Tools (MSVC), Rust toolchain, Node.js; NVIDIA driver | `install.ps1`; the engine runs sandboxed (AppContainer) |

Before any download, the model list shows what actually fits on this machine: which models run, roughly how fast, at what context size, and how big the download is. Capacity math uses your GPU's dedicated memory (on Apple Silicon and GB10-class machines, the whole unified pool minus an OS reserve), and a model that only partially fits gets a CPU/GPU split computed for your card.

---

## Using the network

Why buy inference here instead of a retail API? The price, mostly. Retail inference carries a company's margin; here the price is whatever the market clears at, and the market runs on hardware whose owners have already paid for it. The models that matter for agentic work today — capable 8B–14B instruct models with tool calling, JSON mode, and streaming — serve well from a single consumer GPU or an Apple Silicon Mac, which is exactly the hardware this network is full of. You pay per token from a prepaid balance. No subscription, no monthly minimum, no account with an AI company — the first `mayhem up` is your identity, and your balance works across every model on the network.

What people actually use it for:

| You are | What OpenMayhem gives you |
|---|---|
| **A developer** | A drop-in OpenAI-compatible endpoint on `127.0.0.1`. Point your existing SDK, agent framework, or app at it and nothing else changes. One balance covers chat, embeddings, vision, images, and speech. |
| **An agent builder** | The engine room for agentic loops. Small-to-mid instruct models are what agents run on now — tool calls, JSON mode, multi-step loops, thousands of calls a day — and that's precisely the class a market of consumer GPUs serves at prices retail can't touch. Per-request headers let every call in the loop pick its own price ceiling, context floor, or trust tier; embeddings and speech ride on the same balance. |
| **A team or household** | One funded gateway, shared. Everyone gets their own token with its own budget and rate limit, and you see who spent what. |
| **Privacy-sensitive work** | `--min-att-tier 3` routes only to confidential-compute providers, where your prompts are cryptographically unreadable to the machine's operator. It's a hard filter, never downgraded. For identity instead of privacy, `--require-kyb` routes only to verified businesses. |
| **Someone who distrusts pricing pages** | Every price on this network is published with the formula and inputs that produced it. Recompute it yourself; you'll get the same number. |

### Start, look around

```bash
mayhem up --yes                   # start the local stack, print URLs
mayhem models --gateway           # models with live routes right now
mayhem status                     # component health, ports, balances
mayhem price show <model-id> --tier 1   # live market price + its derivation
```

### Pay — card first, one price everywhere

Everything you see is in dollars. Internally the ledger counts in atto-USD (`au`, 10⁻¹⁸ dollars), which is why a $0.01-per-million-token embedding model still has an exact integer price per single token and the market can move prices in tiny steps. You'll never handle raw `au`; the CLI and dashboards show dollars. The price is the same dollar figure on every rail, and rails never mix during settlement.

**Card via Stripe (the default — no wallet, no token, nothing to learn):**

```bash
mayhem pay stripe --amount 10.00
```

```text
Stripe checkout created. Opening your browser (URL also printed for copy/paste):

  https://checkout.stripe.com/c/pay/cs_live_a1B2c3...

Waiting for payment confirmation...
```

Pay in the browser like any online purchase. Stripe notifies the network, the signature-verified webhook credits your balance:

```bash
mayhem deposit status
mayhem balance
```

```text
fiat: 10.000000 USD    tap: 0.000000 USD    tnk: 0.000000 USD
```

For most people, that is the whole payments story.

<details>
<summary><b>On-chain rails (optional): TAP and TNK</b></summary>

If you prefer paying on-chain (OpenRouter offers the same option), two rails exist: **TAP**, an ERC-20 on Ethereum, and **TNK**, Trac's native token. Both work the same way: top up your in-app address, then deposit. The app signs everything locally. You never leave the CLI, never paste calldata anywhere, and no admin key is involved.

Your addresses were created on your first `mayhem up`: one encrypted keypair holding a Trac address and an Ethereum address, both derived from the same seed. Back it up once. The mnemonic restores both addresses, and any earnings bound to them, on any machine:

```bash
mayhem wallet show        # your Trac (TNK) and Ethereum (TAP) addresses
mayhem wallet backup      # reveal the mnemonic after explicit confirmation
mayhem wallet import      # bring an existing Trac and/or Ethereum key instead
mayhem wallet passwd      # re-encrypt with a new password
```

**TAP:** send TAP to your in-app Ethereum address, then deposit. The app checks balances and gas, simulates the transaction, and only then signs approve+deposit over an Ethereum RPC (`--rpc-url` overrides the public default):

```bash
mayhem deposit tap --amount 10.00            # dry-run: balances, gas check, simulation
mayhem deposit tap --amount 10.00 --confirm  # sign + broadcast
```

**TNK:** send TNK to your in-app Trac address, then deposit. Signed locally, confirmed through the local MSB that ships with the node. No external RPC at all:

```bash
mayhem deposit tnk --amount 10.00
```

Either way, `mayhem deposit status` and `mayhem balance` show the credit land. The ETH that comes along when you top up an Ethereum address covers your own deposit gas, and the network's settlement rollups are sponsored by the operator. Use the on-chain rails if you want them; the card rail is there so nobody has to.

</details>

### Control what you pay and what serves you

```bash
mayhem config max-price 0.25              # never pay above $0.25 per priced unit (persistent ceiling)
mayhem config set --key min-ctx --value 128000   # only route to providers with ≥128k context
mayhem models --gateway --min-att-tier 3  # only confidential-compute routes
mayhem models --gateway --require-kyb     # only identity-verified businesses
mayhem models --gateway --quant int4      # filter by quantization
```

Per-request, any OpenAI client can override with headers — no SDK changes:

| Header | Effect |
|---|---|
| `X-Mayhem-Max-Price-Au` | price ceiling for this request |
| `X-Mayhem-Min-Att-Tier` | minimum trust tier (hard filter, never downgraded) |
| `X-Mayhem-Min-Ctx` | minimum context window |
| `X-Mayhem-Quant` | required quantization bucket |
| `X-Mayhem-Hedge` | race a second provider for latency |
| `X-Mayhem-Min-Tok-S` | throughput floor |

### Share one funded gateway with your team or your other machines

```bash
mayhem tokens create --name laptop --budget 10/day --max-rate 60
mayhem up --gateway-bind 0.0.0.0:11435
```

`tokens create` prints `sk-mayhem-...` exactly once and stores only a hash. Every machine, agent, or teammate gets its own token with its own budget, rate limit, and optional model allowlist. Spend attribution shows who used what, and everything still settles from your one balance. A non-loopback bind refuses to start until at least one token exists; loopback stays token-free unless you add `--gateway-require-auth`. The gateway speaks plain HTTP, so put a TLS reverse proxy or Tailscale in front of anything that leaves your LAN.

```bash
mayhem tokens list                # names, masked prefixes, expiry, spend
mayhem tokens revoke laptop      # immediate
```

### User CLI at a glance

| Command | What it does |
|---|---|
| `mayhem up` / `mayhem down` | start/stop the supervised stack; prints endpoint + dashboard URLs |
| `mayhem status` | live component state, ports, sync, balances |
| `mayhem models --gateway` | models with live routes, capabilities, tiers, quant |
| `mayhem opencode` | wire the bundled opencode agent to the gateway; re-run to re-sync models |
| `mayhem price show <model> [--tier]` | current market price with published derivation |
| `mayhem pay stripe` / `mayhem deposit tap\|tnk` | buy credits on your chosen rail (card is the default) |
| `mayhem deposit status` | pending/confirmed deposits from the ledger |
| `mayhem balance` | per-rail balances |
| `mayhem config max-price / set` | persistent spending and routing defaults |
| `mayhem tokens create/list/revoke` | bearer tokens for shared gateways |
| `mayhem wallet show/backup/import/passwd` | your key, your custody |
| `mayhem update` | signed, verified, rollback-safe self-update |

---

## Providing to the network

The same software and the same market serve four fairly different kinds of operation. Pick your lane; the knobs match it:

| You are | Typical setup | How you run it |
|---|---|---|
| **Casual** | The gaming PC or Mac you already own, serving when you feel like it | `mayhem up --provider`, set a daily `--budget` and an accept-rate cap, walk away. Refusals from your own limits never hurt your reputation, and `mayhem provider drain` signs you off cleanly. |
| **Enthusiast** | A dedicated homelab box or a 24 GB+ GPU running around the clock | Serve a bigger model, or several at once. One command packs multiple models into your memory budget and serves them concurrently, each in its own market. |
| **Professional** | Multiple machines, business identity, uptime discipline | Run a fleet of provider identities, verify your business (Tier 4 KYB) so users who filter `--require-kyb` route to you, and take the identity premium your market clears at. |
| **Confidential operator** | AMD SEV-SNP hosts with H100-class GPUs in CC mode, cloud-rented or on-prem | Attest Tier 3 and sell the one tier where user prompts are unreadable even to you. CC hardware is scarce, so these markets clear at their own, higher price. |

All four settle the same way, on the same evidence, at the market price of whatever tier they can prove.

### Start serving

```bash
mayhem up --provider --yes
```

That's the whole happy path. The software probes your hardware, shows which admin-approved models fit (with estimated speed and context, before anything downloads), fetches and verifies the model, and starts serving. The provider dashboard shows download, verify, seal, load, and serving progress live.

Model downloads come from Hugging Face. A free Hugging Face token raises your rate limits and makes multi-gigabyte pulls faster and more reliable. Worth setting up once:

```bash
export HF_TOKEN=hf_...                    # or:
mayhem provider start --hf-token-file ~/.mayhem/hf.txt
```

```bash
mayhem provider list              # your enclave and room joins
mayhem provider health            # ledger state, heartbeats, route visibility
```

**Bigger machine? Serve several models at once.** One command serves N models from one box. The packer fits them into your measured memory budget and refuses combinations that don't fit, rather than degrading anything silently. Each model joins its own market at its own price, and per-enclave limits cap each one independently. You can leave one market and pick up another without touching the rest:

```bash
mayhem provider drain --enclave <enclave-id>     # leave one market cleanly
mayhem up --provider                              # re-pack with the new selection
```

### Choose how you get paid

Pick which rails you accept — you will only be matched with users paying on rails you accept:

```bash
mayhem provider rails set --rails fiat,tap,tnk    # accept everything (recommended)
mayhem provider rails get
```

Earnings settle **automatically at the end of every epoch (hourly)** on each rail you earned in. You keep 85%; the network fee funds gas sponsorship and operations.

Two rails push the money to you. One is a pull, for a reason:

| Rail you accepted | How you receive it | Push or pull |
|---|---|---|
| `fiat` | Stripe pays your bank account every epoch. Onboarding happens once: the CLI prints a Stripe Connect link, you finish it in the browser, and payouts flow on their own from then on. Until you onboard, earnings simply accrue, and `mayhem provider earnings` tells you what's left to do. | Push, automatic |
| `tnk` | The epoch settlement sends real TNK transfers straight to your Trac address every epoch, fees sponsored. Nothing to run, nothing to sign. | Push, automatic |
| `tap` | The epoch settlement publishes a settlement root on Ethereum and your earnings accumulate under it. `mayhem claim` runs a Merkle claim that transfers everything owed to your Ethereum address in one transaction. | Pull, you claim |

TAP is a pull because paying every provider on Ethereum every hour would burn gas per provider per epoch. The settlement root is cumulative instead: unclaimed earnings pile up losslessly, and you claim when it's worth a transaction — after a day, a month, whenever. Nothing expires, claiming late costs nothing, and one claim sweeps everything since your last one. The claim is signed by your in-app Ethereum key, with gas from the ETH in your own wallet.

```bash
mayhem provider earnings          # earnings, holdback, claimable per rail
mayhem earnings                   # same, short form
mayhem reputation                 # your standing, event history
mayhem claim                      # TAP only: sweep accumulated earnings to your address
```

### Set your terms

You don't set the market price; the market does. What you set is your own floor and your own protection:

```bash
mayhem provider min-ask set <model:T1> 120000     # serve only when the market clears above this
mayhem provider limits set --max-concurrent 4 --accept-rate 30/min --budget 5000000/day
mayhem provider limits set --enclave <model-or-enclave> --max-concurrent 1 --budget 1000000/day
mayhem provider drain             # finish in-flight work, accept nothing new, sign off clean
mayhem provider drain --enclave <enclave-id>
```

`limits set` is the casual-provider safety kit: cap concurrent sessions, cap the accept rate, and cap total spend served per day (`--budget`), so your electricity bill stays bounded without babysitting the box. Add `--enclave` to scope limits to one served model; memory and disk reserves stay machine-level. Refusals from these limits are clean protocol events and never damage your reputation. Reputation tracks one thing, delivering what you advertised — a slow machine that advertises 15 tok/s and hits it scores perfectly.

```bash
mayhem provider min-ask get <model:T1>
mayhem provider rails get
```

### Provider CLI at a glance

| Command | What it does |
|---|---|
| `mayhem up --provider` | start serving with everything supervised |
| `mayhem provider list / health` | joins, ledger state, heartbeats, visibility |
| `mayhem provider rails set/get` | which payment rails you accept |
| `mayhem provider min-ask set/get` | your price floor per market |
| `mayhem provider limits set` | concurrency / accept-rate / daily budget caps |
| `mayhem provider drain` | graceful sign-off |
| `mayhem provider earnings` | earnings, holdback, claimables per rail |
| `mayhem reputation` | your standing and why |
| `mayhem claim` | execute TAP Merkle claims |

---

## The market

Every model × tier is an independent market. The admin seeds a starting price once; after that the price floats on measured utilization, epoch by epoch. Providers post min-asks, users post max-bids, and a session locks its price at the moment it opens. The number you agreed to is the number you settle at.

Prices are quoted separately for input and output tokens, in dollars per million. Embeddings bill per input token. Image generation bills per image and per step, scaled by resolution. Audio has its own metered units.

Every epoch's price is published with its derivation: the seed, the measured utilization, the public constants, and the settled work behind them. Run the formula yourself and you get the same number. `mayhem price show` prints it, the dashboards render it, and every model gets a financial-style price chart (candles and volume, built from real epoch prices) on the user and provider dashboards.

Context is part of the deal too. Providers advertise the context window they serve; the network verifies it with targeted probes and it's guaranteed for the duration of your session. Larger context brackets clear at their own prices, and a `min-ctx` filter routes you only to providers with the headroom you need. When every provider is busy, an opt-in `--max-wait` holds your request for the next free slot instead of failing it.

## The four trust tiers

Attestation tiers describe what trust evidence a provider has. They don't all mean the same thing, and a higher number isn't simply "everything below it plus more."

| Tier | Plain meaning | Can the provider read my prompt? |
|------|---------------|----------------------------------|
| Tier 1 | Runs the OpenMayhem software. Trust is mostly economic: if they cheat, probes, receipts, holdbacks, and slashing cost them money. | Yes. |
| Tier 2 | Proven genuine, unique Apple or NVIDIA hardware that attested the real app. Stops fake hardware and makes bans stick to the device — the served model is verified the same way as Tier 1, by network spot checks. | Yes. |
| Tier 3 | Hardware confidential compute — an AMD SEV-SNP confidential VM with an NVIDIA GPU in CC mode. Your prompt is protected even from the provider's own machine, and the served model is the pinned model by construction, not by spot check. | No. |
| Tier 4 | A real, identity-verified business (KYB). You know who they are. | Yes — Tier 4 is identity, not prompt privacy. |

The short version: **only Tier 3 means the provider cannot read your prompt.** Same model, higher tier, higher price; each tier is its own market and the gap prices itself. A `--min-att-tier` request is a hard filter and is never silently downgraded.

The network also spot-checks providers continuously with canary probes. Model identity, output quality, and advertised context get verified against what's actually served, and the results feed reputation, routing, and slashing.

## Available Launch Models

The model catalog is signed and canonical: `mayhem models` reads the ledger anchor and verifies the signed catalog release, so you always discover current models without requiring a repo update. Providers opt into canonical enclaves the network operator creates — they do not set prices, create canonical rooms, or submit arbitrary models — which is what keeps every listed model a verified, hash-pinned artifact instead of a claim.

This is the launch sellable surface. Dev catalog entries may exist for smoke work; don't treat them as launch products.

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

Launch proves every code path with small models across every modality and engine; the same paths scale to larger models as the network grows. Higher trust tiers go on sale as their attestation is proven on real hardware, and Tier 3's already is: the full confidential-compute proof chain (SEV-SNP platform attestation to AMD's roots, GPU CC attestation to NVIDIA's) has run verified on a real H100 confidential VM. Tier-3 enclaves list as confidential-compute providers join. Tool support is route-specific: llama.cpp and vLLM routes serve tool/JSON paths where listed, while MLX and TensorRT-LLM routes de-advertise tools until their real constrained-decoding paths exist.

**Routes:**

| Class | Routes |
|-------|--------|
| Text generation | `/v1/chat/completions`, `/v1/completions` — tools, JSON mode, streaming, vision input where the catalog says so |
| Embedding | `/v1/embeddings` |
| Image generation | `/v1/images/generations` |
| Audio | `/v1/audio/speech`, `/v1/audio/transcriptions` |

## Dashboards

`mayhem up` serves local dashboards on loopback:

| Dashboard | Path | Shows |
|-----------|------|-------|
| User | `/mayhem/dashboard` | balance, sessions, spend history, gateway status, catalog |
| Provider | `/mayhem/dashboard/provider` | enclave status, live sessions, earnings, reputation, holdback |
| Network explorer | `/mayhem/dashboard/network` | every model and provider: abilities, tiers, rails, live prices with derivations, availability |

All figures come from live contract and heartbeat state. Nothing is made up; a model with no live provider shows as unavailable.

## How the money is secured

- **Session price lock.** Every session freezes its rate at open, and settlement validates against the locked rate forever. Market moves never touch work already agreed.
- **Signed receipts, epoch settlement.** Providers sign for the work they deliver, receipts roll into per-epoch settlement roots on the ledger, and the contract enforces that debits equal earnings, per rail, every epoch.
- **Balance-backed authorization.** Spending is bounded by real on-ledger balance, enforced on the provider side where a modified client can't reach it.
- **Challengeable everything.** Over-credited commits and fabricated prices can be fraud-proven by anyone holding the evidence. Bad commits are voided and their submitters penalized, and a running challenger watches every window.
- **Deposits are oracle evidence.** Credits come from verified payment events — Stripe webhooks are signature-checked and replay-deduplicated, chain deposits are watched and confirmed — never from self-asserted writes.

## Install

From a source checkout:

```bash
./install.sh --from-source        # macOS / Linux
.\install.ps1 -FromSource         # Windows PowerShell
```

From release artifacts (verified against a SHA-256 sidecar):

```bash
./install.sh --artifact-url <archive-url> --sha256 <archive-sha256>
```

Installers print a copy/paste `PATH` command even when they update your shell profile, and anything that would open a browser prints the URL first. The whole system works from a terminal alone.

`mayhem update` keeps you current. It stages releases only after verifying the signed manifest, hashes, and signing key, applies with a delay window and a health check, and rolls back if the update misbehaves. Contract-changing releases version-gate explicitly: out-of-date nodes get a clear `UPGRADE_REQUIRED` instead of silent divergence.

## Development

```bash
scripts/dev-net.sh --cleanup
cargo build --workspace
MAYHEM_RUN_INTERCOM_TESTS=1 cargo test -p mayhem-bridge --test sc_bridge -- --nocapture
```

The `intercom/` subtree is the embedded Trac/Intercom contract and P2P runtime. Standalone `mayhem-gateway` is a development smoke binary (`--dev-embedded-catalog` required); real flows go through `mayhem up` / `mayhem down`.

The full knob inventory (every CLI flag, env var, config key, HTTP header, and default) is generated from the code:

```bash
node scripts/knob-inventory.mjs --write && node scripts/knob-inventory.mjs --check
```

## License

MIT — see [LICENSE](LICENSE). Copyright © 2026 Trac Systems UG (haftungsbeschränkt). The code is yours to use, fork, and build on. The "OpenMayhem" and "Trac" names and logos are trademarks of Trac Systems UG and are not part of the code license.

---

**The machines are already bought. The models are already open. OpenMayhem is the market that connects them.**
