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

## Read-only maximum estimates

`POST /v1/proxy/estimate` requires a current authenticated key with permission for
the selected model. Its strict body is `{ "schema_version": 1, "endpoint":
"openai_chat_completions", "request": { ... } }`. `endpoint` also accepts
`openai_completions`, `openai_responses`, and `mayhem_decisions`. `request` is the
exact intended inference envelope, including its explicit `proxy` controls.
The selected execution gateway supplies the configured settlement policy; no
rail, offer, price ceiling or output allowance is selected automatically.

The response is private and not cacheable. Its fields are `schema_version: 1`,
`lane: "proxy"`, `kind: "maximum_estimate"`, `network`, `model`, `endpoint`,
`rail`, `request_hash`, `request_content_digest`, `controls`, the full `offer`,
`offer_digest`, `membership_digest`, `endpoint_contract`, `recipe_hash`,
`settlement_policy_hash`, `metering_policy_hash`, `max_usage`, `max_spend_au`,
`observed_at_ms`, `expires_at_ms`, `availability`, and `estimate_hash`.
`endpoint_contract` is the canonical contract hash, not a contract body.
`availability` contains the same routing `status`, its `observed_at_ms`, and
`expires_at_ms` (null when no current evidence exists). These observations can
report unavailable capacity alongside a valid arithmetic maximum. Expiry bounds
the original evidence; availability can change sooner and is never a lease.

`max_usage` and the decimal-string AU maximum use the same preparation and cost
function as `PreparedPurchase`, including per-unit rounding, one request fee,
session minimum, and the explicit total cap. Generative input units are exact
normalized billing quantities, not native tokenizer tokens; output units are
the explicit maximum allowance, not an estimate of likely model output or the
upstream `max_tokens`. Decisions reserve the validated question count. Final
usage, final wholesale cost, retail fees and exchange rates are not predicted.

`request_hash` is the existing provider request fingerprint after removing the
root `proxy` controls. `request_content_digest` uses the existing retail callback
typed SHA256 algorithm on that same provider body. `estimate_hash` uses BLAKE3
derive-key domain `mayhem/proxy/maximum-estimate/v1`, followed by the little-endian
u64 byte length and `stable_json_bytes` of the entire response excluding
`estimate_hash`. It binds the explicit controls and all returned facts; it is
neither a signature nor a payment authorization. Execution still resolves fresh
canonical state. Clients requiring exact quoted revisions must check the full
offer digest and all request/policy/usage bindings before granting admission.
Existing paid requests must recover their original terms before applying a new
estimate or current-catalog checks.

Custom endpoint contracts are fetched through `p.describe.open` / `p.describe`
on the existing authenticated peer transport. The reply contains only the public
contract, opaque recipe hash and supported metering definition, verified against
canonical membership/offer hashes. Buyer resource limits stay local. Descriptor
traffic uses separate read permits, a five-second maximum deadline, and bounded
192 KiB messages; it never enters proposal, capacity, signing or execution paths.
Older peers that do not support it return an unavailable estimate. No fallback
creates a proposal. HTTP input is independently bounded before JSON parsing.

Estimate failures use category `proxy_estimate`: `proxy_estimate_invalid` and
`proxy_settlement_policy_mismatch` are 400; `proxy_buyer_disabled`,
`proxy_estimate_busy`, and `proxy_estimate_unavailable` are 503. Normal key errors
retain their existing codes. None creates a job or returns a job ID.

## Authenticated financial evidence

`GET /v1/proxy/buyer-policy` requires a current authenticated gateway key and
returns exactly `schema_version: 1`, `settlement_policy_hash`, and the full public
`settlement_policy` currently configured on that gateway's buyer runtime. The
hash uses the existing `mayhem/proxy/settlement-policy/v1` canonical digest.
An unconfigured buyer returns `503 proxy_buyer_disabled`; no default policy is
invented. Responses are private and not cacheable. Private configuration,
credentials and resource limits are never included.

Clients using separate gateways per payment rail must read the policy from the
same gateway that will execute their request. This read describes approved
settlement outcomes; it does not promise readiness, funding or provider agreement.
Admission independently rechecks the explicitly supplied policy hash and frozen
terms. A later policy change cannot reinterpret an existing purchase.

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

