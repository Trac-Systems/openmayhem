# Explicit provider startup

The [guided setup workflow](docs/SETUP_WIZARD.md) generates and reviews the first
managed configuration before its explicit Run action. For an existing reviewed
configuration, the Core CLI supports:

```sh
mayhem provider proxy serve --config /absolute/private/provider.json --home /absolute/wallet/home
```

The normal `--keypair`, `--peer-store-name` and `--wallet-password-file` wallet
options remain available. This command uses the existing encrypted Core wallet;
it does not accept a new inline signing key. `provider proxy catalog ...` remains
read-only and keeps its previous command shape. No native provider is started,
stopped, reconfigured or moved by this command.

The authenticated SC-Bridge peer identity must match the proxy controller's
signing wallet. This applies to buyers as well as providers: sharing a bridge
from a differently identified native peer does not delegate that wallet's
authority. A mismatch must be corrected before testing paid requests.

For a separate POSIX peer using an existing encrypted wallet, the Intercom
launcher supports explicit `--peer-wallet-key-file`,
`--peer-wallet-password-file` and `--peer-wallet-public-key` options together.
Use absolute protected file references and the expected public key. The peer
loads that wallet without exporting it into its own store, verifies the identity
before joining either network, and preserves the original encrypted file.
Use a separate peer store and loopback bridge port; never change the key inside
an existing ledger store. These options do not change the default native peer
wallet path and do not currently accept Windows ACLs as POSIX protection.

For automatic restart through the existing local supervisor:

```sh
mayhem provider proxy add --config /absolute/private/provider.json --home /absolute/wallet/home
```

This requires a running mayhemd advertising persistent-child support. An older
daemon is rejected; setup must not silently promise persistence it cannot provide.
Setup validates the protected configuration and normal wallet identity before
installing a child. It does not open provider stores, start a probe, increment a
controller fence, register an offer or pay admission. The response means installed
and starting, not publicly available; normal canonical and health checks still apply.

Each wallet/network gets one local controller name. Aliases belong in that
controller's configuration rather than competing processes. mayhemd stores its
full launch specification transactionally in a separate owner-only child registry;
it restores it after daemon restart and rejects duplicate/static-config collisions.
No credentials are copied into process arguments, status output or the child
environment. `--wallet-password-file` (or the existing home secret file) supplies
encrypted-wallet credentials. Upstream credentials must use protected file
references for supervised startup, checked again on restart; shell-only secrets
remain supported by explicit foreground startup. The configured files must remain
available at their absolute locations. Repeated child crashes use mayhemd's
existing restart/backoff and crash-loop reporting.

An existing empty, missing, corrupt or unprotected child registry fails closed;
it is never silently recreated. Diagnostic status-file failure cannot turn an
already committed installation into a misleading rejection. Authenticated child
removal commits removal before stopping the child; it does not delete proxy
capacity, retained requests or financial recovery data. The local process limit
is 256 persistent children, not a limit on proxy offers or public catalog entries.

## Configuration and initialization

`managed::Config` defines the version-1 JSON configuration. Files must be regular,
owner-only files, not symlinks or hard links. The control document is bounded to
4 MiB; referenced connection documents to 32 KiB; bridge credentials to 8 KiB.
Relative references resolve against the file which owns them. Errors do not echo
credentials, upstream URLs, prompts or private paths. Filesystem protection
must be validated on the target platform. Windows protected-storage integration
and complete native first-install/restart/Run acceptance remain separate release
prerequisites; do not infer support from a successful build.

The explicit configuration contains:

- Network/contract and provider public identity; the trusted local peer RPC;
  authenticated literal-loopback SC-Bridge URL and a token **file reference**.
- A private state directory and the installed `mayhem-proxy-worker` executable.
  Worker startup validates ownership, permissions and an empty private workdir.
- Connection groups, their maximum concurrency, protected connection files and
  cumulative operator probe budgets. Exact duplicate connections cannot become
  separate independent pools. Known shared physical backends have additional
  allocation-group limits; provider-declared independence is not remote attestation.
- Routes, stable route IDs, connection/allocation groups, endpoint adapters,
  registered offer descriptors and the operator-approved settlement policy.
  Conflicting dispatch identities are rejected. Offer rates/revisions are checked
  canonically during negotiation, not accepted as financial authority from this file.
