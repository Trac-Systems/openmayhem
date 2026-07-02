# Mayhem

Mayhem is a peer-to-peer OpenRouter built on Trac Intercom. The repo currently contains the Intercom scaffold, a Rust workspace, and local development tooling while the roadmap in `docs/` is being implemented.

## Provider Quickstart

```bash
./install.sh
mayhem setup --role provider
mayhem provider start --model qwen3.5-4b-gguf-dev
```

## User Quickstart

```bash
./install.sh
mayhem setup --role user
mayhem use --model qwen3.5-4b
```

## Development

```bash
scripts/dev-net.sh --cleanup
cargo build --workspace
MAYHEM_RUN_INTERCOM_TESTS=1 cargo test -p mayhem-bridge --test sc_bridge -- --nocapture
```

See `docs/PLAN-2026-07-02-p2p-openrouter-on-intercom.md` and `docs/TRACKER.md` for the implementation roadmap and live execution state.
