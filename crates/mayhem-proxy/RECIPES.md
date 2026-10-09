# Declarative JSON, streaming and asynchronous recipes

A provider can import a signed JSON connector for a different upstream JSON
interface while retaining the existing Chat, Completions, stateless Responses
or DECISIONS contract. This is a data-only extension with explicit synchronous JSON, streaming, and
asynchronous job capabilities. It does not
install code, authorize a destination, certify an upstream, publish an offer or
change prices, reservation amounts, usage units or settlement.

The locally approved `ConnectionConfig` still selects the fixed POST path,
origin, credentials, network policy, request concurrency and byte limits. Use
`error_profile: "http_status"` for custom JSON interfaces; an independently chosen
OpenAI error profile may reject an envelope before recipe translation. Recipes
cannot modify headers, URL paths, DNS permissions or dispatch/retry policy.

## Import, review and reuse

`Signed` has exactly `recipe` and `signature`. `recipe` has schema/revision,
explicit ABI range, publisher public key, exact common endpoint contract hash,
JSON bounds, request transform, outcome discriminator, response projection and
one to eight explicit conformance examples. The signature covers
`mayhem/proxy/declarative-recipe/v1`, a NUL byte and canonical stable JSON of the
recipe. Its content digest uses `Digest::hash` with the same domain over those
signing bytes. Signatures establish publisher provenance, not trust or capability.
A publisher can use its own offline Ed25519 tooling; the import path never loads
or exports a financial signing key.

Bundled synthetic signed examples live in `tests/fixtures/recipes/`. Their
publisher key is only a public test identity, never an authority or credential.
From a built local CLI:

```sh
mayhem provider proxy setup recipe inspect --recipe Chat.json
mayhem provider proxy setup recipe preview --recipe Chat.json --sample Chat-preview.json
mayhem provider proxy setup recipe export --recipe Chat.json > reviewed-recipe.json
```

The preview file has exactly `{adapter, request, response}`. `adapter` is a
version-1 same-protocol `AdapterSnapshot` containing the exact common contract,
operator-selected upstream model and local limits. The recipe must not already
be embedded in that sample adapter. Inputs are explicit local synthetic samples;
never put credentials, real user prompts or private upstream locations in a
reusable recipe or example. No file is interpreted as shell, OpenAPI references
or instructions to fetch another resource.

Inspect reports only hash, publisher, endpoint/contract, revision/ABI and mapping
fixture status. Preview displays the translated request and common result shape
with `semantic_verification: requires_supervised_probe`. It does not compile
schemas in the parent process or claim real conformance. Actual explicit setup
probes and execution normalize the response in the supervised worker, validate
original tool/output schemas there, then validate exact request-bound endpoint
semantics before delivery and independent metering. Tool argument bytes, tool IDs,
roles, choice indices, terminal conditions, decision keys/labels/probabilities and
public request identity retain the common endpoint checks. Tools never execute
on the provider host.

For durable setup, use the existing `setup prepare` input with:

```json
{"profile":{"kind":"declarative","endpoint":"chat","contract":{},"recipe":{}}}
```

Replace `contract` and `recipe` with their complete exact objects; all existing
ProfileInput network/provider/market/membership/offers/limits fields remain
required. The existing prepare/check/probe/review/publication flow derives and
pins the recipe commitment. Public setup review includes only recipe identity
and honest offline mapping status, excluding examples and private model mapping.
`Adapter::with_recipe`, `Signed::import/load/export/review/preview_file` expose the
same operations for local dashboard consumers.

## Supported language and bounds

Request transforms are `identity`, `object`, `array`, and `enum`. Object fields
map one source key to an explicit object-key target path and optional recursive
transform. Every present source field must be represented; unknown fields fail
before dispatch. Target paths cannot overlap. Array order and count are
preserved and explicitly capped; enum mappings must be one-to-one and reject
unknown values. There are no request constants, hidden defaults, dropped
controls, templates, expression execution or string interpolation. The existing
private model selection and Responses `store=false` injection precede mapping;
a recipe must represent those values too.

Response projections are `copy`, scalar `literal`, `object`, bounded `array`,
`enum`, and ABI-2 fixed `tuple` (up to 128 elements). Paths address object keys only. Optional fields are missing copy paths,
not a way to suppress type failures. An explicit string outcome discriminator is
mandatory. Success additionally requires an explicit error path to be present
and null; missing/unknown outcomes, mixed errors and missing result data fail
closed. Declared busy/rate-limit/unavailable errors are sanitized, retain
`execution: unknown`, and require original-attempt recovery. They never establish
non-execution or permission to retry. HTTP errors retain the protected connector's
existing classification. Root usage/receipt/cost projections are forbidden; only
existing independent common-protocol meters determine quantities.

Signed import/export is at most 64 KiB; preview files at most 512 KiB. Each recipe
explicitly sets JSON bytes up to 16 MiB, further narrowed by adapter/connection
limits. Each request/response program is at most 512 nodes and depth 12; paths at most 16 keys;
individual mapping objects and enum tables at most 128 entries. Runtime values
are bounded to depth 24, 65,536 visited nodes, 4,096 array entries and 512 object
entries, with shared output-allocation and serialized-byte bounds. Exceeding a
bound is an explicit failure, never truncation or an approximate successful reply.
The existing decoder process, IPC, timeout and buffer quotas remain in force.

Version-1 adapter snapshots and same-protocol digests remain byte-compatible.
The nested `Review.recipe.recipe_hash` identifies the reusable signed recipe.
The membership `recipe_hash` identifies its private execution adapter binding.
Version-2 snapshots retain the complete original signed recipe; the private
adapter hash also binds the local upstream model, limits and exact contract.
Changing a recipe creates a new identity for new work. Journal acceptance and
retained results recover the original request without redispatch, repricing,
changing a receipt or consulting current recipe files.

