# Local provider setup drafts

For first setup without hand-authored configuration, use the
[guided CLI/dashboard workflow](docs/SETUP_WIZARD.md). This document describes the
advanced draft commands used by the same workflow.

The shared `mayhem_proxy::setup` library saves and reviews an explicit provider
declaration before any wallet, upstream connection, payment or publication is
opened. The CLI is a thin client of this same state implementation:

Private file verification and locking must succeed on the actual host. Windows
protected-storage integration and native first-install/restart/Run acceptance
remain separately gated; compiled code is not operational proof. See the
[current platform limits](docs/SETUP_WIZARD.md#platform-and-acceptance-limits).

```sh
mayhem provider proxy setup create --directory /absolute/private/setup --input /absolute/private/declaration.json
mayhem provider proxy setup inspect --directory /absolute/private/setup
mayhem provider proxy setup check --directory /absolute/private/setup --expected-revision 1
mayhem provider proxy setup update --directory /absolute/private/setup --expected-revision 2 --input /absolute/private/revised.json
```

The setup directory must already exist with owner-only permissions. Inputs and
connection files must be private regular files. Symlinks, hard links, unsafe
permissions, oversized documents and unknown fields are rejected. Relative
connection-file references resolve against the declaration file. Credentials
remain references in the private connection configuration; these commands never
read their values, unlock a wallet, contact a backend or start a process.

The version-1 `setup::Input` records the explicit network/provider identity,
connection file, private adapter snapshot, public market/membership, one to16
offers, create/join selection, declared operation sequence and public settlement
policy. One draft selects one endpoint adapter and one membership; its offers
can cover multiple explicitly declared submarkets. All four current endpoint
families and custom endpoint contracts use the same validators. Membership
contract/recipe, connection revision, metering, identity, rails and every offer
rate must agree. No category name, endpoint guess or upstream model label fills
missing fields or grants an assurance level.

Each directory owns one stable random draft ID and a monotonic revision.
Create refuses an existing draft. Update and check require the exact current
revision. An update always invalidates the earlier local check. Changing the
network/provider identity requires a separate setup instead of transferring an
existing draft ID. The store uses a nonblocking process lock, bounded files,
file sync and atomic replacement followed by directory sync. Interrupted
temporary writes never become authoritative on resume. Corrupt current files
are not silently replaced. An uncertain post-rename result requires inspecting
the original draft; it is not permission to create another invoice or identity.

`inspect` and every successful draft mutation return only the public `Review`.
It contains public market/membership/offer and endpoint-contract data, policy,
draft identity/revision and explicit state. Private connection paths and
fingerprints, upstream model mappings, local resource limits, URLs, credentials
and credential references are omitted. Recipe hashes and connection revisions
already required by the public membership remain present.

`unchecked`, `structurally_valid` and `recheck_required` describe local structure
only. Inspection rechecks the current connection fingerprint; file drift removes
the unsigned admission handoff until an explicit update and check. The review
always reports operator-declared claims and serving not started. Publication
remains not submitted until the explicit commands below. Admission remains `not_checked` until an explicit canonical read
described below; its observations never authorize publication. Without an explicit probe it
reports `probe_status: not_run` and `probe: null`. The optional handoff is the
exact unsigned initial `ProxyOperation` and its canonical digest, for the later
identity/invoice workflow to bind. It is not a permit, payment request, proof of
provider control, current sequence or evidence of unused admission entitlement.

An explicit probe uses the existing `execution::probes::Controller`, real HTTP
connector and isolated decoder, plus persistent `capacity::probes` accounting:

```sh
mayhem provider proxy setup probe --directory /absolute/private/setup --expected-revision 2 --plan /absolute/private/probe.json
mayhem provider proxy setup recover-probe --directory /absolute/private/setup --expected-revision 3
```

`ProbePlan` is a protected, version-1 JSON file. It requires `scope`, `budget`,
`worker_program`, `worker_directory`, `request`, `streaming`, `max_output_tokens`
and `timeout_ms`. Paths resolve against the plan file. `scope` contains the
stable `capacity_file`, `route`, `connection_group`, `connection_ceiling`,
`route_ceiling` and sorted `constraints` (`id`, `ceiling`). The connection group
must equal the declared membership's capacity group. The first probe pins this
complete scope permanently to the original draft; updates preserve it. All
aliases sharing a backend or credential pool **must use the same capacity
authority and physical constraints**, including subsequent managed serving.
The setup command cannot discover an undeclared external alias. Use the managed
runtime's `state_dir/capacity.redb` when sharing its authority; its exclusive
database lock rejects this command while that runtime owns the store. Never use
a new capacity file to retry or work around retained occupancy.

`budget` is the existing cumulative `max_attempts`, `max_cost_microusd` and
`per_attempt_cost_microusd` allowance. It authorizes operator upstream use; it is
not an exact upstream price or a buyer balance. Repeated plans/restarts preserve
consumed allowance. An increase is a new explicit operator authorization. Zero
per-attempt cost is permitted only when the operator deliberately declares a
free backend. The capacity file's parent must be owner-only; once pinned, a
missing store fails closed instead of recreating allowance. This setup entry
point supports authorities within 1024 groups, 4096 routes and 65536 active
leases; it does not replace the managed runtime's broader configuration path.

The bundled trusted decoder program must be an absolute protected executable;
its working directory must be empty and owner-only. A probe has at most one
decoder child, a 16 KiB request, 64 KiB response, 1024 declared output tokens and
30 seconds of upstream time, further constrained by the selected adapter and
connection. LLM requests must explicitly include a positive supported output
limit. The entire controller run has ten additional seconds for decoder setup
and completion. No tokenizer is loaded by this setup command, so even successful
protocol validation reports native throughput `not_verified`.

`probe` requires the checked original configuration and an exact draft revision.
It saves a durable pending intent before dispatch and retains the draft lock
until the final report is written. Success advances the revision twice (intent
and report); an interrupted call may have saved only the first revision. Always
inspect the original draft after an uncertain CLI result. `recover-probe` never
dispatches and only cancels prepared work when both the original saved probe ID
and its complete specification match. Unknown IDs, dispatched work and later
identical attempts remain occupied. A completion hash without a saved successful
controller result is not treated as success. There is no automatic resend,
timeout refund, occupancy expiry or budget reset.

Setup saves a stable reservation intent and its derived original probe ID before
reservation. The ID binds the authority's network/provider identity, the whole
probe specification, a fresh nonce and the expected cumulative `used_attempts`
counter. The capacity transaction compares that counter and increments it with
the reservation. A duplicate or closed intent cannot acquire another reservation
or dispatch permit, including after restart, allowance renewal or replacement of
the bounded last-completion record. No per-attempt history or new database table
is required. A new nonce or expected counter declares a new explicit attempt;
it does not authorize bypassing existing occupancy.

An interruption before reserve leaves the counter unchanged and no probe to
release. An interruption after reserve but before dispatch leaves the original
`Prepared` record, which `recover-probe` can identify and safely cancel without
refunding consumed allowance. Dispatched work remains uncertain. Losing the
successful setup report after capacity completion yields `not_validated`, not
invented protocol evidence or permission to re-execute the original intent.
Legacy drafts written before stable intents may have retained work with no saved
original ID. They remain fail-closed with `legacy_probe_identity_unavailable`;
matching configuration hashes cannot reconstruct that identity.

The optional public `probe` report contains only `state`,
`for_current_configuration`, `probe_id`, `evidence_hash` and
`native_throughput`, and `recovery_reason`. `protocol_validated` refers only to that bounded request and
the retained configuration. Protocol/resource/capability changes yield `recheck_required`;
interruption or uncertain work yields `recovery_required`. Requests, replies,
private paths/fingerprints, resource limits and budget configuration stay private.
This is a local controller observation, not a conformance certificate for every
advertised context, performance measurement, admission permit or serving claim.

The state library and unattended CLI are a setup foundation, not the complete
interactive/dashboard wizard. The explicit canonical read below supplies current
provider admission and sequence observations. The explicit publication commands
below accept an existing verifier-signed permit and use the original operation
journal/gate. The invoice, collection and permit-issuance service remains
unimplemented. A paid or pending invoice cannot be replaced merely because a
CLI request was lost.
Only confirmed canonical admission/publication can enable subsequent serving.
Existing `provider proxy add` retains its supervisor-installation meaning;
these setup commands never invoke it automatically.

## Connection discovery and profile preparation

The same library now supports an explicit model-list read and offline profile
preparation before a full declaration exists:

```sh
mayhem provider proxy setup profiles
mayhem provider proxy setup discover --directory /absolute/private/setup --connection /absolute/private/connection.json --expected-revision 0
mayhem provider proxy setup inventory --directory /absolute/private/setup --show-models
mayhem provider proxy setup prepare --directory /absolute/private/setup --input /absolute/private/profile.json
mayhem provider proxy setup resume --directory /absolute/private/setup
mayhem provider proxy setup check --directory /absolute/private/setup --expected-revision 1
```

`profiles` returns the existing four same-protocol endpoint templates and public
metering definitions. These are local adapter definitions, not detected upstream
capabilities. `discover` performs at most one GET through the configured `models`
operation, with the connector's existing destination checks, no redirects,
environment proxies or retries. It is the only new command that resolves the
explicit connection's credential reference. It does not send an inference request,
launch a decoder, reserve capacity, collect fees, publish or start serving.

Discovery has a 64 KiB response limit, retains at most 128 unique model IDs (256
UTF-8 bytes each), and has an explicit `--timeout-ms` of 1–10000 milliseconds,
default 5000. Tighter connection byte/connect/idle limits still apply. Only
validated IDs are retained; arbitrary model metadata, response headers and vendor
error text are discarded. Invalid IDs/duplicates fail the observation. A larger
valid list or explicit upstream `has_more` produces `truncated: true`; this is a
single-response observation, never a promise to enumerate every upstream model.
An absent model-list path or unsupported endpoint reports `unsupported`.
Explicit profiles remain usable without `/models`.

One protected `discovery.json` shares the existing directory lock and atomic
file writer. It is separate from `draft.json`: discovery cannot modify a checked
declaration, reset a probe allowance or invalidate retained work. Inventory has
its own stable ID and CAS revision. Revision 0 creates it; an explicit refresh
uses its current revision and advances twice, saving `pending` before network
I/O and the final result afterward. An interruption leaves the original pending
revision. `inventory` reads it without retrying. A changed connection marks the
observation as unsuitable for the current configuration; drift during discovery
discards the returned model IDs. No timer auto-refreshes this file.

Default discovery/inventory output omits private model IDs. `inventory
--show-models` explicitly includes them for the local operator. Neither form
contains URLs, connection paths/fingerprints, credentials or arbitrary metadata.
The public draft review is unchanged. Model identity, capabilities, readiness
and concurrency always remain `not_verified`; discovery is not conformance or
serving evidence, and fees remain `not_checked`.

`ProfileInput` is a protected version-1 JSON document containing:

- Explicit `network`, `provider_pubkey`, `connection_file`, `upstream_model`,
  adapter `limits`, `sequence` and full `settlement_policy`.
- `profile: {"kind":"standard","endpoint":"chat"}` (also `completions`,
  `responses`, `decisions`), or `{"kind":"custom","endpoint":"chat",
  "contract": ...}` with the complete normative endpoint contract.
- `market: {"action":"create_market","slug": ..., "model": ...}` with an
  explicit public model claim, or `{"action":"join_market","market": ...}`
  with the exact existing public market descriptor. No canonical existence or
  family/model identity is inferred by this offline command.
- `membership` with `revision`, `served_context`, `max_concurrency`,
  `capacity_group` and `accepted_rails`.
- One to 16 `offers`, each containing `revision`, `ctx_bracket`, `outcome_class`,
  a complete `rates` map, `per_request_au`, `min_session_au` and `accepted_rails`.
  AU money values retain the protocol's decimal-string encoding.

Preparation derives the adapter, contract/recipe/metering bindings, market ID,
membership and offer bindings using existing validators. It does not guess model
family, prices, capabilities, allocation, rail or endpoint. Custom profiles can
be copied/reused as protected operator input; private model mappings stay private.
Public recipe import/export and a guided dashboard remain unfinished.

Without `--expected-revision`, `prepare` creates the original draft and refuses
an existing one. With an exact revision it uses the original update operation,
preserving its ID and probe scope while invalidating the old structural check.
Changing rates on that same market retains its market identity and does not
automatically run a probe. Saved protocol evidence has a separate versioned
configuration binding: unchanged protocol/configuration can reuse the original
probe ID and evidence after changing prices, fixed/minimum fees, publication
revision/sequence counters, enabled payment rails or settlement policy. The full
declaration still requires the ordinary structural check before its admission
handoff is available. No accepted purchase or financial terms are changed.

That configuration binding conservatively includes the exact network/provider,
full public market, complete adapter/contract/model mapping, connection
fingerprint/revision, context/concurrency/capacity-group claims, offer
endpoint/context/outcome/metering slots and pinned probe scope. Any change to
those facts invalidates the whole protocol observation. This is configuration
reuse, not independent per-feature conformance reuse, freshness, or proof of
model identity/readiness. Reordering the same offer slots does not change it.
Exhaustive typed field matching forces new input fields to be reviewed for this
binding. Private binding hashes are not included in the public review.

Legacy successful observations can acquire the new binding only during an
explicit update while their original full declaration binding still matches and
their connection is unchanged. Already-stale or uncertain legacy observations
are never promoted from a partial match. Original specifications, evidence
hashes, probe IDs and reservation intents remain intact. No update/check opens
the capacity authority, consumes or resets allowance, retries inference, frees
uncertain occupancy, or changes exact-ID recovery requirements.
`resume` is an alias of `inspect`, with no network request. Existing create/update,
check, probe and recover-probe behavior and the actual admission fee gate remain
unchanged. The complete wizard must still connect original invoice/permit
recovery and managed serving before claiming paid provider readiness.

## Canonical provider admission observation

After checking a draft, explicitly read its provider's canonical registration:

```sh
mayhem provider proxy setup admission-check --directory /absolute/private/setup --expected-revision 2 --peer-rpc http://127.0.0.1:17800/v1
```

The RPC base must identify an operator-trusted Core peer, using literal loopback
HTTP or authenticated-by-origin HTTPS. Credentials in URLs, redirects and
environment proxies are rejected. The peer must already hold the selected
provider identity and support the new `proxy_provider_state` read service; an old
peer reports unavailable. This read never opens a wallet. The peer's existing signed
read transport binds its requester, fresh challenge, network and exact draft
operation digest to the canonical indexer's reply. It performs no admission or
financial signature and no transaction append.

The indexer uses its existing verified signed snapshot and rechecks all consulted
keys before returning. It reads at most five exact registry keys: policy,
provider, provider revocation, consumed entitlement ownership and entitlement
revocation. There are no directory/history scans, invoice lookups, balance reads,
capacity reservations, probes or model calls. Four independent read permits and
a 15-second deadline bound this service; timed-out work retains its permit until
its actual snapshot cleanup finishes. `--timeout-ms` accepts 1–15000, defaults to
5000 and can shorten that bound. Responses have an 8 KiB limit.

The original protected draft receives a pending observation before I/O and a
final observation afterward, advancing its CAS revision twice. A cancelled read
may leave only the pending revision. Inspect the same draft and explicitly repeat
`admission-check` with that revision; `inspect`/`resume` never send a request. A
repeat reads state only and cannot duplicate payment or publication. The exact
configuration and connection remain bound. The last accepted proof/epoch floor
survives unsuccessful reads; regressed or substituted canonical proofs fail
closed instead of replacing it.

The public optional `admission` report includes the canonical context/proof,
registry-enabled flag, fee-policy hash, provider sequence, entitlement ID,
revocation flags, next sequence and whether this exact operation was already
applied. It includes observation/expiry times and a current-configuration flag.
Registry configuration has no independent revision counter in the current
protocol; its policy facts are bound to the same signed snapshot proof instead.
Sequence exhaustion is explicit as no next sequence. Disabled registry policy
remains visible even for an already admitted identity. Missing records produce
`observed_not_registered`, which **never means unpaid**: an off-ledger invoice or
permit may already exist. The report always says `payment_status: not_checked`
and `authorizes_publication: false`.

These are historical observations, not reusable admission permits. Fifteen-second
expiry, a future clock or changed declaration/connection produces
`refresh_required`; unavailable/malformed peers produce `observation_unavailable`.
Neither state authorizes another invoice or fee. A future invoice flow must
recover the original off-ledger invoice/permit and recheck current canonical
facts. The publication path must still validate the exact signed operation,
entitlement and current sequence at its existing gate. No serving readiness,
paid admission collection or completed provider wizard is claimed here.


## Explicit admitted publication and recovery

The shared `Store` now connects a checked draft to the existing canonical writer.
Review the exact public plan before explicitly signing it with the existing
provider wallet:

```sh
mayhem provider proxy setup publication-plan --directory /absolute/private/setup --expected-revision 2
mayhem provider proxy setup publish --directory /absolute/private/setup --expected-revision 2 --peer-rpc http://127.0.0.1:17800/v1 --home /absolute/private/wallet --admission-permit /absolute/private/permit.json
mayhem provider proxy setup inspect --directory /absolute/private/setup
mayhem provider proxy setup recover-publication --directory /absolute/private/setup --expected-revision CURRENT_REVISION --peer-rpc http://127.0.0.1:17800/v1
```

Only `publish` unlocks a wallet, using the existing wallet locator and protected
key handling. It signs only the exact reviewed typed registry operations. Other
setup commands, including recovery, do not unlock it. No command requests a fee,
creates an invoice, issues a permit, transfers funds, installs serving, invokes a
model or calibrates a runtime. An imported private permit file contains only the
existing public permit and issuer signature; cryptographic verification is not a
claim that its issuer is currently authorized. The canonical gate still checks
active issuers, the exact initial operation, fee policy, trusted epoch, evidence
consumption, revocation, quotas and all family/endpoint/membership/offer policies.

A fresh exact provider-state read runs before each operation. An unregistered
provider without a verified permit reports `admission_required` before any submit.
That state does not mean unpaid: the original invoice/permit may exist elsewhere.
An already admitted identity uses no new permit or fee. A present permit cannot
be replaced in a retained attempt; only an `admission_required` attempt with no
permit can attach one bound to its original first operation. Issuer rotation,
permit reissue, invoice recovery and D3 collection/reversal policy require their
own separately authorized implementation.

The guided CLI/dashboard rate editor uses `Store::plan_rates` and
`Store::publish_rates`. It retains a separate unsigned review, reads fresh
canonical admission/current-offer state, derives sequences and per-slot revisions,
and rechecks before signing. Only commercial terms change; probe accounting,
market/membership/rails and the installed controller remain intact. See
[guided rate updates](docs/SETUP_WIZARD.md#change-rates-without-changing-the-original-purchase).

One expert plan contains create/join plus its saved offers (at most 17 operations).
`publication-plan --offers-only` and `publish --offers-only` instead publish only
the saved offers on the existing admitted membership. First update/check the
original draft with explicit next sequence, higher offer revisions and rates;
keep the market/membership identities unchanged. The existing canonical gate
validates those references. This changes neither accepted jobs nor their locked
rates, does not create another market, and reuses the existing entitlement.
There is no guessed next sequence or silent create-versus-update fallback.

Before network I/O, setup atomically retains the entire original public plan,
provider signatures, imported permit, trusted peer origin and configuration
binding in its protected draft. It persists pending state before each submit and
advances progress only after a fresh signed canonical view confirms that exact
provider sequence and operation digest. An HTTP success, writer ACK or returned
error text never substitutes for this confirmation. The existing publication
journal owns its original writer nonce and performs pre-append admission again.
A lost response, interrupted caller, malformed response, offline peer or failed
local write requires inspecting and recovering the same draft, not signing a
replacement sequence or acquiring another permit. Recovery can confirm an already
applied operation despite subsequent connection drift, but it cannot dispatch
new work from changed configuration. Another publication advancing the same
provider beyond an unconfirmed operation produces an explicit sequence conflict;
this client does not guess historical success or silently rebase the plan.

The bound origin accepts HTTPS or explicit literal-loopback HTTP, with no URL
credentials, query, fragment, redirects, environment proxy or transport retries.
Recovery cannot change that origin. Each call has one 1–15000 ms deadline across
the whole plan (default 5000), not a fresh deadline per operation. Provider reads
are bounded to 8 KiB and ignored publication ACKs to 16 KiB; no history/catalog
scan occurs. At most 17 typed 64 KiB envelopes are retained, alongside the original
512 KiB input and bounded recovery metadata. Each directory's existing lock and
CAS revision fence concurrent clients. Updating an unfinished signed plan is
refused; a completed plan may be superseded only by another explicit checked
draft update. Revisions advance at durable progress points, so callers must
inspect after an uncertain result instead of calculating the next revision.

The optional public `publication` report exposes state, plan digest, total and
confirmed operation counts, pending operation digest, current-configuration flag,
latest canonical observation, bounded reason and `authorizes_serving: false`.
It omits signatures, invoice/evidence commitments, peer URL and private input.
`canonical_operations_confirmed` is a historical publication fact, not live
capacity or readiness. Once a publication is retained, `publication-plan` is the
sole original unsigned operation handoff; the older `admission_handoff` is hidden
to avoid constructing a replacement invoice intent.

The local acceptance fixture uses a real signed Autobase/Hyperbee view, canonical
contract, pre-append gate and durable publication journal over loopback HTTP.
Its owned-provider read facade replaces remote service transport, and its issuer
signs synthetic allocations. It proves create/join, all four endpoint families,
FIAT/TNK/TAP permit binding, lost-response recovery and same-market price updates.
It does not prove actual fee collection, production issuer deployment or public
provider readiness. Interactive/dashboard forms, unattended invoice detection,
managed serving handoff, Windows protection and full D2–D4 policy remain open.

## Off-ledger fee enrollment

A checked draft can now authenticate to the explicitly selected admission API
using its existing provider wallet:

```sh
mayhem provider proxy setup enrollment --directory private-draft \
  --expected-revision 2 --admission-origin https://api.example.com
mayhem provider proxy setup enrollment --directory private-draft \
  --expected-revision 2 --admission-origin https://api.example.com \
  --action create --rail tnk
mayhem provider proxy setup enrollment --directory private-draft \
  --expected-revision 2 --admission-origin https://api.example.com \
  --action checkout
```

The default action is `status`; `create` requires exactly one of `fiat`, `tnk`,
`tap`. `checkout` is for the existing FIAT invoice and returns its original Stripe
URL. Wallet selection/unlock uses the same options as `publish`. No payment is
sent, registry operation published, model installed or serving process started.

The client validates the exact network, origin, provider, operation, action,
request digest, nonce and expiry before signing a short-lived identity challenge.
Its bearer grant remains in memory for that action. Each operation has a bounded
network timeout; later status/retry recovers the same provider/network invoice,
including its original rail and valuation. An unavailable status is not evidence
that another fee is owed. Invoice issuance or `ready` is not canonical admission;
the existing protected publication gate independently verifies the signed permit.
`original_operation_matches` distinguishes retained payment for an earlier draft
from permission to publish the current one.

Crypto amount fields are exact base units (18 decimals); FIAT USD uses cents.
The shared client returns structured status for the forthcoming setup dashboard.
Complete interactive wizard/payment-page delivery, custody provisioning,
expiry/reissue and release-policy activation are still required. The command is
not an assertion that public enrollment has been enabled.
