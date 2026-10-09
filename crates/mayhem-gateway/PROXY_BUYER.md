# Proxy buyer integration

This candidate integrates an explicitly enabled paid proxy buyer with durable
HTTP jobs, key spending limits and the existing proxy purchase protocol. It is
not a production activation. The read-only directory and
`mayhem use --proxy-config` alone do not authorize spending.

Native and proxy requests share a durable key budget once provisioned. Their
model identities, routing, accepted prices and financial protocols remain
separate; a proxy request cannot fall through to a native model.

`openai::proxy_request` resolves exact offers independently of native model names.
Its internal candidate model selector is
`proxy/offer/<market digest>/<provider public key>/<offer slot digest>`. Friendly
market/category selectors remain separate work. A malformed proxy selector never
becomes a native selection.

The candidate request envelope has a `proxy` object containing the complete
`prices` rate map, per-request fee cap, session-minimum cap, total-spend cap,
payment rail, pinned settlement policy, output allowance and optional minimum
context/throughput and verified-operator requirement. The gateway strips only
that envelope before passing the owned request to the provider protocol. This
syntax is a candidate interface; public retail, Studio and MCP integration and
release acceptance remain separate requirements.

## Authenticated financial evidence

`GET /v1/jobs/{job_id}/proxy-evidence` returns schema version1,
`object: "mayhem.proxy.job_evidence"`, for the authenticated key's original
proxy purchase. Native jobs have no proxy evidence. Current model scope, key
expiry/revocation and owner checks still apply; an exhausted spending cap does
not hide an already purchased result. The response is private and not cacheable.

The response binds the job/model/endpoint, original request fingerprint and
billing/session identity, frozen terms and their digest, complete provider rates,
rail, settlement policy and signed acceptance. Model output, prompts, upstream
configuration and signing secrets are not included. This route is the evidence
source: fields named `receipt` or `financial` inside a model answer have no
financial authority.

The `financial.kind` discriminator deliberately separates:

- `pending`: includes unknown execution and incomplete admission recovery. Keep
  the original obligation; a missing journal row is not a refund proof.
- `not_authorized`: the exclusive owner retired the intent before granting
  authorization. No paid execution was dispatched.
- `non_admission`: a durable local signing fence, with its commitment and exact
  terms digest/maximum. It is not a canonical payment receipt. Wait for
  `budget_settled: true` before treating owner accounting as complete.
- `canonical`: signed paid receipt, mutual waiver or expired-unknown closure,
  plus the retained canonical observation identity and budget-completion marker.
  The observation is retained evidence, not a claim of current network freshness.
  A zero-charge expired-unknown result does not authorize retrying execution.

Retail consumers must bind the response to their original job, selector, endpoint,
rail and accepted policy, retain first-observed identity/terms, reject unexpected
schema or changed bindings, and use decimal integers for money. Missing or
unavailable evidence cannot justify another purchase or a credit release.
Read/verification uses bounded concurrent storage work and a direct job lookup;
it neither copies the completion body nor scans ledger/receipt history.

This evidence interface is an integration prerequisite. It does not by itself
activate retail proxy requests, Studio or MCP.

The gateway owner supplies a separate resolved policy revision and explicit
epoch lifetimes. Request identity includes the authenticated buyer/key owner,
endpoint, exact selector, full request, every price/rail/filter control and that
policy. Reordering JSON does not change the identity. Identical text from another
invocation is not an idempotency key; the caller must bind its opaque key to this
fingerprint and preserve the original billing/session identity across retries.

Candidate resolution reads the current indexed catalog and the same signed
presence eligibility used by the proxy control plane. It does not reserve a
slot. Prices, accepted rail, advertised served context, endpoint and exact
membership/recipe must match. The current directory has no authenticated T4
evidence, so a verified-only request fails closed rather than trusting an
operator label. The ordinary LLM throughput floor remains in shared presence
eligibility; Decisions do not invent a token-speed guarantee.

At most eight candidate reads run concurrently. Each permit remains with its
blocking storage operation if the HTTP caller disconnects. There is no
all-catalog subscription, history scan, admission queue or per-output-token read.