- Pinned local tokenizer files and byte/channel/worker limits for every LLM route.
  Decision routes use decision-schema validation and have no invented native-token
  rate requirement. Aggregate tokenizer artifact and worker allowances are explicit.
- Control-session/per-buyer quotas, bounded journal retention/payload limits,
  financial-read capacity, maintenance page size and scheduling policy.
- Separate decoder pools for customer inference, operator probes, and retained
  financial-result reconciliation. A long-running inference cannot consume all
  decoder permits reserved for recovery. Local parser/control limits are not
  generation deadlines or advertised upstream concurrency.

Duration fields inherited from the internal limit types use JSON objects such as
`{"secs":5,"nanos":0}`; fields explicitly named `_ms` use integer milliseconds.
The guided CLI and dashboard produce this configuration and review it before
Run. The low-level commands remain available for explicit operator configuration.

The loader checks files, identities, scoped groups, offer uniqueness, supported
operations, adapter contracts, health policy and tokenizer pins before opening
stores. The wallet must match the configured public identity before state creation.
Startup never clears old data. A single capacity database owns shared physical and
credential constraints; each stable route has its own recovery journal. Rates and
offer revisions do not rename that journal. Store integrity, ownership or limit
failures stop startup rather than silently rebuilding or discarding retained work.

## Readiness, recovery and shutdown

Every startup begins with unknown live readiness. LLM routes require fresh speed
evidence from the pinned tokenizer and the configured floor (at least 5 tokens/s).
Restarts do not restore old measurements as current. A successful short/buffered
reply may be valid output while still insufficient to qualify generation speed.
The same monitor gates live capacity and supplies local status.

An optional per-route recovery specification contains a fixed request, bounded
output allowance and **probe-only** duration budget. The operator explicitly
enables a monitoring interval and cumulative attempts/cost allowance. Monitoring
rotates through configured routes, probes only when recovery is needed and due,
and shares credential/physical capacity with ordinary inference. Healthy idle
routes do not receive periodic paid work merely because a timer fired. When
evidence expires, this configured monitoring policy may spend another permitted
probe; disabling/exhausting its budget leaves capacity closed until usable fresh
evidence exists. No budget is automatically replenished.

Bridge authentication must succeed before any probe starts. A probe creates no
buyer reservation, receipt, market-demand record or ledger publication. Unknown
execution remains occupied and cannot be resent after timeout or restart, even
if the configured operator budget has remaining attempts. Checked completion or
non-execution is required to release that occupancy.

SIGINT/SIGTERM or loss of the controller closes admission and tells owned sessions
to stop. Started result publication and recovery pages retain their durable
completion rules. Started probes finish or reach their configured probe timeout;
shutdown does not convert uncertain work into free slots. Health uses coalesced
in-memory snapshots; the CLI prints a single final bounded summary, not a growing
per-request or per-tick log.

Startup also publishes signed proxy availability only after current canonical
offer/admission observation. It reports the actual remaining shared capacity,
keeps original health/speed expiry, and attempts withdrawal at shutdown. Its
control reads have separate permits from paid admission/settlement. See
[PRESENCE.md](PRESENCE.md) for timing, authentication, replay protection and
gateway observations.

## Evidence and remaining integration

Tests exercise protected configuration/wallet rejection, real local HTTP stream
bootstrap, decision readiness, restart freshness and cumulative budgets, aliases,
unknown-outcome retention, authenticated bridge startup, and a negotiated paid
decision request through reservation/result/receipt/closure on FIAT, TNK and TAP.
The three-rail tests use the canonical isolated ledger RPC fixture and ephemeral
test wallets; they are **not live payments or real Noise-network acceptance**.

Explicit foreground startup, guided setup and opt-in persistent mayhemd
installation are available. The separate setup workflow handles admission
status and exact reviewed publication; `serve`/`add` do not register a market,
pay the admission fee or establish production activation. A retained Run does
not automatically adopt a later price publication. Native/external shared-runtime
registration and complete real-model/network/rail acceptance remain separate
requirements. Worker/tokenizer containment and full setup/Run must be proved on
each supported OS; native Windows acceptance remains outstanding.
