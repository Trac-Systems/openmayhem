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

No Flow, Connection, Probe or runtime-policy JSON is required. The form collects
endpoint URL, compatible protocol, exact upstream model, explicit destination
network permission, canonical family/model declaration, context/concurrency,
accepted rails and full price map, settlement choices, cumulative probe allowance,
recovery permission and local completed-journal retention. Prices are exact AU;
no exchange rate or financial policy is filled automatically. The initial form
creates a new market; joining an exact existing market remains supported by the
protected profile path. Nonstandard protocols use reviewed custom recipes.

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

This is the protected first-create connection, not completion of the plan's easy
onboarding UX. Remaining guided work includes discovering models before requiring
a model selection, authenticated family/market selection for create **or** join,
reading the next canonical sequence, human price/allowance entry with explicit
exact conversions, and approved tokenizer provisioning. The current form honestly
requires those identifiers, exact units and host-provided tokenizer prerequisite;
it does not replace them with guessed identities, prices, capacity or evidence.
