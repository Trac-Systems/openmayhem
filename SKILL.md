# SKILL.md — Agent guide for installing & operating OpenMayhem

**You are a coding agent driving OpenMayhem for a human.** Agent-driven install and operation is the
PRIMARY path — most users and providers never type a command themselves. This file exists so you do
not have to reverse-engineer the app: follow it and you save the user tokens and time, and even a
small local-weight model can run the flows correctly. Be **precise and brief** in what you print
back — relay exactly the values the commands emit (addresses, links, amounts), not prose.

---

## 1. What OpenMayhem is

A peer-to-peer AI-inference marketplace. Two roles:
- **User** — runs a local OpenAI-compatible gateway on `127.0.0.1:11435` and buys inference at a
  live market price. Point any OpenAI client at it.
- **Provider** — serves models from their own machine (gaming PC, Mac, homelab, datacenter) and
  earns per token.

The CLI binary is `mayhem`. The network runs on Trac. Prices float on an automated per-epoch market.
Pay by card (Stripe/fiat), or on-chain via **TAP** (Ethereum ERC-20) or **TNK** (Trac native). The
gateway is loopback-only; a public ledger records prices, receipts, settlements.

Before selecting or serving a model, read the canonical
**[`MODEL-CHEATSHEET.md`](MODEL-CHEATSHEET.md)**. It records every current exact model
selector, signed artifact, endpoint and modality surface, supported backend/platform pair, runtime
pin, resource floor, measured guidance, unsupported combination, and provider start command. Live
enclave IDs, rooms, routes, and prices must still come from `mayhem models --gateway`.
Before declaring a new model or workflow done, read
**[`CALIBRATION.md`](CALIBRATION.md)** and verify the relevant proof gates.
For ComfyUI workflows, also read **[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md)** before
calibrating, admitting, serving, or using a workflow class. It lists every current outcome class,
the signed parts index, required part sets, and `/v1/workflows` request shape.
Publicly needed workflow behavior must live in `README.md`, `SKILL.md`,
`MODEL-CHEATSHEET.md`, or `COMFY-CHEATSHEET.md` so users and providers can
follow it directly from the GitHub repository.
Every Comfy calibration must enumerate every file the graph actually loads in the signed workflow
policy `parts` list. If the graph uses a checkpoint, text encoder, VAE, LoRA, ControlNet,
upscaler, lipsync model, or helper model that is not in the signed parts index, mirror/sign that
file first; do not prove or serve a workflow with manual out-of-policy downloads. Accepted workflow
canary rows must carry the complete signed-policy request shape, including `input_files` for media
loaders; do not widen a whitelist or use placeholder graphs to make calibration pass.
Custom-node archives may contain harmless tar PAX metadata, but must still be rootless and free of
dotfiles, symlinks, device files, parent traversal, and absolute paths. If a candidate archive needs
repacking, document the exact source revision and cleaned payload hash before treating it as a
signed part.

ComfyUI workflows are not a second app. They use the same gateway, provider, route, voucher,
receipt, and settlement machinery, with the Mayhem-native endpoint `/v1/workflows` and endpoint
family `mayhem_comfy_workflows`. A user submits a bounded `workflow` graph; Mayhem derives the
required parts, graph hash, runtime id, output class, and usage from the signed workflow policy.
Providers choose and verify their own parts from the anchored parts index; a user request must never
trigger a provider download.

For long-running Comfy jobs, tell users and client implementers to send
`Prefer: respond-async` plus a stable `Idempotency-Key` on `POST /v1/workflows`.
The gateway returns `202 Accepted`, `x-mayhem-job-id`, and `/v1/jobs/<id>`; the
client can disconnect and later poll `/v1/jobs/<id>`, `/v1/jobs/<id>/result`,
and `/v1/jobs/<id>/artifacts/<artifact_id>`. Without `Prefer: respond-async`, a
plain synchronous workflow request waits for delivery and a premature client
disconnect is cancellation, not a background render promise.

When one host will serve multiple models, start their provider workers **serially**. Wait for each
worker to finish artifact verification, model load, functional canary, and fresh-heartbeat
stabilization before admitting and starting the next worker. Concurrent startup can overlap
transient load headroom and cause an OOM even when the models fit together at steady state. After
each worker settles, rerun `mayhem doctor` and `mayhem provider health --json`; never bypass an
aggregate admission refusal or compensate with an undocumented memory override.

---

## 2. PRIME DIRECTIVES (read before doing anything)

1. **NEVER modify the base protocol to make something work.** Do not edit, bump, swap, or
   re-vendor the pinned Trac/Intercom dependencies or `intercom/trac/*`
   (`trac-msb`/`trac-peer`/`trac-wallet`). Mayhem-owned contract, feature, SC-Bridge, and RPC
   specialization under `intercom/` may evolve through an authenticated Mayhem release, but it must
   not change Autobase writer rules or the upstream peer protocol. If a real bug requires changing
   that pinned core, STOP and tell the human; do not work around it.
2. **Relay LIVE values, never hardcode.** Funding addresses, deposit amounts, Stripe links, model
   IDs, and prices come from the command OUTPUT and the live catalog. Run the command and relay what
   it prints. Do not invent or memorize an address or URL. `mainnet` is the fail-closed default;
   users/providers never paste network or contract addresses into setup.
3. **Ask before assuming.** Do not pick a payment rail, a model, or provider limits for the user.
   Ask the required questions (below), then execute. One round of questions, then act.
4. **Explain each command in one line before running it**, then run it. Keep output tight.
5. **Loopback only.** The gateway and dashboards bind `127.0.0.1`. Never expose them publicly.
6. **On macOS "Malicious Script Blocked":** it is a known false positive from macOS's anti-scam
   feature reacting to a command handed over by a chat app. Tell the user to run the installer in a
   Terminal they opened themselves and update macOS/XProtect (`softwareupdate --background`). The
   installer is clean and installs only under `$HOME`.
7. **Preserve release and store identity.** Never run a dirty or mixed-revision release, add a
   migration/compatibility shim, change the sole writer, or delete/rebuild/convert/promote a
   canonical indexer store. Authenticated Intercom must run from the physical packaged tree; a
   link-only Pear launch is not a release.

---

## 3. Install

### 3.1 Get the code — exact source release (MANDATORY rule)
- `v0.2.166` is source-only. Never invent or offer a native archive URL: the release has no unsigned
  OpenMayhem executable assets. Clone the exact release tag and build it locally.

  **macOS/Linux:**
  ```bash
  git clone https://github.com/Trac-Systems/openmayhem.git
  cd openmayhem
  git checkout --detach v0.2.166
  ./install.sh --from-source
  ```

  **Windows PowerShell:**
  ```powershell
  git clone https://github.com/Trac-Systems/openmayhem.git
  Set-Location openmayhem
  git checkout --detach v0.2.166
  .\install.ps1 -FromSource
  ```
On updated source checkouts, `mayhem up` verifies and, when needed, deterministically repairs only
generated Intercom dependency topology before Pear starts. Never delete `~/.mayhem`, wallets, sparse
stores, provider identity, registrations, or payment state to address a dependency-topology failure.
Everything installs under `~/.mayhem/` — no `sudo`, no system directories.
For an update, check out the exact new source tag and rerun the complete installer. Never copy or
replace only `mayhem`, `mayhemd`, or another individual executable; a mixed binary set is invalid.

### 3.2 Prerequisites (install these FIRST and verify each)
**From source, every OS:** Rust stable (rustup), Node.js 20+ with npm, git, curl, unzip.

