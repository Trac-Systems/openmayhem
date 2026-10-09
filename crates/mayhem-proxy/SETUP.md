# Local provider setup drafts

The shared `mayhem_proxy::setup` library saves and reviews an explicit provider
declaration before any wallet, upstream connection, payment or publication is
opened. The CLI is a thin client of this same state implementation:

Private file verification and locking are currently implemented for Unix.
Other platforms fail closed; Windows permissions and setup remain unfinished.

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

`inspect` and every successful mutation return only the public `Review`.
It contains public market/membership/offer and endpoint-contract data, policy,
draft identity/revision and explicit state. Private connection paths and
fingerprints, upstream model mappings, local resource limits, URLs, credentials
and credential references are omitted. Recipe hashes and connection revisions
already required by the public membership remain present.

`unchecked`, `structurally_valid` and `recheck_required` describe local structure
only. Inspection rechecks the current connection fingerprint; file drift removes
the unsigned admission handoff until an explicit update and check. The review
always reports operator-declared claims, admission not checked,
publication not submitted and serving not started. Without an explicit probe it
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

The existing controller reserves internally and returns its ID only on success.
A crash in the reserve-to-report interval can therefore leave `Prepared` work
whose original ID was never saved by setup. This entry point deliberately retains
that occupancy; setup cannot yet recover that pre-dispatch crash automatically.
Closing this gap requires a stable setup-attempt identity saved before reserve
and atomically retained/indexed by the capacity reservation, with bounded lookup
of the original attempt. A callback that merely saves an ID after reserve would
still leave a reserve-to-save crash window. This shared API extension remains
unfinished; configuration hashes alone must never stand in for attempt identity.

The optional public `probe` report contains only `state`,
`for_current_configuration`, `probe_id`, `evidence_hash` and
`native_throughput`. `protocol_validated` refers only to that bounded request and
the retained configuration. Configuration changes yield `recheck_required`;
interruption or uncertain work yields `recovery_required`. Requests, replies,
private paths/fingerprints, resource limits and budget configuration stay private.
This is a local controller observation, not a conformance certificate for every
advertised context, performance measurement, admission permit or serving claim.

The state library and unattended CLI are a setup foundation, not the complete
interactive/dashboard wizard. Next wiring must read canonical provider admission and
sequence before requesting an invoice, retain the original invoice/evidence,
verify a permit bound to the exact initial operation, and submit the typed
provider-signed operation through the existing publication journal/gate. A paid
or pending invoice cannot be replaced merely because a CLI request was lost.
Only confirmed canonical admission/publication can enable subsequent serving.
Existing `provider proxy add` retains its supervisor-installation meaning;
these setup commands never invoke it automatically.