## Durable dispatch ownership

The explicit buyer runtime owns:

- Protected durable HTTP job/idempotency → billing attempt, supplier, endpoint,
  policy, rail and authenticated key attribution.
- Durable key-budget exposure before a buyer signature can leave. Reserve the
  actual accepted terms' maximum cost, not an unrelated caller spending ceiling.
  Unknown signing/execution outcomes retain the original exposure until resolved.
- A separately enabled buyer controller, existing wallet authority, protected
  negotiation/recovery journals, bounded session and decoder resources, and
  joined shutdown. Discovery alone must never enable paid work.
- Durable verified output before acknowledging its receipt. Recovery of a
  canonically paid purchase reads the saved answer; a paid receipt without an
  answer is not a successful response and must not trigger a second purchase.
- One bounded recovery supervisor covering pre-publication negotiation as well
  as published reservations. Reconnect using the original Recover/Status path;
  do not resend Execute or fall through to a different rail/native provider.

`mayhem_proxy::buyer_controller` supplies the owned negotiation/execution layer.
It rechecks the provider proposal and a fresh canonical quote, persists signing
and acceptance, confirms funding, independently verifies output/receipts, and
requires explicit owner hooks for budget authorization and result retention.
The gateway implements those hooks in its encrypted job vault and durable common
key-budget journal. Result retention must precede durable receipt approval as well as signing: a
separate recovery worker can sign a retained approval. The controller verifies
without approving, retains the answer through the owner hook, then rechecks
canonical state before storing approval. Both paid receive paths reject receipts
or waivers that name a different purchase or invocation.

Nonstreaming Chat, Completions, Responses and Decisions use the common purchase
path. Endpoint support is conditional on the actual offer/recipe, not the name
of the model. Streaming Chat, Completions and Responses use the same owned
purchase path; Decisions streaming is rejected before authorization. Category
routing, retail accounting and Studio/MCP invocation remain integration work.

These paths preserve the current admitted endpoint contracts, not full upstream
API compatibility. The default Completions contract accepts `prompt` and its
declared generation controls but rejects `suffix`. The default Responses
contract accepts `input` and its declared controls; it does not admit
`instructions` or caller-supplied `store`, even `store: false`. The provider
adapter forces `store: false` on its own stateless upstream request and rejects
vendor-side history, conversation and background execution. Richer fields need
explicit contract/profile admission, metering review for every prompt-bearing
field and end-to-end verification before support can be claimed.

## Public proxy streaming

Send `stream: true` to the explicit proxy Chat, Completions or Responses route.
The first response has `Content-Type: text/event-stream`, the original
`x-mayhem-job-id` and `Location`, and `Cache-Control: private, no-store`.
`Prefer: respond-async` with streaming is rejected before admission; async
nonstream requests remain supported.

Deltas are provisional. The provider normalizes upstream SSE, and the buyer
independently checks event identity, ordering, bounded content and agreement
with the verified final result. The gateway directly polls that bounded channel;
it does not turn a buffered JSON answer into a pretend live stream. Function
arguments and other provisional text must not trigger tool execution or be
interpreted as financial evidence.

The channel permits eight queued events. Its per-event, aggregate queued-byte
and total provisional-byte bounds equal the controller's configured response
byte limit; message/frame limits and negotiated output allowance remain enforced
independently. Event count is bounded by that byte limit (every encoded event
costs at least one byte), rather than an unrelated fixed count. Queue capacity
is separately charged against the controller's configured buffer budget. Each
open observer holds a separate session permit until its body finishes or is
dropped, including after execution ends.
Terminal metadata is generated lazily from the bounded retained result; no chunk
history is stored. These are application buffer bounds, not a total process RSS
or operating-system socket-buffer guarantee.

Chat/Completions finish chunks and `[DONE]`, and Responses `.done` and
`response.completed`/`response.incomplete`, are withheld until verified output is
durable, canonical financial closure is observed and the exact key-budget
settlement is durable. The terminal response usage is the verified normalized
usage; model-supplied fields never supply financial closure. On unresolved
closure the SSE emits an `error` event with `proxy_recovery_required`, the
original job ID and recovery URL, then ends without a success terminal.