## Trusted retail credit admission

An optional `retail_authorization` object in the protected buyer configuration
contains `url`, `credential`, `owner_token_ids` and `timeout_ms` (1000–5000).
The URL is fixed by the operator: HTTPS, or HTTP at a literal loopback address;
userinfo, query strings and fragments are rejected. The client has no redirects,
retries or system proxy, permits eight concurrent callbacks and reads at most
8 KiB of response. Credentials are never request fields or diagnostic output.
Other gateway token owners continue using their ordinary Core authorization.
Retail dispatch must send `X-Mayhem-Require-Retail-Authorization: 1`. The proxy
branch rejects any other header value; if the configured hook is missing or the
key is outside its allowlist, it fails before job creation or spending. An
allowlisted key always uses the hook even without this additional requirement;
the header cannot opt out. Native handlers are unchanged.

For an allowlisted token, the original `Idempotency-Key` is mandatory and is the
retail `request_id`. Callback participation, this reference, the request content
digest and the fixed authority URL bind the gateway fingerprint. There is no
public skip/approval flag; credential rotation alone does not alter that binding.

The controller has already computed `max_usage` through its existing endpoint
and metering implementation and priced it against the exact frozen offer.
The composite gate first completes normal `Owner.authorize`: durable Core terms
and common key-budget exposure. It then POSTs exactly:

```json
{"schema_version":1,"request_id":"original-idempotency-key","job_id":"original-core-job","terms_hash":"hex-digest","request_content_digest":"hex-digest"}
```

The trusted retail service must authenticate this machine callback, read the
owner-authenticated `/v1/jobs/{job_id}/proxy-evidence`, bind it to its immutable
original request/owner and price the exact maximum with its pinned margin and
rounding policy. It reserves that exact amount at or below the customer's stored
cap; the cap itself is not a credit hold. Pinning and the credit hold must be
atomic/idempotent, with no database lock held across the evidence HTTP request.
Only a committed sufficient hold may return the exact acknowledgment:

```json
{"schema_version":1,"job_id":"original-core-job","terms_hash":"hex-digest","authorized":true}
```

The callback does no inference. Only after both gates succeed may the controller
sign, obtain provider acceptance, publish the canonical reservation and Execute.
Quote freshness is still checked at signing. This is a bounded machine handshake,
not a place to wait for human approval or to renew prices/lifetimes. Decline,
malformed/foreign acknowledgment, disconnect cancellation or timeout fences the
original unsigned intent; a lost reply may still mean the retailer retained a
hold. The retailer must reconcile authenticated evidence, never infer release
from a timeout. If Core rejects before calling retail there may be no retail
hold at all. Recovery does not call the callback or dispatch Execute again.

`request_content_digest` hashes the provider body after removing only the `proxy`
envelope; model, streaming and every other provider-request field remain bound.
It is separate from existing protocol hashes and does not tokenize the request.
SHA-256 input is the ASCII domain `mayhem/proxy/retail-request-content/v1` followed
by a zero byte, followed by this typed encoding (root depth zero, maximum 64):

- Null, false and true: the single ASCII bytes `n`, `f` and `t`.
- Number: `d` and eight big-endian IEEE754 binary64 bytes; normalize negative zero
  to positive zero. Reject nonfinite values and integer values that cannot be
  represented exactly as binary64. The Rust integer check uses wider integer
  round trips so the `u64::MAX` saturation boundary cannot pass.
- String: `s`, an eight-byte unsigned big-endian UTF-8 byte length, then UTF-8.
  JavaScript implementations reject unpaired surrogate code units.
- Array: `a`, an eight-byte unsigned big-endian element count, then each value.
- Object: `o`, an eight-byte unsigned big-endian entry count, then encoded
  key-string/value pairs sorted by the keys' UTF-8 bytes. Do not reconstruct a
  JavaScript object after sorting, which can reorder integer-like keys.

Cross-language golden values are in
`src/openai/proxy_buyer/retail/retail-content-v1.json`. Ordinary JSON stringification
is not this encoding: float formatting and UTF16/integer-key ordering differ.

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
