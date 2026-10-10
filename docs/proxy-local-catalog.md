# Persistent local test catalog

This opt-in example exists only for local Proxy Models UI/API acceptance. Every
offer is labeled **LOCAL TEST ONLY**. Its admission permit is issued by a private
synthetic test authority; it represents no actual fee payment, model, supplier,
verified capability or live capacity. The real-provider, real-model and actual
fee acceptance requirements remain unfinished. Never point production at it.

The Node child runs a local Corestore/Autobase with the actual Mayhem contract,
publication journal, admission gate, provider signatures and signed canonical
discovery. Only admin/epoch/test-marker genesis records are seeded. Policy,
market, membership and offer records are applied through real publication.
No DHT, MSB client, peer swarm, model backend or financial service starts.

The gateway example opens the existing protected Core catalog stores and runs
the shared catalog refresh supervisor. It deliberately does not run the presence
lifecycle or a buyer runtime. Only authenticated `GET /v1/proxy/offers`, exact
offer detail reads and `GET /v1/proxy/offers/batch?ids=...` (1–16 unique IDs)
are exposed; all other paths/methods return 404. The protected
control file's inert presence address/token is never contacted. Source death or
expired catalog observations fail closed. Freshness constants are unchanged.

From the candidate Core worktree, after review:

```sh
mkdir -m 700 /absolute/private/local-catalog
cargo run -p mayhem-gateway --example proxy_catalog_local -- \
  --local-test --directory /absolute/private/local-catalog \
  --bind 127.0.0.1:11435 --duration-seconds 1800
```

Use a new absolute canonical directory: symlinks, foreign ownership, permissive
modes and incomplete original identities are refused. The example is Unix-only
and requires the checkout's installed Node dependencies. A duration is bounded
to 1–86400 seconds; default 1800. SIGINT/SIGTERM or helper exit stops the runner.
Restart with the **same directory** to retain its test keys, original permit,
offer IDs, token, publication journal and catalog cache. Stop before restarting;
OS locks reject concurrent owners. Never delete individual identity, plan,
Autobase or journal files to repair an error. Preserve the original directory for
inspection. No reset/cleanup switch exists.

`manifest.json` is atomically written mode 0600 under the mode-0700 state
directory. It contains:

- `schema_version: 1`, `kind: "local_test_proxy_catalog"`, `test_only: true`,
  `ready`, `gateway_url`, `token`;
- `gateway_pid`, `catalog_pid`, `state_path`, `network`, `started_at_ms`,
  `expires_at_ms`, the three allowed `routes`;
- `paid_execution: false`, `admission: "synthetic_local_test_permit"`.

Shutdown writes `ready: false` and `stopped_at_ms`. A readiness file alone does
not prove a process is alive; require a successful authenticated directory read.
The token is never printed. Do not copy manifests or private state into git,
browser responses, screenshots or public logs.

The candidate website needs server-only `PROXY_CATALOG_GATEWAY_URL` and
`PROXY_CATALOG_GATEWAY_KEY` from this private manifest, plus
`PROXY_CATALOG_LOCAL_TEST=1` for its explicit local-test banner. A candidate API
uses `PROXY_CATALOG_GATEWAY_RAIL=FIAT` and matching `MAYHEM_GATEWAY_URL` /
`MAYHEM_GATEWAY_TOKEN`. The rail selects a discovery source only; this runner
offers no buyer policy, descriptor, estimates or execution. The older API
checkout on another port is not substituted automatically. Configuring or
restarting either server is a separate reviewed local action.

Focused disposable verification:

```sh
node --test intercom/tests/proxy-catalog-local.test.mjs
cargo test -p mayhem-gateway --example proxy_catalog_local -- --test-threads=1
```

These tests create and remove their own private directories and loopback
listeners. The existing 90-second paid integration lab remains unchanged.