**macOS (Apple Silicon):**
```
xcode-select --install        # clang, libclang, git
brew install cmake node
```
Metal ships with the OS. **Pitfall:** if `brew`/`node`/`cmake` are missing, install them before
`install.sh`. **Pitfall:** the "Malicious Script Blocked" dialog (§2.6) — run in a self-opened
Terminal.

**Linux (Debian/Ubuntu):**
```
sudo apt-get install -y build-essential clang libclang-dev cmake pkg-config git curl unzip
```
**Pitfall (most common build failure):** missing `libclang` — verify `ldconfig -p | grep libclang`.
**GPU serving:** NVIDIA driver for CUDA 12 (550+) + CUDA toolkit (`nvcc`); verify `nvidia-smi` AND
`nvcc --version`. `nvidia-smi` proving the driver does NOT prove the toolkit. The source installer
executes the toolkit probe, selects exactly one llama.cpp backend, and verifies the installed
binary before succeeding. Managed Python backends need no system Python, `venv`, `ensurepip`, or
`pip`: Mayhem downloads the exact hash-pinned standalone `uv` for the host and atomically creates
the frozen runtime under `~/.mayhem`.
**ComfyUI workflows:** verify the blessed ComfyUI runtime checkout, currently
`comfyui-v0.30.1` for Krea/LTX/MiniMax H3/upscale/MultiTalk `video.lipsync` lanes,
`comfyui-longcat-avatar-v0.30.1` only for the older LongCat `video.lipsync` proof lane, or
`comfyui-2a68ce33b4c9` only for the rejected InfiniteTalk reference lane. Use
the Python executable that belongs to that runtime; use `python3` only when it
is the runtime environment's interpreter, otherwise set `MAYHEM_COMFYUI_PYTHON`
to the exact executable.
Workflow parts still come only from the signed parts index and require `mayhem provider parts pull`,
`mayhem provider parts add`, and `mayhem provider parts admit --write` before serving. Workflow
provider start must pass `--artifact <comfy-runtime-dir>`; that path is the local ComfyUI runtime
checkout, while the ledger artifact is the workflow class definition. On CUDA hosts set
`MAYHEM_COMFYUI_DEVICE=cuda` when CUDA is required; omitted means runtime auto-selection, while
`MAYHEM_COMFYUI_DEVICE=cpu` is only for intentional CPU proofs. A Comfy
custom node is a signed `custom-node` part, not a model-file workaround: package it as a rootless
`tar.gz` with `__init__.py` at archive root, set `adapter.comfy_custom_node_dir`, and require it in
the workflow policy so Mayhem can extract and whitelist only that node package. A Comfy
provider home has one advertised inventory root; use separate provider homes for disjoint workflow
inventories unless the signed policies require the same root. Krea base generation is a
1024x1024 workflow lane. A Krea+4x lane is one workflow graph with a signed upscaler part, but it
requires the v0.2.107 upscaler-scale derivation on both gateway and provider so vouchers bill and
route the 4096x4096 output instead of the base image.
Standalone upscaling is a separate workflow market: `upscale.conv.le24mp` uses
only the signed `4x-spanx4-ch48.safetensors` part, requires an `input_files`
image, and has paid-route acceptance evidence.
MiniMax H3 2x video upscale is different: it is an optional SeedVR2 branch
inside `video.minimax_h3.t2v_i2v`, requires the six-part inventory root
`1165f3bb28092852c60cdc61d524bb280a45eca39017147484adf9e1d9816ec6`, and needs
v0.2.144+ so `ResizeImageMaskNode` final dimensions are billed correctly.
MiniMax H3 REF2VA/reference-media requests are a separate signed market:
`video.minimax_h3.r2v`. Do not treat a live `video.minimax_h3.t2v_i2v` route as
R2V capacity. If the request carries reference media through `input_files`, the
client must target `video.minimax_h3.r2v` and the operator must have that R2V
provider route live.
Low-VRAM MiniMax H3 is a separate policy family on `comfyui-v0.32.0`. For the
T2V/I2V lane, only steps `4`, `6`, and `8` are permitted and `4` is the
default; do not reuse the existing 20-step H3 graph. The low-VRAM R2V lane is
separate and 4-step only. Do not advertise either low-VRAM lane from an older
release/catalog or without matching provider admission and paid `/v1/workflows`
proof. The R2V lane itself is supported; the public route is the part that must
exist before promising availability.
On Linux containers and VMs, validate sandbox support with the real
`mayhem-enclave sandbox-run` smoke, not with `unshare` alone: read-only writes
and outbound TCP must be denied. Disabled user namespaces do not disqualify a
host when Landlock/seccomp passes.

**Tier 2:** install `tpm2-tools`; the provider uses `/dev/tpmrm0` unprivileged. If the distro owns
that device as `root:tss`, add the login to the existing group with
`sudo usermod -aG tss "$USER"`, then start a new login. Mayhem never creates users/groups or changes
device ACLs.

