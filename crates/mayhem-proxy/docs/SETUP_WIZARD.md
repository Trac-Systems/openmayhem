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
and original probe/publication recovery. It does **not** export or install a
managed serving configuration, start serving, pay a fee, renew a permit, suggest
markets from a canonical directory, or implement general connection editing in
the browser. Those remain explicit next steps. Existing native serving is not
reconfigured. A published offer is not proof of readiness.

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

## Local validation

```sh
cargo test -p mayhem-proxy --test setup flow::
cargo test -p mayhem-gateway --lib proxy_setup::tests
cargo check -p mayhem-cli --bin mayhem
```

The focused setup test uses a credential-free loopback model-list endpoint and
actual isolated worker for its one explicitly budgeted probe. Dashboard tests use
a real loopback HTTP listener and synthetic wallet to check session, Host,
Origin/CSRF, pre-body rejection, private projection, CAS and restart boundaries.
An ignored, short-lived `local_browser_and_cli_fixture` test is available for a
local coordinator through `MAYHEM_SETUP_BROWSER_READY`; it has no financial service
or real upstream configuration and is not a deployment entry point.
