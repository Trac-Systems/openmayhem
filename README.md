# OpenMayhem

**Sell inference from any machine — a gaming PC or a confidential-compute rack. Buy inference at a price no company sets.**

OpenMayhem is a peer-to-peer AI inference marketplace. Providers plug in machines and earn on every token they serve. That can be a gaming PC that sits idle overnight, a Mac, a homelab box, or a rack of confidential-compute H100s in a datacenter. Casual and professional operators sell in the same market, and the trust tiers price the difference between them. Users point any OpenAI-compatible client at `127.0.0.1` and buy at whatever the market currently charges, paying by card or on-chain. There is no cloud in the middle. Requests travel over encrypted peer-to-peer sessions, and a public ledger records prices, receipts, and settlements, so anyone can check what actually happened.

The CLI binary is `mayhem`. The network runs on [Trac Network](https://www.tracsystems.io/trac-network), which carries the peer-to-peer transport and the replicated contract that every node verifies identically.

Telegram: **[t.me/openmayhem](https://t.me/openmayhem)** — join the OpenMayhem group.

Website: **[openmayhem.ai](https://www.openmayhem.ai/)** — the project site.

Community: **[r/Open_Mayhem](https://www.reddit.com/r/Open_Mayhem/)** — setup help, provider earnings talk, announcements.

Models: **[huggingface.co/TracNetwork](https://huggingface.co/TracNetwork)** — the network's Hugging Face account; every served model is mirrored there and the signed catalog is published from it.

Model cheatsheet: **[MODEL-CHEATSHEET.md](MODEL-CHEATSHEET.md)** — exact canonical model IDs, artifacts, endpoints, platform/backend support, runtime pins, resource guidance, controls, and provider commands.

Calibration gates: **[CALIBRATION.md](CALIBRATION.md)** — proof requirements before a model or workflow can be treated as product-ready.

Comfy cheatsheet: **[COMFY-CHEATSHEET.md](COMFY-CHEATSHEET.md)** — workflow classes, signed parts, provider admission, and `/v1/workflows` usage.

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
- **Workflow serving without a separate app.** ComfyUI workflows use the same gateway, catalog, provider admission, vouchers, receipts, and settlement path as model inference. Providers choose verified parts; users submit bounded graphs.
- **Pay how you already pay.** Card via Stripe by default. If you'd rather pay on-chain, TAP (Ethereum) and TNK (Trac) work too; OpenRouter gives its users the same choice, and it's supported here to a similar extent. A payment option, not a crypto project. Everything is priced in dollars and network gas is sponsored, so you never need to hold a token to use or provide.
- **Four trust tiers.** From community hardware up to confidential compute. Each tier is its own market with its own price, and the tier table below says exactly what each one does and doesn't protect.
- **Evidence over trust.** Receipts are signed, work settles in public epochs, prices carry their derivation. The ledger is open and it is the source of truth.

---

## Quickstart

### Quickest for non-techies: use your agent to install & run

No terminal knowledge needed. Hand this repository to a coding agent — Claude Code, Codex, Cursor, or opencode — and paste one of the prompts below. The agent reads this README, checks your operating system and hardware, runs the real install, and verifies the gateway is answering before it hands back. You watch and approve each step; you never have to know a single command. For most people this is the whole install.

> **Agents:** read **[`SKILL.md`](SKILL.md)** first — it is the authoritative, structured guide for installing and operating OpenMayhem (per-OS install + pitfalls, the ask-first flows for users and providers, funding, and the hard rule to never modify the pinned core). It exists so you don't have to figure the app out yourself.

**Agent prompt — user setup (buy inference):**

```text
Read the README of this repository, especially "What you need installed
first" and "Agent install checklist". I want to USE OpenMayhem to run AI
models (not provide compute). Detect my OS, install the missing
prerequisites for it FIRST, then install OpenMayhem, run `mayhem up`,
verify the gateway answers on http://127.0.0.1:11435/v1/models, then help me
buy my first credits with `mayhem pay stripe` and run one test chat completion
against a model from `mayhem models --gateway`. Explain each command before
you run it and show me the dashboard URL at the end.
```

**Agent prompt — provider setup (earn on your hardware):**

```text
Read the README of this repository, especially "What you need installed
first" and "Agent install checklist". I want to PROVIDE compute to OpenMayhem
and earn with this machine. Detect my OS and hardware, install the missing
prerequisites FIRST (including the GPU/backend-specific ones the checklist
names for my hardware), install the software, run `mayhem doctor` and tell
me which models fit, run `mayhem up --provider`, help me choose which
payment rails to accept with `mayhem provider rails set`, bind my own verified
payout destinations with `mayhem provider payout` or `mayhem provider stripe`,
set sensible self-protection limits with `mayhem provider limits set`, and
confirm I am serving with `mayhem provider health`. Explain what my expected
earnings depend on, and show me the provider dashboard URL.
```

No coding agent yet? Any of the ones above installs in a minute, or drive it yourself with the manual steps below.

### Manual install

`v0.2.108` is a source release. GitHub publishes the tagged source archives; it
does not publish unsigned OpenMayhem executables. Clone the exact tag and let
the installer build for the current host.

macOS/Linux:

```bash
git clone https://github.com/Trac-Systems/openmayhem.git
cd openmayhem
git checkout --detach v0.2.108
./install.sh --from-source
```

Windows PowerShell:

```powershell
git clone https://github.com/Trac-Systems/openmayhem.git
Set-Location openmayhem
git checkout --detach v0.2.108
.\install.ps1 -FromSource
```

On an updated source checkout, `mayhem up` verifies the physical Intercom dependency topology
before Pear starts and repairs stale generated nested dependencies from the exact lock when needed.
It does not replace or hydrate `~/.mayhem`, wallets, sparse stores, or provider registrations.

Everything installs under `~/.mayhem/` without requiring a signed Apple or
Windows application bundle.

Every command below assumes you are in the repository directory (or have `mayhem` on your `PATH` after installing).

**Use the network** (buy inference):

```bash
./install.sh --from-source        # Windows: .\install.ps1 -FromSource
mayhem up --rail fiat --yes       # or: --rail tap / --rail tnk
curl http://127.0.0.1:11435/v1/models
```

Point any OpenAI client at `http://127.0.0.1:11435/v1` and go.

`mainnet` is the default and is fail-closed. Installed builds embed the official
Trac MSB and DHT bootstrap set, the canonical Mayhem subnet/admin identity, and
the mainnet TAP, TNK, and Stripe directory identities. `mayhem up` refuses a
conflicting bootstrap or deployment and does not report ready until the signed
rules, payment directory, catalog, and at least one live model route have
synchronized. Users and providers do not paste network addresses or Ethereum
contract addresses into normal setup commands.

For Tier-2 routes, a fresh buyer also needs no TPM, provider helper,
Administrator/root action, or verifier flag. The source installer builds the
verifier from the same checked-out tag, and `mayhem up` authenticates the active
catalog policy and shared public trust data. No verifier executable is fetched
from an operator or provider. Managed Tier-3 verifier distribution is not
activated in this source-only release; a route that cannot prove an available
higher tier falls back to the next tier it can prove.

The selected rail is persisted for later starts. Rails never borrow, convert,
or settle across one another. To switch a running gateway, restart it with the
new rail:

```bash
mayhem down --restart
mayhem up --rail tap --yes
```

`--restart` preserves provider registrations when the same supervised stack is
also serving. Ordinary `mayhem down` deliberately drains provider workers,
leaves their canonical routes, and stops the stack.

**Provide to the network** (earn on your hardware):

```bash
./install.sh --from-source
mayhem up --provider --yes
mayhem provider health
```

`mayhem up` starts everything supervised (peer, bridge, gateway, and the serving worker with `--provider`), health-checks each part, and prints endpoint and dashboard URLs you can copy. No browser required. For an update or temporary restart, `mayhem down --restart` preserves provider registrations so the next `mayhem up` resumes without re-onboarding. Ordinary `mayhem down` drains and leaves before stopping.

### Wire your coding agent to the gateway

The installer ships a checksum-pinned [opencode](https://opencode.ai) coding agent (skip with `--skip-opencode` if you have your own). One command wires it to the gateway — it registers a `mayhem` provider in `~/.config/opencode/opencode.json` and fills its model list live from `/v1/models`, leaving any other providers you have configured untouched:

```bash
mayhem opencode                   # wire (or repair) opencode for the local gateway
opencode run --model mayhem/<model-id> "Say hello from OpenMayhem."
```

`mayhem opencode` issues or reuses a dedicated local gateway bearer and repairs it if it was revoked or expired; the gateway token store keeps only its hash. When the catalog changes, the same command re-syncs the model list. Now your own agent loops run against models served from the network — including your own hardware.

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

### What you need installed first

Every release install builds locally from source. Set up Node.js 20+ with npm,
Rust, and the native compiler toolchain before running the installer. The source
build compiles llama.cpp through cmake and Rust bindings through bindgen;
missing `libclang` is the single most common source-build failure.

**Every OS for a source build:**
- Rust (stable, via [rustup](https://rustup.rs)) — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- Node.js 20+ with npm — carries the P2P runtime (Pear bootstrap)
- git, curl, unzip

**Linux (Debian/Ubuntu):**

```bash
sudo apt-get install -y build-essential clang libclang-dev cmake pkg-config git curl unzip
```

- GPU serving: an NVIDIA driver new enough for CUDA 12 (550+), and the CUDA toolkit (`nvcc`) so llama.cpp builds its CUDA kernels. Cloud GPU images usually ship both — check with `nvidia-smi` and `nvcc --version`.
- Managed Python backends do not require system Python, `venv`, `ensurepip`, or
  `pip`. Mayhem downloads the exact standalone `uv` archive for the host over
  HTTPS, verifies both archive and executable hashes, and atomically creates
  each frozen runtime under `~/.mayhem`.
- ComfyUI workflow providers need a verified ComfyUI runtime checkout for the
  blessed runtime, currently `comfyui-v0.30.1`. Use `python3` on `PATH`, or set
  `MAYHEM_COMFYUI_PYTHON` to the executable belonging to that checkout's
  environment. Workflow parts still come only from the signed parts index and
  must pass `mayhem provider parts pull`, `mayhem provider parts add`, and
  `mayhem provider parts admit`.
- AMD GPUs: ROCm, or a Vulkan loader for the Vulkan path.
- Tier 2 needs a TPM 2.0 exposed at `/dev/tpmrm0` plus `tpm2-tools` (`sudo apt-get install -y tpm2-tools`). The quote helper runs as the provider user. If the distro exposes the device as `root:tss`, add the existing login to that existing distro group once with `sudo usermod -aG tss "$USER"`, then log out and back in. Mayhem never creates users or groups, changes device ACLs, or runs hardware-proof helpers merely because it detected a TPM.

**macOS (Apple Silicon):**

```bash
xcode-select --install        # Xcode Command Line Tools: clang, libclang, git
brew install cmake node
```

Metal ships with the OS — nothing GPU-specific to install.

**Windows 11 or newer:**

- Visual Studio Build Tools with the "Desktop development with C++" workload (MSVC + Windows SDK)
- LLVM for libclang: `winget install LLVM.LLVM` (set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin` if the build doesn't find it)
- CMake: `winget install Kitware.CMake`
- Rust via rustup (MSVC toolchain, the default), Node.js 20+: `winget install OpenJS.NodeJS.LTS`
- Current NVIDIA driver for GPU serving
- Run `install.ps1` from PowerShell; the engine runs sandboxed (AppContainer). TPM 2.0 is mandatory on Windows 11, so the hardware needed for Tier 2 is normally present; the provider still explicitly selects the TPM quote helper when starting. Tier-2 providers also need the .NET SDK 6 or newer (`winget install Microsoft.DotNet.SDK.8`). The helper uses Windows TBS plus PCP/NCrypt from the normal provider account: no Administrator shell, service, cached quote, user/group creation, or TPM policy change is required.

### Agent install checklist (deterministic — for coding agents driving the install)

Most installs are driven by a coding agent. Agent: execute in THIS order, verify each step before the next, and never skip the preflight — several failure modes below are silent if you do.

1. **Detect OS + GPU** (`uname -a` / `systeminfo`; `nvidia-smi`, or on macOS assume Metal).
2. **Install the per-OS prerequisites above and verify each**: `rustc --version`, `node --version` (≥20), `cmake --version`, and on Linux confirm libclang (`ldconfig -p | grep libclang`) — a missing libclang is the single most common build failure and only surfaces mid-`cargo build`.
3. **NVIDIA machines**: `nvidia-smi` proves the driver, **not** the toolkit. If the provider will serve GPU models, also verify `nvcc --version`. A missing CUDA component is rejected by the provider preflight before a worker starts.
4. **Build/install**: `./install.sh --from-source` (Windows: `.\install.ps1 -FromSource`). Expect 20–40 min of compiling; that is normal. The installer selects exactly one local llama.cpp backend and verifies the installed binary's claimed capability before reporting success.
5. **Preflight before serving**: run `mayhem doctor` and read its verdicts. It reports per-backend feasibility for this machine (which engines can run, expected tok/s, memory fit). Do not start a provider whose chosen backend the doctor marks insufficient.
6. **Backend-specific extras — install only what the hardware/models need:**
   - **vLLM, TensorRT-LLM, and MLX**: Mayhem creates exact-version Python environments under `~/.mayhem/venvs`, discovers CUDA when applicable, and owns the backend caches. The `MAYHEM_*_PYTHON` variables are diagnostic overrides, not setup steps.
   - **ComfyUI workflows**: verify the blessed ComfyUI runtime checkout and its
     Python executable. Use `python3` if it belongs to that checkout; otherwise
     set `MAYHEM_COMFYUI_PYTHON` explicitly. Then pull verified parts and run
     `mayhem provider parts add` and `mayhem provider parts admit --write`
     before serving. Start workflow providers with
     `--artifact <comfy-runtime-dir>` so the worker uses the verified local
     ComfyUI checkout; the ledger artifact is the workflow class definition.
   - **Audio/image serving**: the engines are external binaries that must be on `PATH` (or pointed at by env): `whisper-cli` (`MAYHEM_WHISPER_CPP_BIN`), `piper` (`MAYHEM_PIPER_BIN`), `sd-cli` (`MAYHEM_STABLE_DIFFUSION_CPP_BIN`). Accelerator selection is automatic. Text-only providers can ignore this.
7. **Start and verify**: `mayhem up --yes` (or `--provider`), then confirm the gateway answers (`curl http://127.0.0.1:11435/v1/models`) and, for providers, `mayhem provider health` is green AND the served model appears in `/v1/models`. A green health with a missing route means the model failed to load — re-run `mayhem doctor` and check the backend extras above.
8. **Explain to the human** what was installed, where the dashboards are, and (providers) what their earnings depend on.

Rule of thumb for agents: `llama.cpp`/GGUF models need nothing beyond steps 1–4 on any OS — that is the zero-extra path. Everything in step 6 is only for the specific backends named there.

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
| **Privacy-sensitive work** | Inspect route evidence for the confidential-compute property you need. `--min-att-tier 3` is a hard numeric filter, but tiers are not privacy supersets: a Tier 4 identity route can satisfy that minimum without providing Tier 3 prompt confidentiality. For verified business identity, use `--require-kyb`. |
| **Someone who distrusts pricing pages** | Every price on this network is published with the formula and inputs that produced it. Recompute it yourself; you'll get the same number. |

### Start, look around

```bash
mayhem up --rail fiat --yes       # fiat, tap, or tnk; persists across restarts
mayhem models --gateway           # canonical markets, prices, and current route counts
mayhem status                     # component health, ports, balances
mayhem price show <model-id> --tier 1   # live market price + its derivation
```

`mayhem models --gateway` also lists an admin-created, priced tier market before a provider has
joined it. Its market row says `routes=0 status=no_eligible_provider_yet`; it is discoverable for
provider onboarding but cannot receive a request. `/v1/models` exposes durable registration as
`registered_route_count`, while `route_count`, `providers_online`, `availability=routable`, and
`mayhem.route_candidates[]` come only from fresh heartbeats that pass the same capacity,
capability, context, tier, rail, attestation, and circuit-breaker checks used for dispatch. A crash
or `mayhem down --restart` ages out of live counts but keeps durable registration, so the same
provider resumes without re-onboarding. A deliberate remove, enclave-changing switch,
drain-to-stop, provider stop, or ordinary `mayhem down` signs the corresponding leave before
retiring the worker.

### Pay — card first, one price everywhere

Everything you see is in dollars. Internally the ledger counts in atto-USD (`au`, 10⁻¹⁸ dollars), which is why a $0.01-per-million-token embedding model still has an exact integer price per single token and the market can move prices in tiny steps. You'll never handle raw `au`; the CLI and dashboards show dollars. The price is the same dollar figure on every rail, and rails never mix during settlement.

The gateway spends only the rail selected by `mayhem up --rail <fiat|tap|tnk>`;
it never falls through to another rail. `mayhem balance`
defaults to that selected rail, and `mayhem balance --rail <rail>` inspects a
different bucket without changing routing.

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

The signed Checkout price and ledger credit stay in USD. Stripe Adaptive
Pricing may present and collect a supported local currency such as EUR or GBP;
that local display never changes the USD-denominated model price or mixes
payment rails.

```bash
mayhem deposit status
mayhem balance
```

```text
fiat: 10.000000 USD    tap: 0.000000 USD    tnk: 0.000000 USD
```

For most people, that is the whole payments story.

There is no public Mayhem paygate website. The local CLI relays a signed
checkout request to the admin service and always prints the resulting
`https://checkout.stripe.com/...` URL for copy/paste before trying to open a
browser. Pass `--no-open` on a remote terminal. Stripe hosts checkout and its
confirmation UI; Mayhem does not invent a redirect domain.

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

**TAP:** send TAP to your in-app Ethereum address, then deposit. The app checks balances and gas, simulates the transaction, and only then signs approve+deposit over an Ethereum RPC (`--eth-rpc` overrides the public default):

```bash
mayhem pay tap --amount-tap 10.00            # dry-run: balances, gas check, simulation
mayhem pay tap --amount-tap 10.00 --confirm  # sign + broadcast
```

**TNK:** send TNK to your in-app Trac address, then deposit. Signed locally, confirmed through the local MSB that ships with the node. No external RPC at all:

```bash
mayhem pay tnk --amount 10.00 --submit-intent --submit-transfer --wait
```

Either way, `mayhem deposit status` and `mayhem balance` show the credit land. The ETH that comes along when you top up an Ethereum address covers your own deposit gas, and the network's settlement rollups are sponsored by the operator. Use the on-chain rails if you want them; the card rail is there so nobody has to.

</details>

### Control what you pay and what serves you

You never *have* to set anything: with no ceiling, a session simply pays the current market price, and that price is locked for the whole session — no mid-conversation repricing. The controls below are for when you want guarantees.

**One unit to know:** everything price-shaped ends in `-au` and is an integer in **atto-USD per token** (1 dollar = 10¹⁸ au). Handy conversion: *$X per 1M tokens = X × 10¹² au*. So "never pay more than $0.50 per million tokens" is `500000000000`. `mayhem price show <model>` prints the live market value in the same unit, so you can sanity-check your zeros against it.

**Persistent ceiling (set once, applies to every session):**

```bash
mayhem config max-price 500000000000      # ceiling = $0.50 per 1M tokens, survives restarts
mayhem config max-price                   # inspect
mayhem config max-price --clear           # remove
mayhem up --max-price-au 500000000000     # same thing for one run only (beats the config)
```

**Discovery filters (see what qualifies before you spend):**

```bash
mayhem models --gateway --min-att-tier 3  # Numeric tier 3+ markets; inspect protection evidence before use
mayhem models --gateway --require-kyb     # only identity-verified businesses
mayhem models --gateway --quant int4      # filter by quantization
```

**Per-request headers** — any OpenAI client, no SDK changes. A header beats the `--max-price-au` flag, which beats the persistent config. Where to put them:

```bash
# curl
curl http://127.0.0.1:11435/v1/chat/completions \
  -H "X-Mayhem-Max-Price-Au: 500000000000" \
  -H "X-Mayhem-Min-Att-Tier: 3" \
  -d '{"model":"...","messages":[...]}'
```

```python
# OpenAI Python SDK — once for every call this client makes:
client = OpenAI(base_url="http://127.0.0.1:11435/v1", api_key="sk-mayhem-...",
                default_headers={"X-Mayhem-Max-Price-Au": "500000000000"})
# or per call:
client.chat.completions.create(..., extra_headers={"X-Mayhem-Min-Ctx": "128000"})
```

The Node SDK takes `defaultHeaders: {...}` on the client the same way, and any framework that lets you add custom headers to an OpenAI provider (opencode, LangChain, …) carries them on every call in the loop.

| Request header | Value | Effect |
|---|---|---|
| `X-Mayhem-Max-Price-Au` | integer au per token | price ceiling for this request |
| `X-Mayhem-Min-Att-Tier` | `1`–`4` (or `T3`) | minimum trust tier (hard filter, never downgraded) |
| `X-Mayhem-Min-Ctx` | tokens, e.g. `128000` | minimum context window |
| `X-Mayhem-Quant` | `fp16`, `int4`, `nvfp4`, … | required quantization bucket |
| `X-Mayhem-Hedge` | `1` | race a second provider for latency |
| `X-Mayhem-Min-Tok-S` | tokens/sec | throughput floor |
| `X-Mayhem-Max-Wait-Ms` | ms (max 60000) | how long to wait for a free provider before giving up (default 10s) |

These are **routing filters, not refunds**: a request whose conditions no live provider meets fails up front with a clear error and costs nothing — you're never silently served something worse or billed something higher. (Failover timing knobs — open/TTFT/stall timeouts — are catalog-controlled per model; sending them as headers is rejected by design.)

**Response headers tell you what actually happened** on every reply: `X-Mayhem-Backend` (which provider served it), `X-Mayhem-Usage` (token counts), `X-Mayhem-Receipt` (the signed billing receipt, when enabled), `X-Mayhem-TTFT-Ms` (time to first token). `mayhem history` shows your recent sessions — per session, provider, model, spend — without any of this.

**If a session goes wrong**, you're not stuck writing a support ticket: `mayhem dispute --session-id <id> --reason service_failure` opens a bonded on-ledger dispute, and if it's never resolved the bond auto-refunds after a timeout (`mayhem dispute-expire`).

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
| `mayhem up` / `mayhem down` | start the stack / deliberately drain, leave provider routes, and stop |
| `mayhem down --restart` | stop for an update or temporary restart while preserving provider registrations |
| `mayhem status` | live component state, ports, sync, balances |
| `mayhem models --gateway` | canonical priced markets, capabilities, tiers, quant, route counts, and availability |
| `mayhem opencode` | wire the bundled opencode agent to the gateway; re-run to re-sync models |
| `mayhem price show <model> [--tier]` | current market price with published derivation |
| `mayhem pay stripe` / `mayhem deposit tap\|tnk` | buy credits on your chosen rail (card is the default) |
| `mayhem deposit status` | pending/confirmed deposits from the ledger |
| `mayhem balance [--rail fiat\|tap\|tnk]` | your prepaid credit balance per rail |
| `mayhem history` | recent sessions: provider, model, spend, receipts |
| `mayhem config max-price [au]` | persistent price ceiling (inspect / set / `--clear`) |
| `mayhem tokens create/list/revoke` | shared-gateway keys with per-token budget (`10/day`), rate limit, model allowlist |
| `mayhem dispute` / `mayhem dispute-expire` | open a bonded session dispute / reclaim the bond after timeout |
| `mayhem wallet show/backup` | your addresses / reveal the recovery mnemonic (gated) |
| `mayhem doctor` | probe this machine's hardware and what it can run |
| `mayhem balance` | per-rail balances |
| `mayhem config max-price / set` | persistent spending and routing defaults |
| `mayhem tokens create/list/revoke` | bearer tokens for shared gateways |
| `mayhem wallet show/backup/import/passwd` | your key, your custody |
| Source release update | check out the exact new tag, then rerun the complete source installer; never replace one binary |

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

That's the whole Tier-1 happy path. The software probes your hardware, shows which admin-approved models fit (with estimated speed and context, before anything downloads), fetches and verifies the model, and starts serving. The provider dashboard shows download, verify, seal, load, and serving progress live. Your Mayhem binary may be built from source: its measured hash is signed evidence, not an admin approval list. The model, artifact, manifest, room, and market still come only from the admin-published catalog and ledger.

An LLM catalog context is a ceiling, not a required setting. Use
`mayhem provider start --ctx <tokens>` or `mayhem provider serve add <enclave> --ctx <tokens>`
when you want a smaller fitting commitment. Manual join and streamlined start run the same
hardware-fit selection; explicit values are kept exactly or rejected before model download, and
every supported context bracket has its own admin-published price.

Higher-tier proof is explicit. It never needs a manual provider approval after it verifies, and Mayhem does not auto-run a proof helper just because hardware was detected. For example, a Linux TPM provider starts Tier 2 with:

```bash
mayhem up --provider --yes \
  --provider-hardware-quote-kind tpm2-quote-ek \
  --provider-hardware-quote-command scripts/hardware/mayhem-tpm2-quote-linux.sh
```

Windows uses `scripts/hardware/mayhem-tpm2-quote-windows.ps1` from the same normal account that runs `mayhem`; it obtains a fresh nonce-bound quote through Windows TBS and binds the TPM EK through PCP/NCrypt and the manufacturer certificate chain. It does not need Administrator privileges or a privileged helper. Tier-3 operators pass the matching confidential-compute quote kind/helper. A valid Tier-2 or Tier-3 proof joins the existing canonical tier market automatically. Tier 4 is different: it is the admin's KYB identity overlay.

Those explicit quote commands are provider-side evidence collection. Buyers do
not install or select them: the signed policy and the verifier built from the
same source tag select the verification profile automatically, and only routes
verifiable under that exact active policy become locally routable.

To add a higher-tier worker without restarting an already running stack, pass the same explicit proof options to `mayhem provider serve add <enclave-id> --hardware-quote-kind <kind> --hardware-quote-command <path>`.

Model downloads come from Hugging Face. Without a token you download anonymously, and anonymous pulls are rate-limited: multi-gigabyte models can slow to a crawl or fail partway. A free Hugging Face token fixes that. Getting one takes two minutes, once:

1. Create a free account at [huggingface.co/join](https://huggingface.co/join).
2. Open [Settings → Access Tokens](https://huggingface.co/settings/tokens), click "Create new token", pick the **Read** role, and copy the `hf_...` value.

Then either export it or point the CLI at a file containing it:

```bash
export HF_TOKEN=hf_...                    # or:
mayhem provider start --hf-token-file ~/.mayhem/hf.txt
```

The Read role can only download; keep the token out of anything you commit or share.

There is no separate model installer to maintain. To serve a specific canonical
model, name it when starting the normal Mayhem stack:

```bash
# Speech-to-text
mayhem up --provider --provider-enclave nvidia/parakeet-tdt-0.6b-v3 --yes

# Music generation
mayhem up --provider --provider-enclave acestep/ace-step-1.5 --yes
```

Run `mayhem doctor` first and serve only a model it marks feasible. On start,
Mayhem resolves the active admin-created enclave from the ledger, downloads the
immutable Hugging Face artifact and every required sidecar, verifies their
signed hashes and Merkle roots, prepares the managed backend, runs the signed
functional self-test, and joins the enclave's canonical room. Parakeet and
ACE-Step use this same path; providers do not clone either model repository,
install a separate model server, choose arbitrary weights, or submit a local
model to the catalog. `--hf-token-file` only authenticates the download and
never changes which artifact is allowed.

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
mayhem provider rails set --rails fiat,tap,tnk --submit    # accept everything (recommended)
mayhem provider rails get
```

`--submit` sends the provider-signed, free rails declaration through the local
read-only peer to the admin indexer. Without it, the command is a dry run and
only prints the copy/paste submission command.

When accepting fiat, finish the one-time Stripe Connect setup from the same
terminal:

```bash
mayhem provider stripe onboard --country DE
mayhem provider stripe status
```

The onboarding command always prints the exact `https://connect.stripe.com/...`
URL before attempting to open a browser. On a remote terminal, pass `--no-open`
and open that copy/paste URL anywhere. Once Stripe reports
`details_submitted`, payouts enabled, and transfers enabled, `onboard` or
`status` signs and submits the verified fiat payout binding automatically.
There is no Mayhem-admin approval step. The same provider identity reuses that
Stripe account across every model and room. To reuse a ready account under a
different provider identity, run `mayhem provider stripe relink --help`; the
source and destination provider identities must both consent, and entering an
`acct_...` value is never enough.

If the provider already has a Standard Stripe account that has never been used
with Mayhem, use `mayhem provider stripe adopt --country DE`. Stripe's hosted
OAuth page lets the provider choose the account; Mayhem never accepts a typed
account id. The URL is still printed before browser open and works from a
headless terminal with `--no-open`.

To replace this provider identity's current Connect account, run
`mayhem provider stripe rotate --country DE` (using the provider's actual
country). The command requires the current account as a signed compare-and-swap,
prints a fresh hosted onboarding URL, and leaves existing liabilities bound to
the previous payout revision until the replacement is verified and reaches its
recorded activation epoch. Readiness is retained per provider and account, so
verifying the replacement cannot make the still-active account ineligible.

For TAP and TNK, bind the local provider wallet directly:

```bash
mayhem provider payout set --rail tap --submit
mayhem provider payout set --rail tnk --submit
mayhem provider payout get
```

These commands sign both the provider intent and the target-wallet ownership
proof. A separate or shared target wallet is also supported through
`--target-wallet-key-file`; that target wallet co-signs each provider binding.
Use `mayhem provider payout rotate --rail tap --submit` or the corresponding
`tnk` command to replace a target. If `E` is the latest fully applied epoch, a
first Stripe/TAP/TNK binding activates at `E+1` and a rotation activates at
`E+2`; `mayhem provider payout get` reports the exact activation epoch. The old
revision remains current until that boundary, and reservations and earnings
already tied to it continue to settle there afterward.

All payout submissions travel through the provider's local read-only peer. The
sole canonical indexer verifies the signatures and appends valid bindings
automatically; providers receive no writer capability, and no operator SSH or
per-provider admin transaction is involved. Bindings belong to the provider
identity, not a model, enclave, room, or machine. Registration can complete
without one, but paid sessions on a rail require its active verified binding.

Earnings settle **automatically after every finalized epoch (hourly)** on each rail you earned in. The operator's bounded, idempotent payout worker reconciles Stripe transfers, TNK transfers, and TAP root/fee/burn work from the same canonical epoch/apply hash; providers never run an admin settlement command. You keep 85%; the network fee funds gas sponsorship and operations.

Two rails push the money to you. One is a pull, for a reason:

| Rail you accepted | How you receive it | Push or pull |
|---|---|---|
| `fiat` | Onboarding happens once: the CLI prints a Stripe Connect link, you finish it in the browser, and the verified binding makes fiat sessions eligible. Buyer credit remains USD-denominated while Stripe presents supported local currencies; settlement converts separately into the account's verified payout currency. | Push, automatic |
| `tnk` | After the one-time wallet ownership binding, epoch settlement sends real TNK transfers straight to your verified Trac address, with fees sponsored. | Push, automatic |
| `tap` | The epoch settlement publishes a settlement root on Ethereum and your earnings accumulate under it. `mayhem claim` runs a Merkle claim that transfers everything owed to your Ethereum address in one transaction. | Pull, you claim |

TAP is a pull because paying every provider on Ethereum every hour would burn gas per provider per epoch. The settlement root is cumulative instead: unclaimed earnings pile up losslessly, and you claim when it's worth a transaction — after a day, a month, whenever. Nothing expires, claiming late costs nothing, and one claim sweeps everything since your last one. The verified target wallet authorizes the claim and pays its Ethereum gas.

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
| `mayhem provider payout set/get/rotate` | bind or rotate a provider-signed TAP/TNK destination |
| `mayhem provider stripe onboard/adopt/rotate/relink/status` | create, adopt an existing Standard account, replace, reuse, or inspect a verified Stripe Connect payout binding |
| `mayhem provider min-ask set/get` | your price floor per market |
| `mayhem provider limits set` | concurrency / accept-rate / daily budget caps |
| `mayhem provider drain` | graceful sign-off |
| `mayhem provider serve plan/add/remove` | see what fits this machine; add/remove served models live |
| `mayhem provider earnings` | earnings, holdback, claimables per rail |
| `mayhem reputation` | your standing and why |
| `mayhem claim` | execute TAP Merkle claims |

---

## The market

Every model × tier is an independent market. The admin seeds a starting price once; after that the price floats on measured utilization, epoch by epoch. Providers post min-asks, users post max-bids, and a session locks its price at the moment it opens. The number you agreed to is the number you settle at.

Prices are quoted separately for input and output tokens, in dollars per million. Embeddings bill per input token. Image generation bills per image and per step, scaled by resolution. Audio has its own metered units.

Every epoch's price is published with its derivation: the seed, the measured utilization, the public constants, and the settled work behind them. Run the formula yourself and you get the same number. `mayhem price show` prints the derivation, while the dashboard's bounded Markets workspace shows the current catalog reference price and supply facts without inventing historical intervals. A newly opened market is visible immediately even at zero routes; it remains unroutable until an eligible provider joins.

Context is part of the deal too. Providers advertise the context window they serve; the network verifies it with targeted probes and it's guaranteed for the duration of your session. Larger context brackets clear at their own prices, and a `min-ctx` filter routes you only to providers with the headroom you need. When every provider is busy, an opt-in `--max-wait` holds your request for the next free slot instead of failing it.

## Pricing

Every market has one price at a time, and everyone active in the same epoch pays that same number. Nobody types the price in. Not providers, not users, and after seeding the starting price once, not us either.

The clock behind it is the epoch: the network's settlement window, one hour by default. At the end of each hour, all the signed receipts from that hour are settled, and that settlement doubles as the price input for the next hour.

Here is the loop. Each epoch the network measures how busy a market was: paid work that actually settled, divided by the capacity of the providers taking part. Above roughly 85% utilization the price steps up for the next epoch. Below it, the price steps down. A single step is capped at 10%, but steps compound, so a market that stays hot climbs hour after hour until enough supply shows up or demand cools.

Providers steer it with a min-ask, the lowest price they are willing to serve at. When the market price is under your ask, you sit out. That shrinks supply, the market runs hotter, and the price climbs until it crosses your number and you flow back in. An ask does not name the price. It decides when you work, and who is working is what moves the price.

Users steer it from the other side with a max-bid. If the price is above your bid, you get a clear "no provider at your price" and pay nothing. Enough buyers stepping back cools the market, and the price sinks toward where the buyers actually are.

Your session keeps its price. The rate at the moment a session opens is locked into the spend voucher and the provider receipt, and settlement uses that locked rate no matter where the market moves afterwards. Nobody can reprice work you already agreed to.

Two guardrails hold it steady. A market with fewer than two active providers stays pinned at its seed price, so a lone machine cannot steer the controller. And utilization counts only receipt-verified settled work, so posted intent, fake demand, and phantom supply move nothing.

### What a machine can earn

The table below spans hardware from a laptop to a datacenter-class box. What a machine can hold decides what it earns: a 24 GB card or a big-memory box runs `qwen3.6-35b-a3b` at its reference price of $0.05 per million input tokens and $0.35 per million output; smaller machines run a 7B–14B model instead, which clears at roughly a third to a fifth of that price but runs several times faster. Every row assumes the market sits at the seed price, demand keeps the machine busy around the clock, input runs at three tokens for every output, and the 15% network fee is already deducted. The GB10 throughput is measured; every other figure, and the smaller-model prices, are estimates.

| Machine | Model | Concurrent sessions | Sustained output | Net per month |
|---------|-------|--------------------|------------------|---------------|
| MacBook Pro, 48 GB | 7B–14B | 2–4 | ~120–220 tok/s | ~$40–90 |
| RTX 3080, 10 GB | 7B–8B | 4–6 | ~400–700 tok/s | ~$90–170 |
| RTX 3060, 12 GB | 7B–8B | 4–6 | ~300–500 tok/s | ~$70–130 |
| RTX 4060 Ti, 16 GB | 7B–14B | 4–8 | ~300–550 tok/s | ~$80–150 |
| RTX 4070 / 4070 Ti, 12 GB | 7B–8B | 6–8 | ~500–900 tok/s | ~$120–220 |
| Mac mini M4 Pro, 64 GB | 35B-A3B | 2–4 | ~80–110 tok/s | ~$80–120 |
| RTX 3090, 24 GB | 14B, or 35B-A3B | 6–10 | ~700–1300 tok/s | ~$140–280 |
| GB10-class box, 128 GB (DGX Spark and similar) | 35B-A3B | 2 | ~115 tok/s | ~$105–135 |
| Mac Studio M4 Max | 35B-A3B | 4–6 | ~160–220 tok/s | ~$150–240 |
| GB10-class box, 128 GB | 35B-A3B | 6 | ~230 tok/s | ~$210–260 |
| Mac Studio M3 Ultra | 35B-A3B | 6–8 | ~250–350 tok/s | ~$230–380 |
| RTX 4090 desktop, 24 GB | 35B-A3B | 8 | ~300–500 tok/s | ~$280–550 |

Three things move these numbers. Concurrency is the cheapest lever: these models batch well, so serving six sessions instead of two roughly doubles the take on the same hardware. The market price is the biggest one: the table uses the frozen seed, and a market that runs hot walks the price up toward 4× the reference, which multiplies every row with it. And the numbers assume saturation; a machine only earns while sessions are actually flowing. A fast small model on a cheap card can out-earn a slow large model, because per-token price drops more slowly than throughput rises, so the 12–24 GB range is the sweet spot for consumer hardware; 8 GB or less is limited to 3B–4B models and short context, where earnings are real but thin. For cost context, a box drawing 150 W runs about €16 a month in electricity at typical European rates.

Attestation tiers describe what trust evidence a provider has. They don't all mean the same thing, and a higher number isn't simply "everything below it plus more."

| Tier | Plain meaning | Could my prompts potentially be used for training? |
|------|---------------|----------------------------------|
| Tier 1 | Runs the OpenMayhem software. Trust is mostly economic: if they cheat, probes, receipts, holdbacks, and slashing cost them money. | Yes. |
| Tier 2 | A proven, unique physical machine, anchored in its TPM security chip — Windows 11 or newer (TPM is mandatory there) and Linux machines with a TPM, which covers most PCs and GPU servers. Stops fake hardware; the served model is verified the same way as Tier 1, by network spot checks. Apple machines join when macOS 27 ships its attestation support. | Yes. |
| Tier 3 | Hardware confidential compute — an AMD SEV-SNP confidential VM with an NVIDIA GPU in CC mode. Your prompt is protected even from the provider's own machine, and the served model is the pinned model by construction, not by spot check. | No. |
| Tier 4 | A real, identity-verified business (KYB). You know who they are — and KYB is not just a passport check: the process asks what they run, whether they can read your data at all, and what they do with it. We publish which KYB'd entity is behind which offering, so their answers are on the record under their legal name. | Depends — on what that business runs and declared. Their published KYB profile says. |

## Available Models

The model catalog is signed and canonical: `mayhem models` reads the ledger anchor and verifies the signed catalog release, so you always discover current models without requiring a repo update. Providers opt into canonical enclaves the network operator creates — they do not set prices, create canonical rooms, or submit arbitrary models — which is what keeps every listed model a verified, hash-pinned artifact instead of a claim.

The launch roster is being onboarded model by model right now; `mayhem models --gateway` is always the live truth. The table below lists the launch set in onboarding order, newest live model on top. Status moves to **live** as each model finishes calibration and its signed catalog entry is published.

| Model | Category | Class | Status |
|-------|----------|-------|--------|
| `upscale.diffusion` | Comfy workflow, SeedVR2 diffusion upscale/restore | A/B | signed dev policy; `.70` reference admission passed; paid route proof pending |
| `upscale.conv.le24mp` | Comfy workflow, standalone 4x image upscale | A/B | **live** |
| `video.minimax_h3.r2v` | Comfy workflow, MiniMax H3 reference-media video with native audio | D | **live** |
| `video.minimax_h3.t2v_i2v` | Comfy workflow, MiniMax H3 text/image-to-video with native audio | D | **live** |
| `video.minimax_h3.spectrum` | Comfy workflow, optional MiniMax H3 Spectrum enhancer lane | D | signed dev policy; paid fiat proof passed; owner benefit review pending |
| `huihui-ai/Huihui-Agents-A1-abliterated` | Multimodal LLM, vision/video input, tools, uncensored, 262K ctx | C | **live** |
| `ResembleAI/chatterbox` | TTS + zero-shot voice cloning | A/B | **live** |
| `SulphurAI/Sulphur-2-base` | Video with synchronized audio, uncensored | C/D | **live** |
| `prism-ml/Ternary-Bonsai-27B` | LLM, reasoning, 262K ctx, 2-bit ternary | B | **live** |
| `ACE-Step/Ace-Step1.5` | Music generation | B/C | **live** |
| `nvidia/parakeet-tdt-0.6b-v3` | ASR | A | **live** |
| `Tongyi-MAI/Z-Image-Turbo` | Image | B/C | **live** |
| `google/gemma-4-E4B-it` | LLM, small + vision, laptop/CPU-friendly | A/B | cataloged — not served by the owned fleet |
| `HauhauCS/Qwen3.6-35B-A3B-Uncensored-HauhauCS-Aggressive` (`@nvfp4`) | LLM, uncensored, 262K ctx | C | **live** |
| `Cactus-Compute/needle` | Deterministic tools-only specialist, 30.4M, CPU/CUDA | A | **live** |
| `microsoft/Mage-Flow-Edit-Turbo` | Image editing from a reference image + instruction, 512–2048 native | B | onboarding — **next (⑩)** |
| `microsoft/Mage-Flow-Turbo` | Image, 4-step, 512–2048 native resolution, any aspect ratio | B | onboarding — **next (⑩)** |
| `deepreinforce-ai/Ornith-1.0-35B` | LLM, agentic MoE, 262K ctx — GGUF Q4 / MLX 4-bit / NVFP4 | B/C | onboarding — **⑪** |
| `NousResearch/Hermes-3-Llama-3.1-70B` (`@mlx-4bit`) | LLM, 70B, 128K ctx | D | onboarding — ⑬ |
| `openai/gpt-oss-20b` | LLM, agentic | B | onboarding |
| `Qwen/Qwen3.6-35B-A3B` | LLM, MoE, 262K ctx | C | onboarding |
| `mistralai/Devstral-Small-2-24B-Instruct-2512` | LLM, coding | C | onboarding |
| `openai/gpt-oss-120b` | LLM, flagship | D | onboarding |
| `HauhauCS/Qwen3.5-9B-Uncensored-HauhauCS-Aggressive` | LLM, uncensored | B | onboarding |
| `wepiqx/Qwythos-9B-Claude-Mythos-5-1M-MTP-SHQ8-GGUF` | LLM, 1M ctx, MTP | B | onboarding |
| `baidu/Unlimited-OCR` | OCR, vision-language | A/B | onboarding |
| `black-forest-labs/FLUX.2-klein-4B` | Image | B/C | onboarding |
| `lodestones/Chroma1-HD` | Image, uncensored | C | onboarding |
| `Qwen/Qwen-Image-2512` | Image, text rendering | C | onboarding |
| `hexgrad/Kokoro-82M` | TTS (CPU-friendly) | A | onboarding |
| `openai/whisper-large-v3-turbo` | ASR | A/B | onboarding |
| `BAAI/bge-m3` | Embeddings (CPU-friendly) | A | onboarding |
| `Qwen/Qwen3-Embedding-0.6B` | Embeddings (CPU-friendly) | A | onboarding |
| `BAAI/bge-reranker-v2-m3` | Reranker (CPU-friendly) | A | onboarding |

Class = smallest machine class that serves it well: **A** CPU/laptop, **B** consumer GPU 8–12GB, **C** enthusiast GPU 16–24GB, **D** pro 48–80GB or big-memory Apple Silicon. Many models ship multiple artifacts (GGUF for llama.cpp, MLX for Apple Silicon, NVFP4 for Blackwell), so the same model can serve from very different hardware. Most launch models are Apache, MIT, or CC-BY licensed; models under a vendor license (Gemma, Llama, LTX-2) carry that license in their signed catalog entry. Larger flagships join after launch as capable hardware comes online.

**Routes:**

| Class | Routes |
|-------|--------|
| Text generation | `/v1/chat/completions`, `/v1/completions`, `/v1/responses` — tools, JSON mode, streaming, vision input where the catalog says so |
| Embedding | `/v1/embeddings` |
| Image generation | `/v1/images/generations` |
| Video generation | `/v1/videos` |
| Speech and transcription | `/v1/audio/speech`, `/v1/audio/transcriptions` |
| Audio and music generation | `/v1/audio/generations`, `/v1/music/generations` |
| ComfyUI workflows | `/v1/workflows` — Mayhem-native workflow graph execution |

### ComfyUI workflows

When the signed catalog exposes a workflow-enabled model, users call the same
local gateway with a JSON body containing `model` and `workflow`. The gateway
derives the graph hash, required parts, output class, runtime id, and quoted
usage from the admin-signed workflow policy before it opens a paid session.
Graphs using non-whitelisted nodes, unknown parts, unsafe paths, or outputs
outside the signed caps fail before spend.

Discover live workflow capacity:

```bash
curl 'http://127.0.0.1:11435/v1/models?endpoint_family=mayhem_comfy_workflows&live=true'
```

The full provider/user workflow, outcome-class grid, and current signed parts inventory are in
**[COMFY-CHEATSHEET.md](COMFY-CHEATSHEET.md)**.
Every Comfy calibration must enumerate all files the graph loads in the signed
workflow policy. Missing checkpoints, encoders, VAEs, LoRAs, ControlNets,
upscalers, lipsync models, or helper models must be mirrored and signed as
parts before the proof is valid.

Schematic request shape:

```json
{
  "model": "<workflow-enabled-model-id>",
  "workflow": { "<node-id>": { "class_type": "<whitelisted-node>", "inputs": {} } },
  "response_format": "artifact"
}
```

Providers never download parts because a user asks for them. A provider first
pulls verified parts from the anchored parts index, proves a reference graph and
headroom for an outcome class, then advertises that class through ordinary
heartbeats. The dashboard route counts for workflows therefore mean live
admitted capacity, not merely catalog presence.

Provider start shape:

```bash
mayhem up --provider --provider-enclave <workflow-enclave-id> --artifact <comfy-runtime-dir> --workflow-class-definition <definition.json> --yes
```

`--workflow-class-definition` may be omitted only when the signed catalog embeds
`workflow.outcome_class_definition`. The source release canonicalizes
integer-valued workflow JSON floats for graph hashing, so `1.0` and `1` bind to
the same voucher.

Krea 2 Turbo's base lane generates the image; a 4x result is a single workflow
graph that adds an approved upscaler stage after Krea. It is published as a
separate workflow market for honest pricing and admission, but users still send
one `/v1/workflows` request and receive one receipt. Gateways and providers need
`v0.2.107` or newer for Krea+4x because the voucher must meter the signed
upscaler scale, not only the base latent size.

Chatterbox uses `voice: "default"` for ordinary speech. For zero-shot cloning, add a bounded
base64 WAV reference:

```json
{
  "model": "ResembleAI/chatterbox",
  "input": "This voice was cloned from the supplied reference.",
  "voice": "default",
  "response_format": "wav",
  "reference_audio": {
    "data": "<base64-wav>",
    "encoding": "base64",
    "content_type": "audio/wav"
  }
}
```

## Dashboards

`mayhem up` serves local dashboards on the gateway's canonical loopback origin,
`http://127.0.0.1:11435`, and always prints the tokenized user and provider URLs. Open the exact
printed URL: the bare paths intentionally return `401`.

| Dashboard | Path | Shows |
|-----------|------|-------|
| User | `/mayhem/dashboard` | gateway readiness, model choice, Playground, receipts, balance, connection setup |
| Provider | `/mayhem/dashboard/provider` | serving routes, preparation progress, current capacity, canonical earnings, reputation |
| Network explorer | `/mayhem/dashboard/network` | bounded model, provider, market, route-activity, and evidence snapshots |

The default copy/paste URL shapes are:

```text
http://127.0.0.1:11435/mayhem/dashboard?token=<generated-token>
http://127.0.0.1:11435/mayhem/dashboard/provider?token=<generated-token>
```

Open the tokenized URL once for each gateway run. The gateway exchanges it for an
HttpOnly browser cookie and redirects to a clean dashboard URL. That browser remains
authenticated without an idle timeout, including after closing and reopening it, for
as long as the same gateway process is running. Restarting the gateway rotates both
secrets, so the newly printed URL must be opened once again. Dashboard cookies are
transport-secure; use the documented loopback SSH forwarding flow or HTTPS for a
remote gateway, never plain remote HTTP.

When Mayhem runs on a remote machine, forward the same loopback port and then open the URL printed
by `mayhem up`:

```bash
ssh -N -L 11435:127.0.0.1:11435 user@remote-host
```

The SSH tunnel only transports the connection; it does not replace the dashboard token.

Production figures come from the current catalog, contract, ledger, gateway records, or provider heartbeats, with freshness and unavailable states kept explicit. Workbench fixtures are labelled and isolated from production data.

For dashboard UI work without starting the full stack, use the isolated fixture
[dashboard workbench](crates/mayhem-gateway/DASHBOARD_WORKBENCH.md).

## How the money is secured

- **Session price lock.** Every session freezes its rate at open, and settlement validates against the locked rate forever. Market moves never touch work already agreed.
- **Signed receipts, epoch settlement.** Providers sign for the work they deliver, receipts roll into per-epoch settlement roots on the ledger, and the contract enforces that debits equal earnings, per rail, every epoch.
- **Balance-backed authorization.** Spending is bounded by real on-ledger balance, enforced on the provider side where a modified client can't reach it.
- **Challengeable everything.** Over-credited commits and fabricated prices can be fraud-proven by anyone holding the evidence. Bad commits are voided and their submitters penalized, and a running challenger watches every window.
- **Deposits are oracle evidence.** Credits come from verified payment events — Stripe webhooks are signature-checked and replay-deduplicated, chain deposits are watched and confirmed — never from self-asserted writes.
- **Direct first, bounded relay fallback.** An official relay forwards bounded Noise-authenticated
  session bytes without gaining plaintext, receipt, provider, payment, or writer authority.
  Fragmented and transport-coalesced frames keep their order across relay-to-direct recovery;
  replay is rejected, and sustained over-budget traffic closes only the offending link/session.

## Install

`v0.2.108` is source-only. The GitHub release contains the tagged source, not
unsigned platform executables. Install from the exact release tag.

macOS/Linux:

```bash
git clone https://github.com/Trac-Systems/openmayhem.git
cd openmayhem
git checkout --detach v0.2.108
./install.sh --from-source
```

Windows PowerShell:

```powershell
git clone https://github.com/Trac-Systems/openmayhem.git
Set-Location openmayhem
git checkout --detach v0.2.108
.\install.ps1 -FromSource
```

The source installer compiles only OpenMayhem's generic runtimes and backends.
Model weights and model-specific catalog records remain external; adding a
catalog model does not embed its weights or create another Mayhem executable.
Pinned Pear `2.0.4` is provisioned without modifying the checked-in Intercom
runtime.

Installers print a copy/paste `PATH` command even when they update your shell profile, and anything that would open a browser prints the URL first. The whole system works from a terminal alone.

Current releases are source-only. To update, fetch the exact new tag and rerun
the complete source installer so every binary and runtime asset comes from one
revision. Never copy or replace an individual executable. The dormant
signed-artifact updater is not the supported release path unless the project
owner explicitly enables signed executable distribution later.

When a temporary stop is needed around an update, use `mayhem down --restart`;
the next `mayhem up` reuses durable provider registrations. Ordinary
`mayhem down` deliberately drains and leaves those registrations.

## Development

The current signed catalog adds `video.minimax_h3.t2v_i2v` as a Comfy
workflow-class market with mirrored MiniMax H3 base parts, a signed
workflow-class definition, active enclave/price/room rows, and native
video+audio caps. `.70` completed admission and paid `/v1/workflows` media
proofs through the `.31` sponsored gateway. The current retained live proof is
`openmayhem-minimax-h3-t2v-paid-proof-v0.2.127.mp4`, session
`02e4b50f367548e156d1ca47975c1e2d213138bf4c1110311f87417af9173e58`, BLAKE3
`f258297d464d7ee060ed9f49f38008b3880379c3cf29adb922769c8c577b7ffc`; media
is `896x512`, `124` frames at `24` fps with AAC stereo. The stronger
dialogue-oriented retained artifact is
`openmayhem-minimax-h3-anime-dialogue-fight-paid-v0.2.123.mp4`, session
`286e0bf679ae7dc44c34f4979a4b53affd6c209a8ecfe774eef9380dffada1b8`, BLAKE3
`431caae7624d36a15917a1f7ee60fdd47118b35c5e025f2efc4c41aca372aae1`. These
proofs must not be replaced by a direct Comfy run.

MiniMax H3 REF2VA/reference-media video (`video.minimax_h3.r2v`) is
product-accepted against parts-index v13. Its policy requires request-carried
reference media through bounded `/v1/workflows` `input_files`; retained proof
artifact: `openmayhem-minimax-h3-r2v-paid-proof-v0.2.126.mp4`.

Krea 2 Turbo base image generation (`image.heavy.le1_2mp`) and Krea 2 Turbo
with signed 4x upscaling (`image.heavy.le17mp`) are product-accepted after
`.42` admission and paid fiat `/v1/workflows` proofs through the `.31`
sponsored gateway. Retained review artifacts are
`openmayhem-krea-base-mercedes-salespitch-paid-v0.2.123.png` and
`openmayhem-krea-4x-mercedes-salespitch-paid-v0.2.123.png`.

The `0.2.127` source release adds standalone convolutional 4x upscaling
(`upscale.conv.le24mp`) as a signed Comfy workflow row. It uses the SPANx4 part
`d871ba305a9cbe521c3da166f06d84b80db02a36a1b4e89720d6bddf54965e0a`,
embeds the outcome-class definition in the catalog, and carries a request-bound
image `input_files` canary. Local reference admission passed with retained
artifact `openmayhem-upscale-conv-le24mp-reference-v0.2.127.png`. The `0.2.128`
source release fixes Comfy workflow input-file capacity accounting so one
request-bound input plus one output artifact does not hide an otherwise eligible
workflow route during dispatch. Paid fiat proof then passed through the `.31`
proof gateway against the `.42` provider with retained artifact
`openmayhem-upscale-conv-le24mp-paid-v0.2.128.png`, session
`04a42dc2aa5159d317be3c0e80924421d1163021e91d8b41352d72ba96b940fe`,
usage `1` `megapixel`, and artifact BLAKE3
`1d47264316354f7f0aa787da4637711e38a0b8e78a715682cb241a937e8f9699`.

The `0.2.123` source release adds metadata-only Comfy workflow-class policy
calibration, signs workflow-class modality fingerprints for Krea base,
Krea+4x, and LTX A/V rows, and keeps backend media quality proof as a separate
paid-route acceptance gate. Krea base and Krea+4x later passed paid fiat
acceptance proofs on 2026-08-11.

The `0.2.122` source release corrects the MiniMax H3 Comfy parts manifest to use
Hugging Face blob/LFS SHA-256 values instead of xet/cache `ETag` values. The H3
part IDs in the Comfy policy-to-parts matrix are therefore regenerated from
verified payload hashes; the smallest H3 base part was mirrored and SHA-checked
through the intended parts path on the mirror host.

The `0.2.121` source release makes Comfy parts YAML imports preserve explicit
Hugging Face revisions when deriving `file_path` origins, so new workflow parts
can be mirrored from immutable `/resolve/<40-hex>/...` sources instead of mutable
`main`. It also records the validated MiniMax H3 source manifest in the Comfy
cheatsheet; H3 still requires parts mirroring, workflow policy publication, and
a paid `/v1/workflows` quality proof before public serving.

The `0.2.135` source release adds LongCat Avatar 1.5 as the active
`video.lipsync` dev row with signed v17 parts, request-carried image/audio
`input_files`, and a workflow-class `video_av_fingerprint` canary from retained
reference media. It also mounts Comfy model files from the signed part record
path rather than the policy display name, so display labels such as UMT5 do not
break provider startup, and keeps Comfy part record path derivation available in
default source builds. Krea base, Krea+4x, LTX A/V, MiniMax H3, and standalone
upscale have paid-route acceptance evidence. LongCat paid fiat `/v1/workflows`
proof passed through the `.31` proof gateway against the `.42` provider with
retained artifact `openmayhem-longcat-paid-anime-fight-v0.2.135.mp4`, session
`33fa30144eeb90c5086716231cffa9c6a0ad49e722b3d064b336760e2febae22`, and BLAKE3
`acafe1209d34b02e2a97f045f507aed762a69fa71dc807521f7829ba10e9a2d3`; owner
quality review for speech intelligibility and sync is still required before
public product acceptance.

The `0.2.136` source release materializes the optional MiniMax H3 Spectrum
workflow lane (`video.minimax_h3.spectrum`) as a signed dev policy with a
workflow-class canary proof. Catalog verification passes at hash
`debf0574baf90c286e132a518986c99ddf2789890cd7b8edb7262c54694fc34a` with
canary set `canary-minimax-h3-spectrum-workflow-launch-v1` and a 49-case
workflow endpoint matrix. A paid fiat `/v1/workflows` route proof passed
through the `.31` sponsored gateway on 2026-08-12 with retained artifact
`openmayhem-minimax-h3-spectrum-paid-v0.2.136.mp4`, session
`c51f1fe861e46a6ebe680f1c57670f2982f81a782db7a9ce93c384ad78af7e6f`, and
BLAKE3 `25b0a156e76ad22653cee1037f6bc0fb64955e668bc5b3ecc3af39a4915ed8f2`.
It is not live product capacity until owner review confirms a measurable
speed/quality benefit over the accepted base H3 lane.

The `0.2.137` source release materializes the SeedVR2 diffusion
upscale/restore lane (`upscale.diffusion`) as a signed dev policy using the
official Comfy-Org SeedVR2 3B int8 convrot model plus SeedVR2 VAE. Parts index
v19 adds the int8 part and verifies at root
`6b75023e49e1d0954ae4f76004f1cb1b620f773ed700d2cb0e55280d0a52a2a0`.
Catalog verification passes at hash
`f1f8d1755311fd18a3f8334bbe09ffcaf0f19a6a929959f130831c5c430eb926`.
`.70` completed signed-inventory admission with inventory root
`f501d4d7340fe2d891560aef0192adb28c0bd8e91c77b36e4ccc0e16b33fd15b`,
reference graph SHA-256
`2f8f41767f90fd18a7a7109b156ef2464f9c22648ce37ada3126e02e76fc2c93`,
output SHA-256
`671c24b35e99ee28edbf08b3c88c37472fbb552dfd52764f9f2d6f422149e328`,
and retained artifact
`openmayhem-seedvr2-upscale-diffusion-reference-v0.2.136.png`. It is not live
product capacity until a paid `/v1/workflows` route proof passes.

The `0.2.118` source release documents the current Comfy parts inventory,
binds workflow providers to the signed outcome-class definition instead of the
local ComfyUI runtime directory, canonicalizes integer-valued workflow JSON
floats before graph hashing, and derives Comfy upscaler output dimensions from
the signed upscaler part scale. A 1024x1024 Krea graph followed by a signed 4x
upscaler is therefore routed and billed as a 4096x4096 workflow, not as the
base image; the Krea+4x workflow policy uses a 900s request timeout for the
combined generation plus upscaler canary. It also carries bounded workflow `input_files` for image/audio/video
source media, which lipsync and talking-video policies use through the normal
paid `/v1/workflows` path, and validates muxed A/V workflow canaries whose audio
track is embedded in the returned MP4. The dedicated Comfy reference is
**[COMFY-CHEATSHEET.md](COMFY-CHEATSHEET.md)**.

The `0.2.104` source release makes local builds self-consistent across macOS,
Linux, and Windows. Source installers select exactly one available llama.cpp
backend and verify the installed result; managed Python runtimes bootstrap from
a version- and hash-pinned standalone `uv` executable without relying on system
`venv`, `ensurepip`, or `pip`. Supervised gateway and provider children now
wait for their configured loopback dependencies without consuming restart or
crash-loop budgets.

The `0.2.57` source release makes live route reporting use each text endpoint's
signed default output-token allowance. Small-context providers therefore remain
visible when they can serve the signed default; request-specific dispatch still
enforces the caller's actual context requirement.

The `0.2.56` source release makes functional startup probes honor each signed
endpoint's output-token range. A model whose endpoint minimum exceeds the
ordinary four-token health probe now receives the smallest signed-valid probe
instead of being rejected before serving.

The `0.2.55` source release fixes co-resident provider memory admission:
in-flight model loads remain serialized, while memory already resident in a
host or unified-memory pool is not subtracted twice from the operating
system's available-memory measurement. Dedicated-VRAM claims remain reserved.

The `0.2.54` source release adds `Cactus-Compute/needle`, a deterministic
tools-only 30.4M model pinned at
`5f89b4307696d669c3df1d38ae057e6e1728b107`, with its `needle-hf` runtime
source pinned at `ffd0d081401257fee31150d30c494b2f98910fc0`. It accepts 1 to
10 tools and has a 1,024-token combined context with a 512-token decoder
ceiling. Needle has exactly two canonical markets: `needle-cpu` on Linux,
Windows x86_64, and Apple Silicon macOS, and `needle-gpu` for CUDA on supported
Linux aarch64/x86_64 and Windows x86_64 hosts with NVIDIA driver r580 or newer.
Needle GPU means CUDA, not Metal/MPS. Apple MPS measured
about 2.5 decode tok/s cold and 10.7 warm, so it is intentionally ineligible.
The focused M5 CPU proof measured 3,087 prefill / 309 decode tok/s for one
tool and 8,964 prefill / 329 decode tok/s for two parallel tools. Windows CPU
measured 2,675 prefill / 126 decode tok/s for one tool and 4,933 prefill /
159 decode tok/s for two parallel tools. Prior GB10 measurements were about
89.7 cold / 166 warm decode tok/s on CUDA and 64-78 decode tok/s on CPU.
The immutable ten-model catalog, four Needle T1/T2 CPU/CUDA markets, and a
permanent CUDA provider are live. A paid TAP request returned the correct
parallel-safe tool call with nonzero usage and exactly one final signed
receipt/ACK.

The release also adds a forward-reader gate for future catalog engines while carrying
the current PAYOUTFREE, ATTAUTO, FLOWRATE,
LIVEROUTE, autonomous bounded TPM route activation after gateway restarts,
consistent exact-boundary vLLM shared-memory admission,
bounded Z-Image readiness, process-lifetime dashboard auth,
non-destructive authenticated direct-to-relay session handover, and the
generalized joint audio/video serving path used by Sulphur. The releases page and live signed catalog remain
the user-facing truth for the current software revision and available models.
It also keeps provider join/start context commitments on one hardware-fit path and pins every
byte-hashed Chatterbox runtime input to LF so Windows source builds are independent of
`core.autocrlf`. Video duration requests resolve deterministically to the greatest
signed legal frame count inside the requested duration, and providers enforce that
exact frame/duration pair before signing usage.
Needle's canonical catalog entry requires Mayhem `0.2.54`; older installations
must update before they can discover or serve it.

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
