# Mayhem

Mayhem is a peer-to-peer OpenRouter built on Trac Intercom. The repo currently contains the Intercom scaffold, a Rust workspace, and local development tooling while the roadmap in `docs/` is being implemented.

## Provider Quickstart

```bash
./install.sh
mayhem setup --role provider
mayhem provider start --enclave <admin-enclave-id> --rooms auto --serve-sessions
mayhem test --sync-models
```

Providers can only opt into admin-created enclave and room records from the contract ledger. They cannot submit arbitrary models, create canonical rooms, or set pricing.

## User Quickstart

```bash
./install.sh
mayhem setup --role user
# terminal 1: leave the gateway running
mayhem use --sc-bridge-url "$MAYHEM_SC_BRIDGE_URL" --sc-bridge-token "$MAYHEM_SC_BRIDGE_TOKEN"
# terminal 2
mayhem models
mayhem balance
mayhem test --sync-models
opencode run --model mayhem/<model-id> "Say hello from Mayhem."
```

`mayhem use` reads canonical contract state, requires an admin-created active
enclave, current `mu_usd` price, open room, active provider route, and local
SC-Bridge credentials for direct provider sessions. For isolated API
development only, use `mayhem use --dev-embedded-catalog`.

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

Both installers print a copy/paste PATH command even when they update your shell profile. They also install a pinned, checksum-verified opencode binary unless `--skip-opencode` is passed or `opencode` is already on PATH. Browser-opening commands, such as hosted payment checkout, also print the copy/paste URL before attempting to open a browser.

## Development

```bash
scripts/dev-net.sh --cleanup
cargo build --workspace
MAYHEM_RUN_INTERCOM_TESTS=1 cargo test -p mayhem-bridge --test sc_bridge -- --nocapture
```

Standalone `mayhem-gateway` is a raw development smoke binary and must be
started with `--dev-embedded-catalog`; production/user flows should use
`mayhem use`.

See `docs/PLAN-2026-07-02-p2p-openrouter-on-intercom.md` and `docs/TRACKER.md` for the implementation roadmap and live execution state. Operator, provider, and user docs live in `docs/operator-runbook.md`, `docs/provider-guide.md`, and `docs/user-guide.md`. Beta launch and metrics docs live in `docs/beta-launch.md` and `docs/beta-metrics.md`. v2 groundwork lives in `docs/v2/README.md`.
