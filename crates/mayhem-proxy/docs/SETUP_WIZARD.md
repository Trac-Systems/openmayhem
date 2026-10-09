# Local shared provider setup

The CLI wizard and the opt-in gateway dashboard use `setup::Flow` and the same
owner-only `Store`. This is a connected setup surface for the existing four
compatible endpoint profiles and reviewed custom profiles. Compatible profiles
calculate their own recipe, endpoint-contract and metering hashes. They do not
require the operator to hand-author a recipe. Discovery lists upstream model IDs;
it never certifies model identity, concurrency, capabilities or readiness.

This checkpoint includes explicit configuration review, one bounded model-list
read, manual/discovered model selection, full existing price-map editing,
structural checks, reviewed bounded probes, canonical admission reads,
provider-key invoice/create/status/checkout, exact publication review/signing,
original probe/publication recovery, and reviewed installation/reconciliation of
one existing managed provider controller. It does **not** pay a fee, renew a permit,
suggest markets from a canonical directory, or implement general connection
editing in the browser. Those remain separate work. Existing native serving and
upstream model servers are not reconfigured. A published offer or running process
is not proof of readiness.

## Configuration and identity

`FlowConfig` is an owner-only JSON file (0600, no symlink). Its fields are:

- `schema_version: 1`, `directory`: existing owner-only setup directory (0700).
- `profile`: existing `ProfileInput` from `setup prepare`; normally
  `profile: {"kind":"standard","endpoint":"openai_chat_completions"}` (or `openai_completions`,
  `openai_responses`, `mayhem_decisions`). Network, provider public key, connection reference,
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
unit. The simple rail editor narrows each offer's existing rails. Exact market
joining and changes to protected endpoint/identity configuration use the existing
profile interface; neither aliases nor discovered labels become canonical proof.
Unattended actions use strict `FlowAction` JSON in an owner-only bounded file.
Each mutation carries the current retained revision; a stale client fails. The
same wallet locator/cache is used by existing CLI commands. Read/check actions do
not unlock a wallet. Enrollment signs only the existing scoped challenges;
publication signs only the exact acknowledged `plan_digest`.

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
No fee transfer, issuance, native registration or ledger append is performed by
the flow itself. Existing service/canonical policy remains authoritative. Cached
permits still undergo the original exact provider/network/operation and canonical
validation before publication; an expired permit is not renewed by this wizard.

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
An edited draft cannot reprice or replace a previously retained Run.

The first Run plan is immutable for this draft directory. Updating an installed
controller to a later publication remains an explicit operator lifecycle task;
this wizard does not silently remove/re-add it. Runtime policy/template authoring
and the current low-level revision/AU controls still need onboarding UI polish.

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