Disconnect, cancellation and shutdown stop delivery and join the owned purchase;
they do not prove non-execution, refund a hold or dispatch another Execute.
Use the original job URL or original idempotency key to recover. An identical
streaming replay returns pending job JSON (202) or the retained endpoint JSON
(200, `application/json`), with the same job headers. It never restarts SSE,
replays old fragments or splices recovered content into a previous stream.
Changing `stream`, body, price, rail or policy under that key conflicts. Historical
fragments are not journaled, so restart recovery verifies only the original
retained full result, not a persisted transcript of earlier deltas.

Local acceptance covers all three streaming endpoints on FIAT/TNK/TAP using the
real gateway router, buyer/provider controllers, isolated decoder and signed
canonical ledger fixture. It includes a delta delivered before a gated upstream
tail, delayed receipt publication, original-result recovery, exact replay,
disconnect and backpressure cancellation/shutdown. These fixtures use a bounded
bridge double; they are not real Noise-relay, live-model or mainnet evidence.

## Explicit CLI activation

Provisioning and serving are separate operations:

```sh
mayhem proxy buyer-init --config buyer.json --home /path/to/gateway-home
mayhem use --proxy-config discovery.json --proxy-buyer-config buyer.json
```

Use the same gateway home/wallet when serving. Stop all older gateway binaries
using that home before provisioning: old binaries cannot honor the new migration
lock. Current binaries hold a shared serving lock; provisioning requires an
exclusive lock. No inference, signature or payment is started by `buyer-init`.

The protected configuration explicitly binds the network/bootstrap/contract,
buyer public key, canonical peer RPC, local authenticated bridge, decoder
executable, state directory, settlement policy/revision and epoch lifetimes.
It also specifies session, buffer, protocol, worker, storage and retained-record
bounds. These are operator controls, not request fields. No private signing key
belongs in this file: serving uses the unlocked gateway wallet. Relative paths
are resolved beside the configuration file. Current paid-buyer file protection
requires the supported Unix ownership/permission checks.

Provisioning imports existing key counters once and creates bounded negotiation,
recovery and budget journals. It then writes a durable activation pointer and
version-2 token configuration. Serving strictly reopens all retained stores:
missing, empty, foreign or uninitialized stores are errors, never an instruction
to reset accounting. Interrupted provisioning requires explicit reconciliation.
Do not remove its stores or pointer to make startup pass.

After activation, a native-only restart also opens the durable budget authority.
Removing the proxy flag cannot silently restore stale spending counters from
JSON. The activation pointer and token version must agree. New reservations
atomically include both native and proxy exposure. Revoking a key prevents new
work and response access, but does not erase its financial obligations. An
exhausted active key can retrieve its existing paid answer without buying again.

## Recovery and limits

HTTP disconnect does not abandon the owned purchase. Async requests return the
same job identity; status, cancellation and idempotent retries refer to that
original identity. A completed replay returns the retained endpoint response,
not a replacement job or a new Execute. Changed request bodies, accepted limits,
rail or operator policy conflict with the original idempotency binding.

Recovery reads a bounded page of pending owner jobs every two seconds, uses the
same bounded session permits and reconnects through Recover/Status. Native
receipt restoration uses bounded retained local history at startup; neither
path scans ledger receipt/price history per request or per output token.
Shutdown stops admission and joins both purchase-controller and owner work.

Budget rejection or interrupted signing cannot be resolved from a missing buyer
row. The controller retains an unsigned intent before calling the budget owner,
and signing and permanent non-admission fencing are mutually exclusive durable
transitions. Only that opaque fence may retire a matching never-admitted hold.
Once signing may have happened, canonical financial closure remains necessary.
Fence retention is bounded and capacity exhaustion fails closed; it must never
silently forget outstanding obligations or permit duplicate signing.

Acceptance evidence distinguishes local controller/HTTP fixtures from real
model, bridge and payment-rail acceptance. Successful fixture settlement does
not establish production readiness. The release and rollout approval gate
continues to apply.
