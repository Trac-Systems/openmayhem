# Local shared provider setup

The CLI wizard and the opt-in gateway dashboard use `setup::Flow` and the same
owner-only `Store`. This is a connected setup surface for the existing four
compatible endpoint profiles and reviewed custom profiles. Compatible profiles
calculate their own recipe, endpoint-contract and metering hashes. They do not
require the operator to hand-author a recipe. Discovery lists upstream model IDs;
it never certifies model identity, concurrency, capabilities or readiness.

The current CLI and dashboard support guided first creation, bounded model
listing, canonical family/market browsing, exact create or join selection,
structural checks, explicit bounded probes, admission invoice/status/checkout,
reviewed publication, and persistent Run/recovery. Each external action is
separate. Saving configuration does not pay, publish, start a model server or
establish live availability. The admission service controls fee collection and
permit issuance; the wizard does not invent a fee or issue a permit itself.

Proxy offers have provider-set prices: every unit rate, request fee, minimum
session amount and accepted rail belongs to the exact signed offer. They do not
use the native lane's market-clearing price. Native model startup, defaults and
attestation remain separate. A provider declaration, model-list response, KYB
identity or published offer does not prove the remote model's identity or privacy.

## Before first setup

Install one complete matching Core build, including `mayhem`, `mayhemd`,
`mayhem-gateway`, `mayhem-proxy-worker` and its authenticated Intercom assets.
The worker must remain beside `mayhem`; copying the CLI alone is insufficient.
Standard recipes and dashboard pages are compiled in. Node.js 20+ is an existing
Core runtime prerequisite; source builds also need the toolchain described in the
[root README](../../../README.md#what-you-need-installed-first). No development
checkout or Node development dependencies are needed by an installed wizard.

Use an existing private Core home, encrypted wallet, configured trusted peer and
SC-Bridge. Persistent Run also needs the existing running mayhemd with persistent
child support. Setup does not create a second wallet, peer or supervisor. Have
these operator inputs ready:

- A working upstream API and permission to use it, with an exact model ID or an
  explicitly requested model-list read. Setup does not install or restart it.
- A protected upstream credential file, or an explicit no-authentication choice.
  The local dashboard can instead accept a write-only bearer value.
- For LLM endpoints, approved protected local `tokenizer.json` data. Its exact
  bytes are imported and pinned; the tokenizer is used for speed measurement,
  not billing or verification of the remote model. Decisions needs no tokenizer.
- Your concurrency, price, rail, settlement and probe allowance choices. A trusted
  admission origin is needed for enrollment; its current response supplies the
  actual fee and collection instructions. Never substitute a buyer deposit.
- An existing protected wallet password file for unattended restarts, if needed.

The examples use `$MAYHEM_HOME` for that existing home and `$TOKENIZER_FILE` and
`$ADMISSION_ORIGIN` for inputs you have selected. Omit the tokenizer option for
Decisions. Add `--api-key-file PATH` for a protected upstream key reference.

```sh
mayhem provider proxy setup init --home "$MAYHEM_HOME" \
  --tokenizer-file "$TOKENIZER_FILE" --admission-origin "$ADMISSION_ORIGIN"
mayhem provider proxy setup wizard --home "$MAYHEM_HOME" \
  --config "$MAYHEM_HOME/proxy-setup/wizard.json" --inspect
mayhem provider proxy setup wizard --home "$MAYHEM_HOME" \
  --config "$MAYHEM_HOME/proxy-setup/wizard.json"
```

`init` opens the wizard after saving the new bundle. Resume its existing
`wizard.json` after interruption; do not initialize a replacement capacity store.
`--inspect` is a local read and performs no upstream request. Use the current
revision returned by each action, rather than assuming how far a lost response
advanced the draft.

The interactive wizard exposes these explicit steps:

| Action | Meaning |
|---|---|
| `c` Connect; `d` Discover; `s` Select/price | Review the saved connection, request bounded listing, or save exact choices. Listing is not a probe. |
| `k` Check; `p` Probe | Check local structure, then separately review and authorize one request within the retained cumulative allowance. |
| `a` Admission facts | Read fresh canonical registration and operation-sequence facts; unavailable is not unpaid. |
| `i` Invoice/create; `t` Status; `f` FIAT checkout | Create/recover the original invoice, reconcile it, or request checkout. The CLI does not send a transfer. |
| `v` Review publication; `u` Publish | Review the exact initial create/join and offers, then explicitly sign/submit it. |
| `e` Review new rates; `z` Publish rate review | Edit USD prices for the existing submarkets, then confirm the retained offer-only plan. Sequence and offer revisions come from fresh canonical reads. |
| `r` Recover probe; `o` Recover publication | Reconcile the original retained work; no new probe or payment identity. |
| `g` Review Run; `b` Begin Run; `h` Reconcile Run | Review the generated controller, explicitly install it, or inspect that original child. |

Payment instructions and a checkout return are not payment confirmation. Status
must reconcile the same invoice, and canonical publication must confirm the exact
operation before Run. An admitted result with a separate financial `review_code`
retains its admission history while exposing the review; do not erase that history
or treat the flag as a new fee or automatic revocation.

## Configuration and identity

`FlowConfig` is an owner-only JSON file (0600, no symlink). Its fields are:

- `schema_version: 1`, `directory`: existing owner-only setup directory (0700).
- `profile`: the full `ProfileInput` accepted by `setup prepare`. Inside that
  object, its own `profile` field is normally
  `{"kind":"standard","endpoint":"openai_chat_completions"}` (or
  `openai_completions`, `openai_responses`, `mayhem_decisions`).
  Network, provider public key, connection reference,
  limits, exact create/join market declaration, capacity group, rails, all prices,
  sequence and settlement policy remain explicit operator choices.
- `probe_plan`: protected existing `ProbePlan` path, or null. It binds the request,
  finite allowance, installed worker and **existing shared** capacity authority.
- `peer_rpc`: configured trusted Core peer RPC base, or null.
- `admission_origin`: trusted admission API **origin**, or null. HTTPS is required
  except explicit literal loopback HTTP fixtures.
- `run`: optional protected runtime handoff settings, absent/null when disabled.
  `{ "template": "/private/runtime-policy.json", "wallet_password_file": null }`.
  The password field is a protected **file reference**, using the existing wallet
  password file; it defaults to `<home>/secrets/wallet-password` when present.
  The template contains `schema_version:1`, existing managed `bridge`, `health`,
  `limits`, optional `tokenizer`, and explicit `allow_recovery_probes` fields.
  No offers, price maps, adapter/recipe snapshots, routes or budgets are entered
  again: they come from the exact published draft and existing probe plan.
- `declaration_registry`: optional trusted registry origin/settings for signed
  data-handling declarations; see [DATA_HANDLING_SETUP.md](DATA_HANDLING_SETUP.md).
- `timeout_ms`: 1 through 10000 for discovery/admission reads. Probe execution
  retains its separately bounded existing plan timeout.

Relative private paths resolve against the protected configuration file. The
browser cannot choose paths, hosts, credentials, a worker or an arbitrary body
to sign. Configuration loading only reads protected metadata; it does not resolve
upstream credentials or contact services. Payout, wallet custody and managed
runtime configuration stay with their existing components. Do not put secrets in
command arguments or profile/recipe JSON.

```sh
mayhem provider proxy setup wizard --config /private/wizard.json
mayhem provider proxy setup wizard --config /private/wizard.json --inspect
mayhem provider proxy setup wizard --config /private/wizard.json --action-file /private/action.json
```

The interactive path prompts for model, new-market display label/slug, context,
concurrency, membership/offer revisions, accepted rails and every signed price
unit. The simple rail editor narrows each offer's existing rails. First-time
create/join selection is guided as described below. Later changes to
protected endpoint/identity configuration use the explicit profile interface;
neither aliases nor discovered labels become canonical proof.
Unattended actions use strict `FlowAction` JSON in an owner-only bounded file.
Each mutation carries the current retained revision; a stale client fails. The
same wallet locator/cache is used by existing CLI commands. Read/check actions do
not unlock a wallet. Enrollment signs only the existing scoped challenges;
publication signs only the exact acknowledged `plan_digest`.

## Guided first setup without configuration JSON

`mayhem provider proxy setup init --home <existing-private-home>` creates the
initial standard-profile bundle under `<home>/proxy-setup`. It reuses the saved
Core peer/bridge, canonical network checks, existing wallet and installed worker.
It does not start/reconfigure Core, the supervisor or a model server. The parent
home must already be owner-only. An existing bundle is always retained: resume
its `wizard.json` rather than initialize another capacity store.

The prompts collect an API base directory URL, protocol endpoint, protected
bearer-key reference (or explicit no authentication), upstream model, served
context, shared concurrency, rails and every exact price unit. Canonical family
and compatible market pages support either creating a new market in an enabled
family or joining an exact existing market. Review reads the provider operation
sequence; final save rechecks it and the selected canonical descriptor. A failed
read never becomes an empty catalog. Creating an already-existing exact descriptor
is refused; select that market through join instead.
Public HTTPS is the restricted network choice. Local/private endpoints require
explicit CIDRs and explicit permission for plaintext HTTP; no scanning or
destination inference occurs. The standard profile does not certify that every
claimed upstream operation or optional capability works.

Use `--api-key-file` for an existing protected key reference. There is no raw-key
argument or echoed key prompt. The shared factory additionally accepts a
nonserializable, zeroizing write-only credential from the local dashboard
form. Its output contains only a protected reference. Credentials never enter
recipes, reviews, ledger declarations or diagnostic errors.

LLM profiles require `--tokenizer-file` or an explicitly selected protected local
`tokenizer.json`. The factory validates and imports a digest-pinned private copy;
it never downloads or guesses tokenizer/model identity. The tokenizer measures
generation speed, not billing. Decisions requires no tokenizer. The guided
probe is a short streaming greeting for LLMs or one typed decisions question.
The operator chooses the cumulative attempt/cost allowance, per-attempt upper
estimate, deadline/output bound and whether targeted recovery may use the same
allowance. Creation sends no probe; the existing wizard separately requests
permission for its exact retained probe plan.

Charging outcomes, checkpoints and hold-expiry behavior require explicit choices;
no financial policy, fee, receiver or FX rate is invented. `--admission-origin`
selects an explicitly trusted admission service; omission leaves enrollment
unavailable. `--restart-password-file` retains an existing protected wallet
password reference (otherwise the existing `<home>/secrets/wallet-password` is
used when available). No new wallet or payout identity is created.

The `bounded_single_connection_v1` resource preset generates runtime policy:
1 MiB request/4 MiB response bounds, 16 choices/64 tools/questions/options,
bounded decoder buffers, one route with the chosen concurrency, a 60-second
health evidence lifetime, existing refresh scheduling and the existing minimum
5 native tokens/second for LLM health. These are local resource/monitoring
settings, not measured upstream capacity or inference-duration limits. The
operator explicitly chooses local completed-journal retention; this makes no
claim about upstream data retention. Exact generated runtime configuration is
reviewed again before Run. Existing expert configuration remains available.

The factory writes a fresh owner-only staging directory, validates the generated
Flow/Profile/Connection/Probe and complete managed configuration, fsyncs, and
renames the whole bundle under a stable parent lock. Invalid input leaves no
published bundle; concurrent creators have at most one winner. It never opens
a capacity database or alters another setup. Tokenizer validation may launch the
contained local worker; this is not an upstream probe or a serving controller.
The shared wizard then handles
discovery, checks, admission, publication and Run with their existing revisions
and explicit confirmations. There is no automatic publication or payment.

The authenticated dashboard connects its initial form to this same factory.
Start it with `mayhem use --proxy-setup` and the host-only options described in
[SETUP_DASHBOARD.md](SETUP_DASHBOARD.md). Its existing exact-origin/session/CSRF
guards expose no path or credential readback. `--proxy-setup-config` resumes an
existing bundle and is mutually exclusive with first-time `--proxy-setup`.

Focused local checks:

```sh
cargo test -p mayhem-proxy --test setup bootstrap
cargo test -p mayhem-cli --bin mayhem proxy_provider::setup::bootstrap::tests
```

## Existing dashboard integration

```sh
mayhem use --proxy-setup-config /private/wizard.json --bind 127.0.0.1:11435
```

Use the normal printed provider-dashboard session link, then **Open proxy setup
wizard**. This requires the normal loaded wallet to match the profile provider.
It is disabled by default and startup refuses nonloopback binds. The configured
literal IP and port must match the actual listener, Host and mutation Origin;
`localhost` aliases, reverse-proxy origins and wildcard binds are not accepted.
It adds no listener and no second wallet-password prompt. A provider bootstrap
link opened on an alternate hostname redirects to the configured origin before
consuming the one-use session. Already established alternate-host cookies are
not transferred; the page explains the canonical-origin session requirement.

Routes under `/mayhem/dashboard/provider/setup` share the existing per-process
session and no-store/CSP/frame protections. The action route additionally
requires a fresh per-process CSRF token, exact Origin and JSON before consuming
its at-most-64-KiB body within five seconds. There is no cross-origin CORS grant. Restart invalidates
session and CSRF credentials, while the exact retained draft survives.

Invoice display shows the required, verified, missing and excess amounts in the
original rail's units (FIAT minor units; crypto base units), exact receiver,
network/chain/token and quote expiry. It is a **last authenticated snapshot**, not
live confirmation. Explicit status reconciliation recovers the same invoice.
Checkout URLs are returned only to the requesting session and removed from the
retained projection. Browser checkout return never marks a payment successful.
Enrollment actions do not send a fee transfer, issue a permit, register a native
provider or append a ledger operation. The separate Publish action submits only
the exact reviewed proxy registry operations. Existing service/canonical policy remains authoritative. Cached
permits still undergo the original exact provider/network/operation and canonical
validation before publication. Reconcile the original invoice with the admission
service for an expired permit; never create another invoice merely to retry.
Renewal and collection remain service/canonical decisions.

A changed probe file invalidates the reviewed probe-plan digest. The probe always
uses existing shared capacity and cumulative allowance. Exhaustion or uncertain
work cannot become a successful observation or reset the allowance. Publication
requires an explicit review and separate confirmation; recovery reconciles the
original retained operation rather than creating another identity/fee.


## Reviewed managed Run

Run requires a current structurally checked draft, an actual protocol-validated
probe, and the exact publication confirmed by the canonical peer. Its capacity
file must already be the existing managed `state_dir/capacity.redb`. Preview
validates the full generated configuration and protected connection/bridge/
tokenizer references without opening a capacity authority or dispatching work.
The original connection fingerprint, route, group, physical constraints and
cumulative probe policy are retained. A template cannot substitute other offers,
rates or an adapter. Setting `allow_recovery_probes` authorizes only the exact
existing probe request within the same remaining cumulative allowance.

The CLI has **Review Run**, **Begin Run**, and **Reconcile Run** actions. The
existing authenticated dashboard has the same controls. Unattended action bodies
are `{"action":"run_plan","expected_revision":N}`,
`{"action":"start_run","expected_revision":N,"plan_digest":"…"}`, and
`{"action":"recover_run"}`. Start requires explicit acknowledgement of the
reviewed plan. The content-addressed owner-only managed configuration and original
Run intent are fsynced before any supervisor installation request.

An existing running mayhemd with authenticated exact-child inspection is a Run
prerequisite; the wizard does not start a second supervisor. The host reuses its
existing wallet locator, password-file reference, normal
wallet helper, fixed local mayhemd origin and authenticated control token. The
stable child name remains one controller per wallet and network, matching
`provider proxy add`. The generated child command uses hidden supervised config
pinning; every restart verifies the full canonical config digest and exact
connection fingerprint before opening any stores. A missing, empty or unrelated
capacity database is rejected instead of re-created. The original database's
exclusive lock prevents two controllers, and its used probe counters/retained
leases are never reset. Fresh health and canonical registration still gate
presence and admission after restart.

Authenticated `POST /children/inspect` accepts only an exact child name and
expected canonical full-child-config hash. It returns `missing`, `nonpersistent`,
`mismatch`, or `matched` plus sanitized process lifecycle. It exports no argv,
environment, paths, credentials, or actual configuration. Its durable exact-name
lookup does not scan other children. Config equality includes all daemon defaults,
fixed binary/wallet/config references and the managed config digest.

A lost installation ACK is reconciled by inspecting that original identity. A
matched persistent child may be stopped, restarting, or running; none of those
states attests capacity. Recovery never installs/replaces a child, changes the
retained plan, reloads the editable runtime template, or creates a new budget.
A missing child requires another explicit Start of the same acknowledged plan;
a conflicting child requires operator reconciliation, not automatic removal.
An edited draft cannot replace a previously retained Run. A confirmed commercial-only publication keeps that original child; other execution changes still require explicit lifecycle reconciliation.

The first Run plan remains immutable for this draft directory. A commercial-only
publication leaves its controller identity, capacity store and probe allowance
unchanged. Provider-owned presence reads follow the latest canonical rates for
that same offer slot; paid requests and accepted jobs retain their exact offer
bindings. Other runtime reconfiguration is not an automatic remove/re-add.

## Change rates without changing the original purchase

Use **e Review new rates** in the resumed CLI wizard, or **Change rates in this
market → Review new rates** in the dashboard. Enter exact USD prices for the
existing billing units, request fee and minimum session. This path preserves the
market, membership, rails, endpoint, metering units, model, connection, resource
limits and settlement policy. It requires the current configuration's completed
publication. Use the separate reconfiguration path for other changes.

Review reads fresh admitted-provider state and the current same-slot offers from
the configured trusted Core peer. It derives the next canonical operation sequence
and each slot's next offer revision, including unequal existing revisions. The
bounded retained proposal shows old and new terms; reviewing it does not alter
the draft, sign, collect a fee, probe or restart inference.

Use **z Publish retained rate review** or **Publish reviewed rates** to confirm.
Before a new submission, setup rechecks the exact sequence and offer baseline. A
concurrent publication invalidates the review; refresh and review the new facts.
Only `set_offer` operations are signed. The existing entitlement is reused: no
replacement market, membership, invoice or permit is created. Original probe
identity, evidence and cumulative accounting survive the commercial-only update.
The running controller adopts canonical rates through its existing presence read;
Run inspection/recovery retains the original immutable installed child.

Unattended actions use the same strict shared flow:

- `rate_plan`: `expected_revision` and `choices` containing each existing `slot_id`,
  exact `rates`, decimal-string `per_request_au` and `min_session_au`. Obtain
  current slots from `rate_choices` in `wizard --inspect`; never invent them.
- `publish_rates`: current `expected_revision` and the retained `plan_digest`.

The action wire uses exact AU strings; the interactive forms convert USD at
18 decimal places without floats or rounding. Sequence, revisions, membership,
rails and private paths are not accepted as rate choices. A pending signed
publication must be recovered before another rate edit. After a lost response,
refresh the existing wizard and use the retained rate confirmation or **Recover
original publication**. If the local draft write completed before submission,
the retained proposal offers the same recovery. Do not create a second invoice,
controller or allowance. Accepted work keeps its original signed prices; native
price-floor commands do not change proxy rates.

The expert `prepare` / `check` / `publication-plan --offers-only` / `publish
--offers-only` interface remains available for explicitly authored profiles. It
requires explicit canonical sequence and offer revisions; the guided rate flow
performs those reads and derivations instead.

## Read health for the correct controller

**Reconcile Run** reports the exact persistent child and its running/restarting/
stopped state. It does not establish fresh upstream readiness or capacity. The
provider controller starts with unknown health and needs current canonical
registration plus fresh route evidence. LLM speed uses the approved tokenizer;
a successful short probe can still have insufficient speed evidence. Only an
explicitly enabled recovery-probe policy can spend more of the existing probe
allowance when evidence expires.

`mayhem provider health` describes the native lane. There is no `provider proxy
health` subcommand or public `/v1/proxy/health` endpoint. The foreground proxy
controller prints one final bounded health summary on exit. In a separately
configured proxy gateway, authenticated `GET /v1/proxy/offers` and exact offer
details expose `availability: {status, observed_at_ms, expires_at_ms}` for that
gateway's own control instance. Another process's catalog or heartbeat does not
make this one ready. A directory read does not subscribe unobserved markets or
reserve capacity; `checking`, `heartbeat_missing`, `stale_evidence` and
`controller_conflict` must remain visible. See [DIRECTORY.md](../DIRECTORY.md)
and [PRESENCE.md](../PRESENCE.md).

`mayhem provider proxy catalog status --config PATH` reads a stopped controller's
cache; it is not live health. Do not open a second owner against the running
controller's database. Native `/v1/models` visibility and a running process are
also insufficient proof of proxy readiness.

## Platform and acceptance limits

Protected setup and managed configuration use platform filesystem checks, not a
permission override. The exercised complete guided CLI/dashboard/Run path is
macOS. Native Linux ARM and x86_64 worker/tokenizer isolation have separate
acceptance evidence; that does not prove complete fresh installation and managed
Run. Linux startup needs supported seccomp and procfs and fails closed when
containment cannot be installed. Emulation alone is not proof.

On Windows 11 x86_64 build 26300, native protected storage and actual bundled
decoder/tokenizer checks pass. Complete installation, mayhemd/Run, other Windows
versions and ARM64 remain separate acceptance requirements. Packaging executable
files or cross-compiling cannot establish those claims. Never fall back to an
uncontained worker or weaken file protection.
See [WORKER_CONTAINMENT.md](WORKER_CONTAINMENT.md) and
[TOKENIZER_ISOLATION.md](TOKENIZER_ISOLATION.md) for the exact boundaries.

Local fixture tests establish the documented state/recovery behavior. They do
not establish live payment collection, issuer activation, production networking,
remote model identity or OS support beyond the host actually tested.

## Local validation

```sh
cargo test -p mayhem-proxy --test setup flow::
cargo test -p mayhem-gateway --lib proxy_setup::tests
cargo check -p mayhem-cli --bin mayhem
cargo test -p mayhem-proxy --test setup 'run::'
cargo test -p mayhemd exact_child_inspection_http_auth_lost_ack_and_restart
# Explicit local binaries, synthetic encrypted wallet and loopback fixtures only:
cargo build -p mayhem-cli --bin mayhem -p mayhemd --bin mayhemd
MAYHEM_SETUP_CLI_BINARY="${CARGO_TARGET_DIR:-target}/debug/mayhem" \
MAYHEM_SETUP_DAEMON_BINARY="${CARGO_TARGET_DIR:-target}/debug/mayhemd" \
cargo test -p mayhem-proxy --test setup run_cli_real_supervisor_publication_restart -- --ignored
```

The focused setup test uses a credential-free loopback model-list endpoint and
actual isolated worker for its one explicitly budgeted probe. Dashboard tests use
a real loopback HTTP listener and synthetic wallet to check session, Host,
Origin/CSRF, pre-body rejection, private projection, CAS and restart boundaries.
An ignored, short-lived `local_browser_and_cli_fixture` test is available for a
local coordinator through `MAYHEM_SETUP_BROWSER_READY`; it has no financial service
or real upstream configuration and is not a deployment entry point.

The opt-in CLI Run acceptance archives the committed Intercom asset source into
its disposable directory, seals that fixture using the existing release-identity
test algorithm, and runs normal startup verification. It does not modify the
shared release seal or disable verification. The actual encrypted synthetic
wallet helper, CLI and mayhemd are exercised; canonical publication is the local
contract fixture and SC-Bridge is a bounded loopback protocol double. This is not
production relay/fee collection evidence.