**Windows 11+:**
- Visual Studio Build Tools with "Desktop development with C++" (MSVC + Windows SDK)
- LLVM for libclang: `winget install LLVM.LLVM` (set `LIBCLANG_PATH` if the build can't find it)
- CMake: `winget install Kitware.CMake`; Rust via rustup (MSVC); Node 20+: `winget install OpenJS.NodeJS.LTS`
- Run `install.ps1` from PowerShell. The engine runs sandboxed (AppContainer).
- **Tier 2:** install .NET SDK 6+ (`winget install Microsoft.DotNet.SDK.8`). The TPM helper uses
  Windows TBS plus PCP/NCrypt as the normal provider user; do not elevate `mayhem` or create a
  service, setup account/group, EK cache, or TPM policy exception.
- **Pitfall (git line endings):** if consent/rules hashing fails, git converted `RULES.md` to CRLF.
  Restore it from the exact repo bytes (`git checkout -- RULES.md`) before continuing.
- **Pitfall (slower per-turn admission):** Windows loopback/IPC + sandbox add per-turn overhead;
  expected, not a fault.

Rule of thumb: **llama.cpp/GGUF models need nothing beyond the base prerequisites on any OS** — that
is the zero-extra path. GPU backends (vLLM/TensorRT/MLX) and audio/image engines need the extras
above only when the chosen model uses them.

### 3.3 Verify the install
```
mayhem --version
mayhem up --yes            # starts supervised peer + gateway; health-checks each
curl http://127.0.0.1:11435/v1/models
```
`mayhem up` does not report ready until the signed rules, payment directory, catalog, and at least
one live model route has synced (fail-closed on mainnet). Ordinary `mayhem down` drains provider
workers, leaves their canonical registrations, and stops everything. Use `mayhem down --restart`
for an update or temporary restart; it preserves durable registrations so the next `mayhem up`
resumes without provider re-onboarding.

For a Tier-2 candidate, a fresh buyer needs no TPM, provider quote helper, root/admin action,
manual verifier flag, or machine-wide setup. The source installer builds the verifier from the same
checked-out tag, and `mayhem up` authenticates the active policy and shared public collateral. It
never fetches verifier code from an operator or provider. Managed Tier-3 verification is enabled
only by authenticated admin policy; a route that cannot prove an available higher tier falls back
to the next tier it can prove.

---

## 4. THE GUIDANCE PROTOCOL — how to drive EVERY request

**General shape for any user request (apply this to all surfaces, not just install):**
1. **Identify the intent** (buy inference? provide compute? fund? switch rail? check status? wire a
   coding agent? troubleshoot?).
2. **Ask only the questions that flow requires** (below) — one round, concrete choices.
3. **Explain then execute** each command (one line each).
4. **Relay the precise success output** the commands emit — funding addresses/amounts, Stripe links,
   model IDs, dashboard URL, endpoint. Never pad with prose. Never fabricate a value.
5. **State what's next** in one line (e.g. "credit will land after payment; then run a completion").

### 4.0 Public error-code handling

When a gateway/API/job request fails, read the structured public fields before
guessing. Normal responses use:

```json
{"error":{"code":"<stable-code>","category":"<class>","retryable":true}}
```

Async artifact and workflow jobs expose the same classification at
`GET /v1/jobs/<id>` as `error_info`. Relay `code`, `category`, `retryable`, and
any `safe_detail`. Do not ask the user to infer from generic prose.
The exhaustive public code list is in `README.md`; the table below is the
operator triage subset that most often needs action.

Important codes:

| Code | What to do |
|---|---|
| `request_exceeds_provider_capacity` | The request is too large for the signed provider envelope. Reduce media bytes, pixels, seconds, frames, steps, or context, or choose a larger workflow/model market. Do not retry unchanged. |
| `provider_admission_no_capacity` | Provider lane is busy/draining or did not accept before the wait deadline. For workflows use `Prefer: respond-async`, a stable `Idempotency-Key`, and a body `timeout_ms` appropriate for the render. Retry later or choose another live route. |
| `required_modality_unavailable` | The model family may be visible, but the exact modality market is not live. For H3, `video.minimax_h3.t2v_i2v`, `video.minimax_h3.r2v`, and low-VRAM variants are separate markets. |
| `payment_rail_not_supported_by_provider` | The buyer rail and provider accepted rails do not overlap. Switch rail or choose a route that accepts it. |
| `insufficient_balance` | Fund the selected buyer rail and confirm `mayhem balance` before retrying. |
| `payment_reservation_failed` | No spend should occur without a receipt. Retry after the route or settlement path is healthy; report if repeated. |
| `provider_transport_closed` / `provider_response_timeout` | Provider became unreachable or exceeded the session deadline. Retry, or check provider health if operating that machine. |
| `provider_response_invalid` / `provider_model_output_invalid` | The provider/model returned a result that did not satisfy the endpoint contract. For strict JSON/tool mode, simplify the request or choose a stronger compatible model. |
| `provider_verification_failed` | Treat as non-retryable for that route until the provider/operator fixes the signed data, canary, attestation, or receipt path. |
| `client_receive_rate_exceeded` | The client/proxy did not drain the provider stream fast enough. Prefer async jobs for long artifact/workflow work or fix the client transport before retrying. |

Public error fields are intentionally sanitized. Never request or reveal private
rooms, peer tokens, wallet secrets, store paths, provider stderr, or raw graph
internals while troubleshooting.

### 4.1 User — buy inference
**Ask first:**
- **Which payment rail?** `fiat` (card via Stripe) · `tap` (Ethereum ERC-20) · `tnk` (Trac native).
- **Which model?** (or "recommend one" — then run `mayhem models --gateway` and pick a live one).

**Execute:**
```
mayhem up --rail <fiat|tap|tnk> --yes      # rail persists across restarts
mayhem models --gateway                    # live truth: models + live prices
```
**Fund (relay whatever the command prints — do NOT hardcode):**
- **fiat:** run the Stripe purchase flow → it prints a **copy/paste Stripe checkout URL** (and opens
  a browser if it can). **Relay the URL verbatim**; tell the user to pay it; credit lands via the
  webhook. Print any Stripe-related detail the command emits (session, amount, currency, status).
- **tap:** run the TAP deposit flow → it prints the **Ethereum deposit address, the ERC-20 token,
  the chain id, and the amount**. Relay those exactly; the user sends that asset to that address.
- **tnk:** run the TNK deposit flow → it prints the **Trac treasury address and amount**. Relay
  exactly.
**Then:**
```
mayhem balance            # confirm credit landed
```
Point any OpenAI client at `http://127.0.0.1:11435/v1`, or wire opencode (§4.3). Show the exact
tokenized dashboard URLs printed by the command. The canonical default shapes are
`http://127.0.0.1:11435/mayhem/dashboard?token=<generated-token>` and
`http://127.0.0.1:11435/mayhem/dashboard/provider?token=<generated-token>`; the bare paths
intentionally return `401`. For a remote terminal, print
`ssh -N -L 11435:127.0.0.1:11435 user@remote-host` alongside the URLs. A tunnel does not replace
the dashboard token, and an operational service on another port must not be presented as the
canonical dashboard.

**To switch rail later:** `mayhem down --restart` then `mayhem up --rail <other> --yes`. Restart
mode preserves provider registrations if this stack also serves. Rails never convert into each
other.

### 4.2 Provider — earn on this machine
**Ask first:**
- **Which payment rails to ACCEPT?** (any subset of `fiat,tap,tnk`; default at registration is
  `fiat` — a provider that wants on-chain MUST set rails or it silently rejects those users).
- **Which payout destination for each accepted rail?** TAP/TNK may use the provider wallet by
  default or a separate wallet that co-signs the binding. Fiat uses hosted Stripe Connect; ask for
  the provider's actual two-letter country code and always relay the printed copy/paste URL.
- **Min-ask floor?** (the lowest price the provider will serve at; "market" = accept the protocol
  price).
- **Self-protection limits?** (`--max-concurrent`, `--accept-rate`, `--budget` as USD/day|month|total).
- **Which model?** (run `mayhem doctor` first — it reports which backends/models fit this hardware
  and expected speed; do NOT start a provider whose backend the doctor marks insufficient).
- **Hugging Face token set?** Model downloads come from Hugging Face. Anonymous downloads are
  rate-limited: multi-gigabyte pulls can crawl or fail outright. Recommend a free token to every
  provider. How to get one: create a free account at https://huggingface.co/join, open
  https://huggingface.co/settings/tokens, choose "Create new token" with the **Read** role, copy
  the `hf_...` value. Then `export HF_TOKEN=hf_...` before `mayhem up --provider`, or pass
  `--hf-token-file <path>`. Read-only scope; costs nothing; never commit it anywhere.

**Execute (in order):**
```
mayhem doctor                                  # what fits + expected tok/s + memory
mayhem up --provider --yes                      # start serving the first feasible enclave
mayhem provider rails set --rails <fiat,tap,tnk> --submit
mayhem provider payout set --rail tap --submit  # when TAP is accepted; local wallet by default
mayhem provider payout set --rail tnk --submit  # when TNK is accepted; local wallet by default
mayhem provider min-ask set <...>               # participation floor (per market)
mayhem provider limits set [--max-concurrent N] [--accept-rate R] [--budget <USD/day|month|total>]
mayhem provider health                          # green AND the model appears in /v1/models
```

Accepted rails are not enough for buyer routing. A provider can be active,
room-joined, and heartbeating while staying invisible to buyers if it lacks an
active verified payout binding for an accepted rail. In that case
`mayhem provider health --json` reports
`gateway.route_preconditions.status = no_active_verified_payout_binding`; run
`mayhem provider payout get`, then bind TAP/TNK or complete Stripe readiness.

For fiat, run `mayhem provider stripe onboard` with the provider's actual
two-letter country code. It always prints the hosted `connect.stripe.com` URL
before attempting a browser open; use `--no-open` on a remote terminal and then
run `mayhem provider stripe status`. Readiness automatically creates the
provider-signed fiat binding. For a ready Stripe account owned by another
provider identity, inspect `mayhem provider stripe relink --help`: both provider
identities must consent, and an account id alone is rejected.
If an existing Standard Stripe account has never been connected to Mayhem, use
`mayhem provider stripe adopt --country <CC>`. It always prints the Stripe OAuth
URL, accepts `--no-open`, and obtains the account identity only from Stripe's
callback; never ask the operator or provider to paste an `acct_...` value.
For a German provider, `mayhem provider stripe rotate --country DE` replaces
the current Connect account; use the actual country code the provider answered
with. It signs the current account as a compare-and-swap, always prints the new
hosted URL, and keeps provider/account-specific readiness for the current
account until the verified replacement's exact activation epoch. If `E` is the
latest fully applied epoch, a first binding activates at `E+1` and a rotation
at `E+2`; existing liabilities never move to the new revision.

TAP/TNK `payout set` signs with both the provider wallet and the selected target
wallet. Use `--target-wallet-key-file` only when the target differs from the
provider wallet; deliberate reuse by several provider identities requires that
same target wallet to co-sign every binding. Use `payout rotate` for a later
change and `payout get` to inspect current/pending revisions. Payout bindings
follow the provider identity across all models and rooms. The local peer remains
read-only: it relays the signed intent, and the sole indexer verifies and
appends it automatically without Mayhem-admin approval or SSH.

After each finalized epoch, the operator payout worker automatically and
idempotently reconciles Stripe transfers, TNK transfers, and TAP root/fee/burn
work from the canonical epoch/apply hash. Providers do not run admin settlement
commands. Stripe and TNK are pushed; TAP remains a non-custodial cumulative
claim, so use `mayhem withdraw` (visible alias: `mayhem claim`) when the provider
chooses to sweep it.

#### 4.2.1 Canonical model provider matrix

Use the exact catalog `model_id` as `--provider-enclave`; do not normalize case or substitute an
upstream repository name. `mayhem doctor --provider-backend <backend>` preflights that backend and
its managed runtime only. The exact model/artifact/enclave fit is decided by the subsequent
`mayhem up` command against the admin-published catalog and ledger.

| Exact selector | Doctor backend | Canonical artifact | Supported execution | Admission / operational minimum | Gateway endpoints |
|---|---|---|---|---|---|
| `Qwen/Qwen3.8-27B` | `vllm` | `nvfp4` / vLLM safetensors | Linux NVIDIA Blackwell; compute capability >= 12.0; signed independent dispatch is text-only | 48 GiB RAM, 24 GiB NVIDIA dedicated or unified memory, AVX2 or NEON; operational Mayhem minimum 0.2.161 | `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/hf-inference/models/<model-id>` |
| `hauhaucs/qwen3.6-35b-a3b-uncensored` | `vllm` | `nvfp4` / vLLM safetensors | Current documented path is Linux NVIDIA; CLI rejects Windows; artifact requires compute capability >= 12.0 | 48 GiB RAM, 24 GiB NVIDIA dedicated or unified memory, AVX2 or NEON | `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/hf-inference/models/<model-id>` |
| `google/gemma-4-E4B-it` | `llama.cpp` | `gguf-q4_k_m` + mandatory BF16 projector | Linux/Windows/macOS CPU; CUDA, Metal, or Vulkan when the installed Mayhem build has that feature | 12 GiB RAM, 8 GiB VRAM for full offload, AVX2 or NEON | `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/hf-inference/models/<model-id>` |
| `tongyi/z-image-turbo` | `stable-diffusion.cpp` | `gguf-q4_k` + text encoder + VAE | Linux/Windows/macOS CPU fallback; CUDA, Metal, ROCm, or Vulkan selected from hwprobe when the matching `sd-cli`/`sd-server` build is installed | 16 GiB RAM, 8 GiB VRAM for full offload; no catalog CPU-flag floor | `/v1/images/generations`, `/hf-inference/models/<model-id>` |
| `nvidia/parakeet-tdt-0.6b-v3` | `transformers-asr` | `safetensors` + processor/tokenizer | Linux, Windows, or macOS CPU; CUDA on Linux/Windows; Metal/MPS on macOS | 8 GiB RAM, 4 GiB VRAM for full offload, AVX2 or NEON | `/v1/audio/transcriptions`, `/hf-inference/models/<model-id>` |
| `acestep/ace-step-1.5` | `ace-step` | `safetensors` composite | Linux x86_64/ARM64, Windows x86_64, or macOS x86_64/ARM64 CPU; CUDA on Linux/Windows; Metal/MPS on Apple Silicon | 16 GiB RAM, 20 GiB VRAM for full offload, AVX2 or NEON | `/v1/music/generations`, `/v1/audio/generations`, `/hf-inference/models/<model-id>` |
| `prism-ml/Ternary-Bonsai-27B` | `llama.cpp` or `mlx` | `gguf-q2_0` or `mlx-2bit`, each with its signed vision projector | GGUF on Linux/Windows/macOS with CPU or a compiled accelerator; MLX on Apple Silicon | 16 GiB RAM, 8 GiB VRAM for full offload, AVX2 or NEON | `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/hf-inference/models/<model-id>` |
| `SulphurAI/Sulphur-2-base` | `sulphur` | `gguf-q4_k_m` CUDA composition or `mlx-q4` composition, with all signed A/V sidecars | CUDA on Linux/Windows; Metal/MPS on Apple Silicon | 64 GiB RAM; one generation in flight | `/v1/videos`, `/hf-inference/models/<model-id>` |
| `ResembleAI/chatterbox` | `chatterbox` | original-English PyTorch safetensors plus four mandatory signed sidecars | Linux/Windows/macOS CPU; CUDA on Linux/Windows; Metal/MPS on Apple Silicon | 8 GiB RAM, 6 GiB VRAM for full offload | `/v1/audio/speech`, `/hf-inference/models/<model-id>` |
| `huihui-ai/Huihui-Agents-A1-abliterated` | `llama.cpp` | `gguf-q4_k` + mandatory BF16 vision projector | Linux/Windows/macOS CPU; CUDA, Metal, or Vulkan when the installed build has that feature | 32 GiB RAM, 32 GiB VRAM for full offload, AVX2 or NEON | `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/hf-inference/models/<model-id>` |
| `Cactus-Compute/needle` | `needle-cpu` or `needle-gpu` | Pinned 30.4M Needle model plus pinned `needle-hf` runtime | CPU on Linux, Windows x86_64, and Apple Silicon macOS; GPU is CUDA-only on Linux aarch64/x86_64 and Windows x86_64; no Metal/MPS market | 1,024 combined context, 512 decoder tokens, 1-10 tools | `/v1/chat/completions`, `/v1/responses` |

For every published model, the managed sequence is:
```
mayhem models --gateway
mayhem doctor --provider-backend <exact-backend>
mayhem up --provider --provider-enclave <exact-model-id> --yes
mayhem provider health --json
mayhem models --gateway
```
The start command is the model-artifact installer: it resolves an active admin-created enclave,
downloads the immutable primary artifact and every signed sidecar, verifies size/hash/Merkle
bindings, seals the artifact, loads the canonical backend, runs signed functional modality prompts,
and only then joins canonical rooms and emits heartbeats. Never use a development catalog, a local
artifact override, an upstream clone, manually selected weights/projectors/encoders, or a hand-built
Python environment for provider service. `HF_TOKEN`/`--hf-token-file` changes download
authentication only.

`mayhem doctor` succeeds for a requested backend when it is not `Insufficient` and its runtime
preflight completes; the non-JSON success line is `Provider backend preflight: <backend> ready`.
It does **not** prove that this model's catalog floor, artifact, active enclave, room, or current
price exists. `mayhem up` is the authoritative model check. On success its provider worker reports
`self_test.ok=true`, functional `modality_health` rows with `ok=true`, and
`Provider start complete: heartbeats flowing.` Final health requires `ok=true`, at least one active
serve, `heartbeat.live=true`, `gateway.ok=true`, `gateway.route_count>0`, and this model/provider in
`/v1/models`.

**Qwen 3.8 27B NVFP4**
- **Install/start:** require Mayhem `0.2.161` or newer, run
  `mayhem doctor --provider-backend vllm`, then
  `mayhem up --provider --provider-enclave Qwen/Qwen3.8-27B --yes`.
- **Exact pins:** canonical model
  `Qwen/Qwen3.8-27B@1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0`; approved NVFP4
  source
  `HivenetQuant/Qwen3.8-27B-NVFP4@cd5a8f0739c1df89d8cd9d39ede58c619d8298c2`;
  canonical byte mirror
  `TracNetwork/mayhem-catalog-Qwen-Qwen3-8-27B-NVFP4@4ee2ca6a5987ba7f07cbe0e779f73f98d66b4a94`;
  artifact root
  `36abadf4a7aa1ac3b60abc57bda718c3329cf3ad69fb9dfca13a5384c32c6f11`.
  Use only its complete hash-verified safetensors payload. Do not use YaRN,
  MLX, GGUF, another quant, or a local checkpoint.
- **Surface:** vLLM `0.24.0`, BF16 compute, artifact-scoped FP8 KV, native
  262,144-token ceiling, text output, text/image/video input, JSON, tools,
  streaming, thinking controls, and low/medium/xhigh reasoning. Required
  context brackets through the ceiling are `le8k`, `le32k`, `le128k`, and
  `le256k`; use live ledger prices, never the catalog reference rate as proof
  that a bracket is active.
- **Measured `.29` evidence:** two text-only requests overlapped at full
  262,144-token context with scheduler capacity `2` and
  `max_num_batched_tokens=2048`. Proof SHA-256 is
  `3fbab1e8f6fed5d8b5e393e958edebef20544346d07ae66a0d68a7c8e59114fd`.
  A 1-megapixel image and 16-frame video calibration used a 15% (`17.35 GiB`)
  unified-memory reserve and `98.32 GiB` F13 budget; process-tree RSS was about
  `5.435 GiB`, excluding accelerator allocations. A warm release `0.2.162`
  paid stream measured 12,017 prompt tokens at 1,543.46 tok/s end to end
  (1,626.19 tok/s after response headers) and 32 completion tokens at
  2.18 tok/s. EngineCore used 46,303 MiB of accelerator memory while about
  64.67 GiB system memory remained available; the provider stayed at zero
  restarts and returned a final FIAT receipt. Treat these as `.29` request-path
  evidence, not universal requirements or pure kernel throughput. Separate
  paid launch proof used two overlapping full-context FIAT requests with
  independent final signed receipts, plus cancellation isolation.
- **Concurrency:** `independent_dispatch` is a signed, artifact-root-bound
  opt-in and currently authorizes only the exact text-only modality set.
  Image/video requests remain exclusive, and artifacts without a matching
  profile remain serial. Mayhem derives the provider's context-dependent
  capacity from hwprobe, local/operator session limits, usable memory after
  reserves and claims, per-session KV memory, any signed scheduler ceiling,
  and vLLM's runtime KV capacity, then reports it in heartbeats. The `.29`
  result `2` is not a hardcoded model limit or a value to copy elsewhere.
- **Status:** `v0.2.164` is the completed fleet checkpoint, not the minimum
  client/provider version. At `2026-09-03 00:58 CEST`, the designated `.29`
  provider was observed on that exact release with one live route, 262,144
  served context, and `fiat,tap,tnk`. Overlapping FIAT requests produced
  independent final signed receipts and website billing completed. Run a
  fresh `/v1/models` check before making any future liveness claim; this proof
  does not replace bindings and paid checks when another provider joins.

**Qwen 3.6 35B-A3B uncensored**
- **Install/start:** `mayhem doctor --provider-backend vllm`, then
  `mayhem up --provider --provider-enclave hauhaucs/qwen3.6-35b-a3b-uncensored --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-hauhaucs-qwen3-6-35b-a3b-uncensored-NVFP4@58722d97ba2d93c32740f409efc9155b784edb95`;
  `model.safetensors` is 23,354,242,416 bytes, plus exactly eight signed chat-template,
  config/generation/processor/preprocessor/recipe/tokenizer sidecars. Total catalog payload is about
  21.77 GiB. The only canonical backend/artifact pair is vLLM/`nvfp4`.
- **Compute:** CPU-only, Apple Metal, AMD, pre-Blackwell NVIDIA, and Windows are not eligible for
  this artifact. In addition to the catalog floors in the matrix, vLLM preflight requires a CUDA
  toolkit containing `bin/nvcc`; no CPU fallback or alternate quant is cataloged.
- **Endpoints/controls:** text output with text/image/video input, 262,144-token catalog context,
  JSON and tools. Defaults are temperature `1.0`, top-p `0.95`, top-k `20`, min-p `0`, and
  presence penalty `1.5`. `thinking_mode` defaults to `enabled`; `thinking_history` defaults to
  `latest_only`. The provider may advertise only modalities and speciality levels present in the
  signed adapter.
- **Evidence:** `canary-launch-v2`, `token_fingerprint`, `match_min=0.9`; canonical `nvfp4`
  fingerprint `e8e920fbe8d2a63fc4742a7fb49eef7ba250c34aa2f91180e465bf567a6e14f3`.
  Startup functional cases must cover text, image, and video and return non-empty text/tool output;
  periodic auditor probes perform the fingerprint comparison.
- **Stop on:** `no NVIDIA GPU detected`, Windows vLLM rejection, compute capability below `12.0`,
  less than 48 GiB RAM or 24 GiB NVIDIA memory, or the exact CUDA-toolkit preflight error. Relay the
  reason; do not choose another Qwen checkpoint, quant, runtime, or local weights.

**Gemma 4 E4B IT**
- **Install/start:** `mayhem doctor --provider-backend llama.cpp`, then
  `mayhem up --provider --provider-enclave google/gemma-4-E4B-it --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-google-gemma-4-E4B-it-GGUF@68772908c9431af9c9bfc3cee0ebefcd74995891`;
  `gemma-4-E4B-it-Q4_K_M.gguf` is 5,335,289,664 bytes and the exact mandatory sidecar is
  `mmproj-gemma-4-E4B-it-BF16.gguf` at 991,551,840 bytes. Never pair the GGUF with another
  projector. Total payload is about 5.89 GiB.
- **Compute:** 8 GiB VRAM is the catalog full-offload target, not a CPU admission minimum.
  llama.cpp may run CPU-only or partially offloaded after the 12 GiB RAM and AVX2-or-NEON gates;
  an accelerator verdict also requires the matching CUDA, Metal, or Vulkan feature in this Mayhem
  build.
- **Endpoints/controls:** text output with signed text/image/audio/video input, 131,072-token catalog
  context, JSON and tools. Defaults are temperature `1.0`, top-p `0.95`, top-k `64`;
  `thinking_mode` defaults to `disabled`. `visual_token_budget` is exactly
  `budget_70|budget_140|budget_280|budget_560|budget_1120`, default `budget_280`.
- **Evidence:** `canary-gemma4-launch-v1`, `token_fingerprint`, `match_min=0.9`,
  `tolerance_bps=0`; canonical `gguf-q4_k_m` fingerprint
  `99e2ae42e6d36c8ccbbee9ed12cafc84628490163b9a1e1d53854f971abe2a48`.
  Startup must functionally cover text, image, audio, and video.
- **Stop on:** `needs at least 12 GiB RAM`, `missing required CPU feature expression avx2|neon`,
  a build-feature preflight error, missing/mismatched `mmproj`, or a canary failure. A CPU-only
  doctor verdict is valid; an artifact/projector substitution is not.

**Z-Image Turbo**
- **Install/start:** `mayhem doctor --provider-backend stable-diffusion.cpp`, then
  `mayhem up --provider --provider-enclave tongyi/z-image-turbo --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-tongyi-z-image-turbo-GGUF@b0110258385798d6e5b9bea626f6560607ce17ad`;
  `z_image_turbo-Q4_K.gguf` is 3,864,250,304 bytes, with mandatory
  `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` text encoder and `ae.safetensors` VAE sidecars. Total payload
  is about 6.24 GiB. Signed backend semantics require a separate diffusion model and map public
  steps by `-1` and guidance by `+1`; providers do not alter those offsets.
- **Compute:** 8 GiB VRAM is the full-offload target; the catalog also permits a 16 GiB RAM CPU
  path. hwprobe chooses CUDA, Metal, ROCm, Vulkan, or CPU, but runtime preflight still requires the
  documented external `sd-cli` and sibling `sd-server`; Mayhem manages the model files, not those
  executables.
- **Endpoints/controls:** prompt up to 32,000 characters; width/height `576..2048`, each divisible
  by 16, default `1024x1024`; steps `7..9`, default `9`; guidance `0..49`, default `0`; shift
  `1..10`, default `3`; `n=1..4`, seed `0..4294967295`, and optional negative prompt. OpenAI output
  is `b64_json`.
- **Evidence:** `canary-z-image-launch-v1`, `seed_perceptual_hash`, `match_min=0.9`,
  `tolerance_bps=1500`; prompt `z-image-red-cube-studio-seed7` expects pHash
  `a0000303c3c73f07`. Startup must return an `image/*` artifact.
- **Stop on:** the exact preflight:
  ```text
  stable-diffusion.cpp requires `sd-cli` on PATH or an explicit MAYHEM_STABLE_DIFFUSION_CPP_BIN path before `mayhem up --provider`
  ```
  Also stop on a missing sibling server, less than 16 GiB RAM, missing sidecars, invalid signed
  offsets, or no image canary artifact. Report the prerequisite; do not download an arbitrary
  engine binary or swap the text encoder/VAE.

**Parakeet TDT 0.6B v3**
- **Install/start:** `mayhem doctor --provider-backend transformers-asr`, then
  `mayhem up --provider --provider-enclave nvidia/parakeet-tdt-0.6b-v3 --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-nvidia-parakeet-tdt-0-6b-v3-Transformers@a83f71f1a8a1cf099b5dbe23262c5028ad931086`;
  byte-identical `model.safetensors` is 2,508,311,120 bytes, with exactly five signed config,
  generation-config, processor, and tokenizer sidecars. The portable canonical backend is
  `transformers-asr`; the upstream NeMo format is not a provider alternative.
- **Compute:** CUDA uses Linux/Windows and at least 4 GiB usable device memory; Apple Metal/MPS uses
  macOS unified memory; otherwise Linux/Windows/macOS CPU is supported. The 4 GiB value is the
  full-offload target while the 8 GiB RAM and AVX2-or-NEON catalog gates still apply to every path.
  Concurrency is one transcription.
- **Endpoints/controls:** arbitrary bounded 16 kHz mono WAV/FLAC, automatic recognition across 25
  languages, model punctuation/capitalization, overlapping long-audio chunking, and word/segment
  timestamps. OpenAI `response_format` is `json|text|srt|verbose_json|vtt`;
  `timestamp_granularities` is `word` and/or `segment`. HF `return_timestamps` is
  `false|true|word|segment`. Forced language, prompt, sampling, and streaming controls are not
  supported; do not advertise them.
- **Evidence:** `canary-stt-launch-v2`, `transcript_match`, `match_min=1`; exact transcript evidence
  covers `stt-en-punctuation-auto-language`, `stt-de-auto-language`, and
  `stt-long-overlap-chunking`. Startup must return non-empty transcript text; auditor health requires
  exact normalized transcript matches.
- **Stop on:** less than 8 GiB RAM, missing `avx2|neon`, an unsupported OS, managed
  `transformers-asr` bootstrap/import failure, malformed WAV/FLAC, or transcript canary failure.
  Let Mayhem repair/recreate its exact pinned managed runtime on retry; do not `pip install`, clone
  NeMo, or substitute weights.

**ACE-Step 1.5**
- **Authority/status:** `acestep/ace-step-1.5` is live in the signed catalog with T1/T2 markets.
  The exact artifact and endpoint surface passed Windows CUDA, Linux CUDA, and Apple M5 Max MPS
  calibration.
- **Install/start:** run `mayhem doctor --provider-backend ace-step`, then
  `mayhem up --provider --provider-enclave acestep/ace-step-1.5 --yes`.
- **Artifact shape:** admin mirror
  `TracNetwork/mayhem-catalog-ACE-Step-Ace-Step1-5-SFT@f41443d7171a03181ada08912780b0449e8ff7fe`;
  SFT DiT `model.safetensors` is 4,787,825,604 bytes plus exactly 25 signed sidecars: measured DiT
  config/modeling/APG code and silence latent, pinned Qwen3-Embedding-0.6B files, pinned
  `acestep-5Hz-lm-1.7B` files, and pinned VAE config/weights. Total payload is about 9.40 GiB.
  The measured ACE-Step v0.1.8 source is embedded in the Mayhem engine and runs in the enclave
  sandbox; providers never enable arbitrary `trust_remote_code`.
- **Compute:** the catalog floor is 16 GiB RAM, 20 GiB VRAM for full offload, and AVX2 or
  NEON. Current doctor logic supports CPU on the matrix platforms, CUDA full offload at >=20 GiB,
  CUDA CPU/INT8 partial offload at >=4 GiB, and Apple Silicon Metal/MPS with >=16 GiB available
  unified memory. The worker rechecks free memory at load; do not promise throughput from these
  thresholds. Managed ACE runtime bootstrap also requires 24 GiB free disk.
- **Endpoints/controls:** full music endpoint supports `text2music|cover|cover-nofsq|repaint`,
  prompt/caption (composed maximum 512 characters), lyrics (maximum 4096), style/genre/tags, source
  and reference audio, duration auto/`-1` or `10..600` seconds, steps `1..200` default `50`,
  guidance `1..15` default `7`, `n=1..8` default `2`, seed `-1..4294967295`, thinking, BPM
  `30..300`, key/time signature, ODE/SDE, Euler/Heun, cover/repaint controls, and
  `flac|opus|aac|wav|wav32|mp3`. The simpler audio/HF endpoints expose prompt, duration
  `10..600`, guidance, seed, and their narrower signed response shape.
- **Evidence:** `canary-music-launch-v1`, `audio_fingerprint`,
  `match_min=0.9`, `tolerance_bps=1500` (signed floor 8500), using
  `ace-step-text2music-seed7`. Windows/Linux similarity is 9918 bps; two fresh M5 MPS runs are
  byte-identical and score 8730 against Linux; unrelated audio scores 1666. All three endpoint
  families and 1,208 cases pass. Until ledger publication, the correct outcome is an absent gateway
  model or no active admin-created priced enclave. Never clone ACE-Step, manually install its
  runtime, publish local output, or substitute any component.

**Chatterbox original-English TTS**
- **Install/start:** `mayhem doctor --provider-backend chatterbox`, then
  `mayhem up --provider --provider-enclave ResembleAI/chatterbox --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-ResembleAI-chatterbox-PyTorch@0adbad4d3515285bdcdc3d503759e7110e664201`;
  the primary 2,129,653,744-byte model and exact `ve.safetensors`, `s3gen.safetensors`,
  `tokenizer.json`, and `conds.pt` sidecars are mandatory. Do not substitute the multilingual or
  turbo checkpoints.
- **Compute:** 8 GiB RAM; 6 GiB is the full-offload target. The managed backend selects CUDA,
  Apple MPS, or CPU from the actual host and keeps one synthesis in flight.
- **Endpoints/controls:** OpenAI `/v1/audio/speech` and HF text-to-speech. OpenAI requires
  `model`, `input`, and `voice`; use `voice: "default"` for the model voice. Zero-shot cloning
  passes a bounded base64 WAV as `reference_audio: {"data":"...","encoding":"base64",
  "content_type":"audio/wav"}`. Supported controls are `exaggeration`, `cfg_weight`,
  `temperature`, `min_p`, `top_p`, `repetition_penalty`, and `seed`. Voice cloning replaces the
  need for a built-in voice library; never invent a catalog voice name.
- **Evidence:** `canary-chatterbox-launch-v1`, `audio_fingerprint`, M5/MPS and Windows/CUDA
  endpoint matrices, ordinary TTS, and zero-shot clone proofs.

**Huihui Agents A1 abliterated**
- **Install/start:** `mayhem doctor --provider-backend llama.cpp`, then
  `mayhem up --provider --provider-enclave huihui-ai/Huihui-Agents-A1-abliterated --yes`.
- **Artifact:** admin mirror
  `TracNetwork/mayhem-catalog-huihui-ai-Huihui-Agents-A1-abliterated-GGUF@59d0dfbbdb07138fb53fc8672cd04261efa3065e`;
  `Agents-A1-abliterated-Q4_K.gguf` is 21,166,757,536 bytes and the exact mandatory
  `mmproj-model-bf16.gguf` is 902,821,824 bytes.
- **Compute:** 32 GiB RAM and AVX2 or NEON; 32 GiB VRAM is the full-offload target. llama.cpp
  may run CPU-only or use an accelerator compiled into this Mayhem build.
- **Endpoints/controls:** text output with text, image, and video input; 262,144-token context;
  OpenAI chat/completions/responses and HF multimodal chat; JSON; automatic, required, and
  parallel tool calls. Defaults are temperature `0.85`, top-p `0.95`, top-k `20`, min-p `0`,
  repeat penalty `1`, and presence penalty `1.1`. `thinking_mode` supports exactly `enabled`
  and `disabled`.
- **Evidence:** `canary-a1-launch-v1`, cross-platform token fingerprints from M5 Metal and
  Linux CUDA, all 604 endpoint rows per platform, image/video understanding, and two tool calls
  retained in one turn.

**Cactus Compute Needle**
- **Install/start:** choose exactly one canonical market, run
  `mayhem doctor --provider-backend needle-cpu` or
  `mayhem doctor --provider-backend needle-gpu`, then run
  `mayhem up --provider --provider-enclave Cactus-Compute/needle --yes`.
- **Pins:** model `Cactus-Compute/needle@5f89b4307696d669c3df1d38ae057e6e1728b107`;
  runtime source `Cactus-Compute/needle-hf@ffd0d081401257fee31150d30c494b2f98910fc0`.
  Both are immutable release inputs.
- **Surface:** deterministic tools-only inference for 1 to 10 tools through OpenAI chat
  completions and responses. The combined context is 1,024 tokens and the decoder ceiling is
  512 tokens. Do not advertise ordinary prose generation or a larger context.
- **Markets:** `needle-cpu` supports Linux, Windows x86_64, and Apple Silicon macOS.
  `needle-gpu` is CUDA-only on Linux aarch64/x86_64 and Windows x86_64 hosts and
  requires NVIDIA driver r580 or newer for its frozen CUDA 13 runtime. There are exactly
  two markets; Apple Metal/MPS is not a third market and must not be mapped to `needle-gpu`.
- **Measured throughput:** Apple MPS was about 2.5 decode tok/s cold and 10.7 warm,
  so MPS is intentionally ineligible. M5 CPU measured 3,087 prefill / 309 decode
  tok/s for one tool and 8,964 prefill / 329 decode tok/s for two parallel tools.
  Windows CPU measured 2,675 prefill / 126 decode tok/s for one tool and 4,933
  prefill / 159 decode tok/s for two parallel tools. Prior GB10 measurements were
  about 89.7 decode tok/s cold and 166 warm on CUDA, and 64-78 on CPU.

For every text-generation model, catalog `ctx_max` is a ceiling, not a required provider setting.
A provider may commit a smaller fitting value with `--ctx`; Mayhem carries that exact value through
the heartbeat, route, voucher, receipt, and dashboard. Before an LLM market opens, the admin must
publish every applicable context-price bracket: `le8k` always, then `le32k`, `le128k`, `le256k`,
and `gt256k` as the catalog ceiling crosses each preceding boundary. Do not tell a provider to
inherit the ceiling price for a smaller context, and do not claim a lower context requires a new
enclave or admin approval. `provider join` and `provider start` use the same signed-catalog and
hardware-fit context selection; an explicit `--ctx` is preserved exactly or rejected before
download/load. If this provider previously committed a different context, it can sign its own
`provider leave` and rejoin without an admin transaction.

For any model, preserve and relay the exact `mayhem up` rejection bullets. In particular,
`ledger artifact binding does not match signed catalog artifact`, `no local-compatible artifact`,
`functional modality canary <id> failed/returned invalid output`, and
`provider engine became unhealthy during functional modality self-test` are hard stops. A degraded
health report or missing route is not green: re-run that model's exact doctor command and the same
managed start, fix only documented environment prerequisites, and never bypass verification or
change the admin artifact.

An artifact-binding rejection is not a propagation delay and must not trigger a new wallet,
provider identity, enclave, or catalog edit. First verify that the command and running stack are the
same current release (`mayhem --version` and the checked-out tag), stop the old stack with
`mayhem down --restart`, rebuild/install the exact tag, start it again, and retry with that exact
binary plus `--verbose`. Preserve `~/.mayhem`; do not reset its wallet or sparse store merely to
repair a stale executable. If the exact current binary still rejects the binding, relay the full
unabridged `--verbose` output together with the version and enclave id. The operator then compares
the immutable ledger enclave tuple with the ledger-pinned signed catalog; providers never work
around a genuine mismatch locally.

For Comfy workflow providers, do not serve a workflow class until the parts inventory and admission
proof both pass. Keep **[`COMFY-CHEATSHEET.md`](COMFY-CHEATSHEET.md)** open while doing this.
Pull only from a verified parts-index layout:

```bash
mayhem provider parts pull --layout-dir <parts-index-layout> --part-id <part-id> --require-payload
mayhem provider parts add --layout-dir <parts-index-layout> --part-id <part-id>
```

Use `--all` only when the machine is intended to mirror the whole provider-facing parts index and
has enough disk. For each class, run admission against the blessed runtime and reference graph:

```bash
mayhem provider parts admit \
  --outcome-class <workflow-class> \
  --runtime-id comfyui-v0.30.1 \
  --part-id <part-id> \
  --usable-bytes <size> \
  --working-set-bytes <size> \
  --reference-graph <path.json> \
  --reference-runtime <comfy-runtime-dir> \
  --write
mayhem up --yes
mayhem provider serve add <workflow-enclave-id> --artifact <comfy-runtime-dir> --workflow-class-definition <definition.json> --json
```

If the class uses staged load/unload, pass the approved `--load-plan`. Without it, Mayhem assumes
all required parts must fit together. A workflow heartbeat without a matching saved admission is a
bug or stale worker; do not paper over it with a manual heartbeat, local route edit, memory
override, or catalog change. Omit `--workflow-class-definition` only when the signed catalog embeds
`workflow.outcome_class_definition`; workflow graph hashing treats integer-valued JSON floats and
integers as equivalent.

For explicit Tier 2, pass `--provider-hardware-quote-kind tpm2-quote-ek` and the platform helper
to `mayhem up --provider --yes`: `scripts/hardware/mayhem-tpm2-quote-linux.sh` on Linux or
`scripts/hardware/mayhem-tpm2-quote-windows.ps1` on Windows. Both run under the provider account;
a valid proof automatically joins the existing admin-created Tier-2 market.
**Relay:** the provider dashboard URL, health status, and (later) `mayhem provider earnings`.
**Pitfall:** green health with the route missing from `/v1/models` = the model failed to load —
re-run `mayhem doctor` and check the backend extras (§3.2). **Claiming earnings:** TAP payouts are
claimed non-custodially with `mayhem withdraw` (`mayhem claim` is its visible alias); TNK gas is
sponsored; explain earnings depend on uptime, price, saturation, and reputation. Use
`mayhem provider drain` to stop taking new sessions without a hard stop.

### 4.3 Wire a coding agent (opencode)
```
mayhem opencode                                 # registers a `mayhem` provider in opencode.json, fills models from /v1/models
opencode run --model mayhem/<model-id> "Say hello from OpenMayhem."
```
Re-run `mayhem opencode` after the catalog changes to re-sync. It leaves other providers untouched.
Client sampling (temperature, top_p, seed) is set in the client (e.g. opencode.json `options`) and
forwarded as-is.

### 4.4 Ongoing surfaces (same protocol: ask if needed → execute → relay)
- **Spend controls:** `mayhem config max-price <value|--clear>` (a per-token price CEILING — note it
  bounds the RATE, not the token volume; a bigger/longer request still costs more).
- **Preferred providers (per model):** `mayhem config` preferred-provider `add|remove|list` — pins
  routing to a chosen provider set for privacy; still respects price/ctx/tier gates.
- **Inspect:** `mayhem status`, `mayhem balance`, `mayhem models [--gateway]`, `mayhem price show`,
  `mayhem history` / `mayhem sessions`, `mayhem payments`.
- **Route truth:** in `/v1/models`, `registered_route_count` is durable registration evidence;
  `route_count`, `providers_online`, `availability=routable`, and `route_candidates` are fresh
  dispatch-eligible capacity. Crashes and `mayhem down --restart` age out and resume from durable
  registration without re-onboarding. Deliberate remove, enclave-changing switch, drain-to-stop,
  provider stop, and ordinary `mayhem down` submit leave records.
- **Provider ops:** `mayhem provider health|earnings|list|drain|stop`, `mayhem earnings`,
  `mayhem payouts`, `mayhem reputation`, `mayhem withdraw` (TAP claim).
- **Wallet:** `mayhem wallet show|backup|import|passwd`. Back up the mnemonic on request; never print
  a secret into a shared/logged channel.

---

## 5. Command reference (compact)

| Goal | Command |
|---|---|
| Install release (macOS/Linux) | `git clone …/openmayhem.git && cd openmayhem && git checkout --detach v0.2.166 && ./install.sh --from-source` |
| Install release (PowerShell) | `git clone …/openmayhem.git; Set-Location openmayhem; git checkout --detach v0.2.166; .\install.ps1 -FromSource` |
| Start user gateway | `mayhem up --rail <fiat\|tap\|tnk> --yes` |
| Start provider | `mayhem up --provider --yes` |
| Stop and leave provider registrations | `mayhem down` |
| Temporary restart/update | `mayhem down --restart` |
| Fund (card) | `mayhem pay stripe …` → relay Stripe URL |
| Fund (Ethereum) | `mayhem pay tap …` → relay address+token+chain+amount |
| Fund (Trac) | `mayhem pay tnk …` → relay treasury address+amount |
| Balance | `mayhem balance` |
| Models (live) | `mayhem models --gateway` |
| Price + derivation | `mayhem price show` |
| Max price ceiling | `mayhem config max-price <v\|--clear>` |
| What fits (provider) | `mayhem doctor` |
| Accept rails | `mayhem provider rails set --rails fiat,tap,tnk --submit` |
| Bind TAP payout | `mayhem provider payout set --rail tap --submit` |
| Bind TNK payout | `mayhem provider payout set --rail tnk --submit` |
| Inspect payout bindings | `mayhem provider payout get` |
| Stripe payout setup | `mayhem provider stripe onboard --help` |
| Adopt existing Standard Stripe account | `mayhem provider stripe adopt --help` |
| Stripe account replacement | `mayhem provider stripe rotate --help` |
| Stripe account reuse | `mayhem provider stripe relink --help` |
| Min-ask floor | `mayhem provider min-ask set <…>` |
| Limits | `mayhem provider limits set --max-concurrent N --accept-rate R --budget <USD/day>` |
| Provider health | `mayhem provider health` |
| Earnings / claim | `mayhem provider earnings` · `mayhem withdraw` |
| Wire opencode | `mayhem opencode` |
| Status | `mayhem status` |

---

## 6. Troubleshooting (fix the ENVIRONMENT, never the core)

- **Build fails mid-`cargo`** → missing `libclang` (Linux: `ldconfig -p | grep libclang`) or C/C++
  toolchain. Install the §3.2 prerequisite; do not patch code.
- **Model never loads / provider earns nothing on NVIDIA** → CUDA toolkit (`nvcc`) missing though
  `nvidia-smi` works. Install the toolkit.
- **Model download crawls or fails (429/403 from Hugging Face)** → anonymous rate limit. Set a
  free HF token (§4.2): `export HF_TOKEN=hf_...`, then retry the download.
- **Consent/rules hashing fails on Windows** → CRLF conversion of `RULES.md`; `git checkout -- RULES.md`.
- **"Malicious Script Blocked" on macOS** → false positive; run installer in a self-opened Terminal;
  `softwareupdate --background`.
- **Gateway not ready** → on mainnet `up` waits for signed rules/catalog/payments/route sync; give it
  time; check `mayhem status`. Do not bypass the fail-closed check.
- **Provider can't reach the network / stuck** → connectivity/bootstrap issue; solve it via config
  (retry, direct peers if the CLI offers it), NEVER by changing the pinned Trac core.
- **Buffered media output trips a receive-rate error** → do not add an unbounded limit or blame the
  provider. Current FLOWRATE accepts bounded fragmented/coalesced direct and real official-relay
  frames while sustained floods still fail closed; verify both peers run the same accepted release.
- **Anything that looks like it needs a code/dependency change** → STOP, report to the human. Core is
  canonically pinned and off-limits.

---

## 7. Hard boundaries (never do these)

- Never bump/swap/re-vendor dependencies or edit `intercom/`, `intercom/trac/*`, or any pinned Trac
  core to "make it work."
- Never hardcode a funding address, Stripe URL, or price — always relay live command output.
- Never expose the gateway/dashboards beyond `127.0.0.1`.
- Never choose the user's rail, model, or limits for them — ask.
- Never paste a wallet mnemonic or secret into a shared, logged, or remote channel.
- Never bypass the mainnet fail-closed checks or the signed-catalog verification.
- Never present version metadata or focused local tests as a live release. A source release requires
  one clean tagged commit, fresh source-build acceptance on the six supported targets,
  physical-root Intercom identity verification, canary/live acceptance, and exact-revision fleet
  deployment. The matrix is exactly `aarch64-apple-darwin`,
  `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`,
  `x86_64-unknown-linux-gnu`, `aarch64-pc-windows-msvc`, and
  `x86_64-pc-windows-msvc`. Those builds are internal acceptance evidence, not unsigned release
  assets. Any runtime source change restarts the build matrix.