## ABI-2 streaming

Existing ABI-1 signed recipes retain their exact serialization and digest. A
recipe using `stream`, `job`, a `tuple`, or an empty output object explicitly
requires ABI 2. New optional fields are absent in legacy signed serialization.

`stream` declares `format` (`sse` or `ndjson`), exact `event_path`, a bounded
`events` table, `max_events`, and one to four offline frame fixtures. SSE JSON
frames become `{event,data,id}`; NDJSON JSON becomes `{event:null,data,id:null}`.
The signed table maps each discriminator to a common endpoint `data` projection,
`finish`, explicit sanitized `error`, or explicit `heartbeat`. Unknown events
fail closed. Chat/Completions use an explicit finish marker; Responses terminate
only through a verified `response.completed` or `response.incomplete` event.
No URL, expression, dynamic event name or tool execution is interpreted.

The supervised worker performs normalization with cumulative byte and event
bounds. The existing common endpoint assembler then checks order, stable IDs,
choice indices, tool argument fragments and terminals; the worker verifies the
assembled final result against the original request's schemas before retention.
Only the existing independent meter determines observable quantities. Stream
fragments remain provisional, and local cancellation never certifies remote
nonexecution. Signed examples `Chat-sse.json` and `Chat-ndjson.json` can be
inspected/exported with the existing CLI. Offline fixtures establish mapping
consistency, not upstream conformance or financial authority.

## ABI-2 asynchronous jobs and recovery

`job` declares exact submit HTTP status (200/201/202), `job_id_path`, bounded
`poll_interval_ms` and `control_timeout_ms`, `status_path`, explicit status map,
`poll`, `result`, and nullable `cancel`/`lookup`. Each control contains a
`job_id_field` and nullable `response_id_path`. Status values are `pending`,
`ready`, `cancelled`, `failed`, or `missing`. All required fixed operations must
exist in the operator's local `ConnectionConfig.paths`: `job_poll`, `job_result`,
optional `job_cancel`, and optional `job_lookup`. They inherit the same origin,
credential, network policy, concurrency, and connection fingerprint. Recipes
cannot add operations to a connection. Control responses are bounded to 16 KiB;
result bytes retain the configured adapter/worker bound. Poll cadence is at least
one second; a control step is at most 60 seconds. No total customer generation
deadline or automatic resubmit is introduced.

Submit crosses the existing durable financial/capacity dispatch fence once.
A job-ID-only acknowledgment is sufficient for exact authenticated control
requests. The returned opaque job ID is persisted before polling. If the API
supports original-key recovery, `lookup` explicitly names `submit_key_field`,
`lookup_key_field`, and required echoed `response_key_path`; the deterministic
key binds invocation, attempt, request, recipe and connection. Missing or lost
acknowledgment without that capability remains unresolved. Even a missing lookup
result never authorizes resubmitting inference. The `Chat-async.json` example
illustrates the public signed shape without a real destination or credential.

Journal schema 9 adds one small control record per asynchronous attempt, charged
1 KiB against the existing payload quota before dispatch. Indexed exact reads,
durable generation leases, next-poll times, and a commit-before-send cancellation
flag prevent concurrent/stale recovery and non-idempotent cancel replay. Existing
unfinished-attempt maintenance calls the same bounded `Executor::resume_job`
step. Recovery restores the immutable accepted adapter/request and verifies the
original connection revision/fingerprint; a new recipe, rate, or target never
replaces them. The completed original result passes the same worker, endpoint
and meter before a fenced atomic result commit. Existing receipt and buyer
reconciliation retain monetary authority.

`cancel` explicitly declares whether the per-job operation is idempotent. Its
ACK alone proves neither stopped execution nor financial closure. A cancelled
or failed job status without a fully verified endpoint result remains unresolved;
capacity and charges are not released by a status string or control lease expiry.
A late verified completed result is retained under the original attempt even
after cancellation. Verified partial/cancelled settlement remains a separate D2
acceptance requirement. Exact-result recovery does not mutate settled charges.

Explicit setup probes reuse submit/poll/result, the same supervised verification,
and their existing timeout/cost/capacity allowance. Interrupted/ambiguous probes
remain retained and occupied; no replacement probe is started automatically.
Customer-job restart recovery uses the durable attempt journal. Asynchronous
probe-job restart lookup/cancellation is not implemented: setup must retain its
uncertain probe and use existing explicit reconciliation. Probe conformance
classes bind the original common request before custom field translation.

## Remaining scope

Dynamic question-key dictionaries can be copied intact; converting arbitrary keys
into array entries or back requires a future explicitly bounded operation.
OpenAPI/example assisted drafting and expanded language defaults remain unsupported.
Streaming and async execution are separate capabilities: an async recipe cannot
claim that polling a buffered result is streaming. DECISIONS streaming remains
unsupported. Dynamic per-job URLs, GET query/path interpolation, arbitrary raw
SSE payloads, and executing upstream-returned links are unsupported; local fixed
POST control operations carry the opaque ID in a JSON body. Recipes do not prove upstream
model identity, rights, privacy, tool support or authoritative capabilities.
Explicit probes and the existing reviewed serving/admission path remain required;
no automatic paid probes or arbitrary remote code are introduced.

Focused checks from the Core workspace:

```sh
cargo test -p mayhem-proxy --test recipes
cargo test -p mayhem-proxy --test execution declarative::
cargo test -p mayhem-proxy --test setup profile::
cargo test -p mayhem-cli --bin mayhem proxy_provider::setup::recipe_tests
```
