# First-time local provider dashboard

Start the existing wallet-backed gateway with explicit `mayhem use --proxy-setup`
activation. It remains bound to a literal loopback address. Open the authenticated
provider dashboard URL printed by Core, then **Open proxy setup wizard**. Alternate
hostnames redirect to the configured origin through the existing session bootstrap;
Host/Origin validation is not relaxed.

For LLM profiles, provision approved protected local tokenizer data and pass
`--proxy-setup-tokenizer-file /absolute/protected/tokenizer.json`. The CLI pins its
bytes; the form selects the opaque `approved` asset and shows its digest. This is
needed for local speed measurement, not billing or proof of remote model identity.
There is no tokenizer download or model-name guess. Decisions requires no tokenizer.

Optional host arguments:

- `--proxy-setup-api-key-file PATH`: owner-private existing key, selectable as
  `configured`. Alternatively enter a bearer key once in the password field.
- `--proxy-setup-restart-password-file PATH`: existing protected wallet password
  reference, otherwise the existing host reference is reused when present.
- `--proxy-setup-admission-origin ORIGIN`: explicit trusted admission service.
  Omission leaves enrollment unavailable; no receiver, fee or issuer is inferred.

No Flow, Connection, Probe or runtime-policy JSON is required. The CLI `setup init`
and first-create dashboard use shared guided reads and the same factory. Enter an
API URL and a protected credential reference or write-only bearer value, then
explicitly **Find models on this server**. One guarded GET `/models` returns a
bounded list (128 IDs / 64 KiB / ten seconds). Unsupported listing remains explicit;
an exact manual model ID is allowed. Names never certify model identity,
capabilities or readiness. No generation/probe is sent by this read.

Browse canonical families, then choose **create** or **join**. Family lists are paged;
markets use the canonical family/endpoint-family indexes. Each page is bounded to
40 entries and the existing signed cursor, with no total-catalog cutoff. Later
pages can contain compatible matches even when a current page contains none.
Joining selects a full canonical descriptor with the exact standard endpoint
contract and metering policy. This does not promise provider capacity. Creating a
market declares a model within an enabled existing family; it cannot register a new
canonical family. The local host owns the trusted peer URL and network identity.

Review reads the next provider operation sequence from authenticated canonical
state; a genuinely absent provider starts at 1. Errors are never interpreted as an
empty catalog or absent provider. Final save rechecks the exact market/family and
sequence; an already-existing exact create descriptor must be joined instead. This
draft check reserves nothing. Publication independently rechecks canonical
authority, quota and handle availability. Recovering a previously committed bundle
precedes fresh reads, including during an outage.

Context, shared concurrency, accepted rails, settlement choices and cumulative
probe allowance remain explicit operator decisions. Human prices are exact
USD-denominated decimals (1 USD = 10^18 AU); the chosen unit granularity is retained
and review displays exact AU. Probe allowances convert to micro-USD with up to six
decimal places. Excess precision, overflow and exponent notation are rejected,
never rounded; no token FX or fee is inferred. The same checked Rust conversion
serves CLI and dashboard. Nonstandard protocols still require reviewed recipes.

The shared factory validates the full generated profile and supervised runtime
configuration before an atomic private write under `<home>/proxy-setup`. Browser
requests cannot supply host paths, wallet material, Core identity, worker program,
bridge/admission origins or lifecycle callbacks. All bundle files are 0600, the
bundle is 0700. Bearer values are write-only and are not included in state, review,
logs or error responses. Request handling uses the existing dashboard session,
exact loopback Host/Origin and CSRF secret before a bounded 64 KiB / five-second
body read. The single creation permit stays with the blocking disk task until it
finishes, even if the HTTP caller disconnects. There is no additional listener or
anonymous mutation endpoint. Page/state restoration shares that same gate: a busy
restore/create returns private `503 setup_busy` from the GET routes and never
creates an empty replacement or queues another disk open. The browser retains its
session and entered choices and offers an explicit status-read retry; busy is not
presented as missing authentication and never retries a mutation automatically.

Successful creation opens the existing shared wizard: Connect → Discover → Select
→ Check → Market → Admission → Review/publish → Run. Each upstream request,
probe, invoice/checkout, publication and managed start still requires its existing
explicit action. Saving does not reserve capacity, consume probe allowance, create
an invoice, publish, pay or start a process. Existing native services are unchanged.

First creation never overwrites a destination. Reload after a lost response: the
same running host and a restarted host recover the original protected bundle. A
changed or unreadable retained host binding is an inspect-original error, not a
new empty setup. Stored prices, revisions, cumulative budget and managed child
identity continue through the same Flow/Run engines. Host-approved assets may be
removed from the initial asset list after creation because the factory imports its
own pinned tokenizer; an old bundle never silently adopts a new one.

The focused gateway tests use an actual loopback HTTP server, real protected
filesystem and the shared factory, with a synthetic wallet. They cover auth,
prebody CSRF/Host/Origin refusal, strict authority fields, unknown references,
private projections, immutable retries, commit-without-ACK recovery, restart and
handoff to actual Select/Check. They send no upstream, payment or canonical writes.
The opt-in browser fixture additionally exercises the actual first-create form.

The guided flow removes manual canonical family IDs, full market descriptors,
operation sequences and raw AU/micro-USD price entry. Remaining onboarding
prerequisites are the trusted existing host, explicit resource/charging/probe
choices and approved pinned tokenizer provisioning for LLM speed measurement.
Custom protocols still use reviewed recipes; listing alone is never a conformance
probe. This does not complete admission collection/permit renewal or claim that
saving starts serving. Those steps retain their existing separate checks/recovery.
