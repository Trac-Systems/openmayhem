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

---

## 2. PRIME DIRECTIVES (read before doing anything)

1. **NEVER modify the core to make something work.** Do not edit, bump, swap, or re-vendor
   dependencies, and do not patch `intercom/` or the Trac core (`intercom/trac/*`,
   `trac-msb`/`trac-peer`/`trac-wallet`). Those are canonically pinned. If something fails, fix the
   *environment* (missing prerequisite, config, connectivity) — never hack the pinned code. If a
   real bug seems to require a core change, STOP and tell the human; do not work around it.
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

---

## 3. Install

### 3.1 Get the code — release-first, main-tree fallback (MANDATORY rule)
- **If a released version exists**, always install the **latest release artifact**, verified against
  its SHA-256 sidecar (both on the [releases page](https://github.com/Trac-Systems/openmayhem/releases/latest)):
  ```
  ./install.sh --artifact-url <archive-url> --sha256 <archive-sha256>
  ```
- **If there is NO release version yet**, build from the **main tree** by cloning:
  ```
  git clone https://github.com/Trac-Systems/openmayhem.git
  cd openmayhem
  ./install.sh --from-source            # Windows: .\install.ps1 -FromSource
  ```
Check the releases page first. Prefer a release; fall back to main-tree source only when no release
exists. Everything installs under `~/.mayhem/` — no `sudo`, no system directories.

### 3.2 Prerequisites (install these FIRST, verify each — the build fails mid-way otherwise)
**Every OS:** Rust stable (rustup), Node.js 20+ with npm, git, curl, unzip.

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
`nvcc --version`. `nvidia-smi` proving the driver does NOT prove the toolkit — without it the model
silently never loads and the provider earns nothing. **vLLM/TensorRT artifacts:** Python 3.10+ with
`venv`/`pip`.
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
one live model route have synced (fail-closed on mainnet). `mayhem down` stops everything.

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
Point any OpenAI client at `http://127.0.0.1:11435/v1`, or wire opencode (§4.3). Show the dashboard
URL the command printed.

**To switch rail later:** `mayhem down` then `mayhem up --rail <other> --yes`. Rails never convert
into each other.

### 4.2 Provider — earn on this machine
**Ask first:**
- **Which payment rails to ACCEPT?** (any subset of `fiat,tap,tnk`; default at registration is
  `fiat` — a provider that wants on-chain MUST set rails or it silently rejects those users).
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
mayhem provider rails set --rails <fiat,tap,tnk>
mayhem provider min-ask set <...>               # participation floor (per market)
mayhem provider limits set [--max-concurrent N] [--accept-rate R] [--budget <USD/day|month|total>]
mayhem provider health                          # green AND the model appears in /v1/models
```
For explicit Tier 2, pass `--provider-hardware-quote-kind tpm2-quote-ek` and the platform helper
to `mayhem up --provider --yes`: `scripts/hardware/mayhem-tpm2-quote-linux.sh` on Linux or
`scripts/hardware/mayhem-tpm2-quote-windows.ps1` on Windows. Both run under the provider account;
a valid proof automatically joins the existing admin-created Tier-2 market.
**Relay:** the provider dashboard URL, health status, and (later) `mayhem provider earnings`.
**Pitfall:** green health with the route missing from `/v1/models` = the model failed to load —
re-run `mayhem doctor` and check the backend extras (§3.2). **Claiming earnings:** TAP payouts are
claimed non-custodially (`mayhem withdraw`/collect); TNK gas is sponsored; explain earnings depend on
uptime, price, saturation, and reputation. Use `mayhem provider drain` to stop taking new sessions
without a hard stop.

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
- **Provider ops:** `mayhem provider health|earnings|list|drain|stop`, `mayhem earnings`,
  `mayhem payouts`, `mayhem reputation`, `mayhem withdraw` (TAP claim).
- **Wallet:** `mayhem wallet show|backup|import|passwd`. Back up the mnemonic on request; never print
  a secret into a shared/logged channel.

---

## 5. Command reference (compact)

| Goal | Command |
|---|---|
| Install (release) | `./install.sh --artifact-url <url> --sha256 <hash>` |
| Install (no release → main) | `git clone …/openmayhem.git && cd openmayhem && ./install.sh --from-source` |
| Start user gateway | `mayhem up --rail <fiat\|tap\|tnk> --yes` |
| Start provider | `mayhem up --provider --yes` |
| Stop everything | `mayhem down` |
| Fund (card) | `mayhem pay stripe …` → relay Stripe URL |
| Fund (Ethereum) | `mayhem pay tap …` → relay address+token+chain+amount |
| Fund (Trac) | `mayhem pay tnk …` → relay treasury address+amount |
| Balance | `mayhem balance` |
| Models (live) | `mayhem models --gateway` |
| Price + derivation | `mayhem price show` |
| Max price ceiling | `mayhem config max-price <v\|--clear>` |
| What fits (provider) | `mayhem doctor` |
| Accept rails | `mayhem provider rails set --rails fiat,tap,tnk` |
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
