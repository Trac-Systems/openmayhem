# Supervised provider sessions

`serving::Controller` assembles proposal negotiation, paid execution, retained
results and receipt delivery from the same owned runtime, journal, wallet,
financial client and isolated worker pool. It accepts a provider-authenticated
`negotiation::Channel`, or consumes an authenticated opening through `accept`.
It is not a signing endpoint. The opt-in dispatcher below owns incoming openings.

## Authenticated dispatch

`negotiation::opening::Listener` subscribes through the protected loopback bridge.
It ignores native/other frame tags and validates the bounded opening's authenticated
peer, session, transport lineage, provider and network. An `Incoming` cannot be
deserialized or cloned; it expires if held beyond the configured control wait.
`Controller::accept` reserves session/per-buyer control ownership before connecting
the dedicated session socket and replying Ready. The listener never sends that reply
or acquires model-session ownership. `Channel::dial` waits for a matching context
digest and session before sending Request or Recover on the same connection.
Ready means the negotiation handler exists; it is not financial admission, an
inference-capacity measurement or permission to dispatch a model request.

`serving::dispatch::Dispatcher` accepts only trusted runtime registrations, indexed
by market, endpoint, context bracket, outcome class and metering policy. Rates and
revisions are checked by canonical negotiation and retained recovery, not used to
redirect an old purchase to new terms. Wrong-identity/endpoint and duplicate
registrations are rejected. Registration count, aggregate session owners and each
controller's quotas are bounded. Rejected openings create neither a model request
nor a financial hold; the buyer's finite opening wait still applies.

Several registrations sharing one controller start only one independent maintenance
runner. The dispatcher drains incoming frames while sessions negotiate or execute,
tracks coalesced control/recovery health and closes connection owners gracefully on
shutdown or bridge failure. Started JSON executions and recovery pages retain their
existing durable completion rules. Automatic CLI/mayhemd startup is still required;
the presence of this library does not publish or activate any provider.

## Ownership and bounded I/O

Session and per-buyer quotas are separate from inference capacity. One logical
invocation has one active connection and at most one execution owner. The paid
connection uses independent read/write tasks on one authenticated bridge socket,
with bounded message counts and serialized-byte reservations. Backpressure on
stream delivery does not prevent the control reader from receiving cancellation.
An acknowledgment confirming settlement is flushed before closing the transport.

Negotiation and idle-control waits are finite and configurable. They are not
generation deadlines. A reconnect observing an existing execution can query or
cancel it; another Execute cannot create another upstream POST. Cancel records
durable intent before signalling execution. Lost transport never proves a sent
request stopped, frees unknown capacity, or authorizes a waiver.

A disconnected JSON execution retains its supervised owner through completion so
its result can be recovered. Interrupted streams retain their existing uncertainty
rules. Result/receipt recovery, receipt verification and canonical financial
publication remain distinct. The controller preserves typed invalid-request,
capacity, recovery and financial-admission failures without disclosing credentials,
private endpoint addresses or raw upstream errors.

## Independent maintenance

`Controller::maintenance` allows one recovery runner per controller. Each step
rotates through unsigned proposals, signing intentions, canonical retirement and
retained execution outcomes. Independent seek cursors and pages of at most 64
records prevent one unresolved record from starving the other sources. The
runner uses the existing jitter/backoff schedule and coalesced health snapshots.
Graceful shutdown finishes a started bounded step.

Background execution recovery reads small record/financial/payload metadata first,
not entire prompts and results. A result-present hint grants no payment, delivery
or capacity authority. Actual capacity release still validates durable execution
evidence; already-released capacity skips that payload read. Publication retries
only an already-retained, independently countersigned receipt or waiver. It does
not invent an outcome, resend inference or change native provider payout workers.
Closed-record pruning is indexed and bounded. Unknown work cannot age out.

## Passive health and adaptive allowance

`health::Monitor` observes one declared upstream connection/credential scope and
its registered routes. The provider declares an allocation ceiling. Fresh validated
inference starts an allowance of one; configured successful observations increase
that allowance one at a time, never above the ceiling. This is an adaptive admission
budget, not proof of the remote scheduler's physical concurrency or a reservation
against unrelated clients. Durable `capacity::Authority` separately subtracts active
and uncertain work once across declared shared groups and route aliases.

`Executor::with_observations` opts actual JSON/stream execution into passive timing
and typed-failure observations. It adds no POST, retries, financial action or per-token
journal reads. Invalid caller input, client cancellation and local infrastructure
failures do not become backend health faults. Readiness is recorded only after full
endpoint/schema verification. A malformed model response and upstream busy/quota/auth
errors retain their actual scope; a fresh fault remains visible before first success.

Evidence has its own age, independent of heartbeats. Expired evidence allows no new
work. Recovery is jittered/backed off, respects longer Retry-After values, requires
fresh inference evidence and never cancels valid ongoing work. An earlier in-flight
success cannot clear a later failure. Relative latency deterioration for comparable
request classes reduces allowance without treating one slow request as a dead server.
Classes separate serialized size, stream mode, thinking/effort, output limits and
output format; this is not a native input-token estimate. Class/route retention is
bounded and contains no prompt or output text.

Headers, first meaningful output and completion are timed separately. SSE event counts,
upstream usage and billable units do not certify native tokens/second. A separate trusted
native-tokenizer observation API preserves tokenizer identity and measurement age;
buffered single-timestamp output remains unknown. This tokenizer source is not yet
connected to execution, so ordinary observations cannot certify the generation floor.

`capacity::Authority::bind_live` now connects memory-only readiness sources to actual
proposal reservation, provider signing and dispatch. Sources preserve the observation's
age; reads/heartbeats never renew it. Revision checks reject an inconsistent group/route
pair without spinning. Restart, missing sources, source errors and expired evidence stay
closed, and neither configuration changes nor old observation tickets can silently fall
back to cached Ready. Local capacity schema 5 preserves earlier leases without scanning
or rewriting their history. This is not a ledger-contract version change.

`Controller::new_observed` feeds validated JSON/stream outcomes into the monitor chosen
by trusted startup. It neither attaches guessed scopes nor manufactures initial health.
Bind a connection source only to that exact shared API/credential pool. A broader
physical pool containing independent credentials or native runtimes must not inherit
one credential's authentication/quota failures.

`configure_route_with_constraints` adds bounded overlapping groups to the route's
existing primary group. The primary binding in signed market terms remains unchanged.
Observed groups require fresh evidence from their exact scope; allocation groups created
with `configure_allocation_group` supply only a physical/operator ceiling. Allocation
alone never makes a route Ready. The route's own health and every relevant constraint
must allow admission. Free capacity is the minimum remaining allowance across them.
Credential faults therefore stop that credential's routes without withdrawing unrelated
credentials or native work sharing the physical backend.

One transaction reserves each request once against every relevant group and its route.
Signing and dispatch recheck those constraints without subtracting the reservation twice.
Occupied routes cannot move groups or lose constraints. Lower ceilings preserve existing
work and prevent new admissions until counters fit. The older route configuration method
preserves additional constraints, so a routine ceiling update cannot erase them.

Verified completion releases all corresponding counters and indexes atomically. Completed
inference need not retain a physical slot while its payment acknowledgment is pending;
the durable result and financial obligation remain separately recoverable. A disconnect,
timeout, cancellation intent or health recovery alone cannot free uncertain execution.
Restart retains all counters and fences old controllers. Recovery merges bounded indexed
pages for primary and additional memberships; it never scans whole request histories.
Missing/mismatched references fail closed without partially decrementing counters.

Signed public presence and managed startup remain unwired. Numerical public policy and
end-to-end freshness/recovery bounds require acceptance.

## Operator recovery probes

`execution::probes::Controller` runs an explicitly configured model request only when its
shared health monitor permits recovery. It uses the same protected HTTP connection,
isolated decoder, endpoint validation and tool/schema checks as paid JSON and streaming
execution. A distinct decoder session and consumable probe ticket keep this traffic
separate from customer purchases. There is no buyer, receipt, demand event or ledger write.

The operator supplies cumulative attempt/cost allowances, a conservative per-attempt cost
estimate, request/output byte limits, an explicit LLM output-token limit and a probe-only
duration. These are permissions to use that operator's upstream account, not customer
credits, proof of an upstream balance or a guaranteed vendor price. Reconfiguration and
restarts preserve used allowance. Renewal requires an explicit increase. Even a cancelled
unsent attempt conservatively consumes allowance; no hidden refund or periodic reset.
Missing/exhausted allowances prevent a POST. There is no automatic idle probing loop.

The capacity database commits budget consumption, one probe per overlapping shared group,
and all physical/route counters together. Only excluded health gates for the tested route
and its selected recovery group may be bypassed. Healthy adaptive allowances, unrelated
failed scopes, configured ceilings and outstanding work remain binding. Health recovered
by ordinary traffic or a newer fault invalidates a waiting probe before dispatch. All
blocking storage work has bounded async admission and retains its permit while fsync runs.

Startup failure may cancel a durably Prepared probe. Dispatch commits before POST and
cannot be replayed by deserializing its status. Drop, timeout or restart preserves unknown
execution and occupied counters; reads never create a new dispatch permit. A validated
terminal result or proven pre-dispatch transport refusal closes capacity. The completion
record retains one bounded evidence digest per allowance, not model output history.
Network timing excludes decoder startup and local persistence. Health success is published
only after durable completion; publication delay does not inflate the recorded model latency.

The generic error profile deliberately preserves uncertainty after an arbitrary HTTP
error. In particular, a 429-shaped body is not independent proof that a vendor executed
nothing. The explicit [vLLM admission profile](REFUSALS.md) now closes operator probes
for its verified single-request HTTP admission refusals, with backoff and consumed budget
preserved. Generic errors, started streams, other connectors and paid refusal closure still
require outcome recovery; an unknown probe cannot simply expire.
Prepared-probe recovery is available to trusted startup, but managed startup has not yet
been wired. The monitor and capacity authority must be rebound to their exact scopes.
The operator can explicitly raise a spent allowance; no implementation may silently reset
it or delete an unresolved record to keep checking.

These self-tests do not certify global concurrency, all context sizes, upstream model
identity or native tokens/second. The independent tokenizer/progress source remains
unconnected, and neither JSON usage fields nor streaming chunk counts certify the 5 tok/s
floor. Probe recipes must match the condition under review before declaring recovery.

## Evidence and remaining integration

Local acceptance covers four JSON endpoints and all three payment rails, all three
LLM streaming endpoints, missing funding, reconnect without duplicate inference,
cancellation, generation outliving the control wait, malformed requests, quotas,
pending receipt publication without a connected buyer and canonical non-admission
retirement. Queue checks cover byte/count pressure, cancellation, disconnect and
actual writer acknowledgment. Fixtures use the real local signed financial RPC,
HTTP backend and isolated workers, with a bounded SC-Bridge protocol double.
Layered allocation is exercised through paid signing/execution/recovery on every supported
endpoint and rail, plus simultaneous native/proxy/alias admission, independent credential
failures on the same physical allocation, schema migration and corrupted recovery indexes.
These are local authority tests; registering actual managed native runtimes is still work.

The opening/dispatcher checks also exercise all four paid JSON endpoints/three
rails, malformed/foreign/oversized contexts, substituted readiness, finite opening
waits, duplicate/global quotas, submarket selection, independent recovery ownership
and graceful stop. The SC-Bridge double now honors multiple clients and per-session
subscriptions; it still does not prove real Noise/relay behavior.

Automatic CLI/mayhemd startup, adaptive capacity, public availability and real-network/
payment acceptance are still required. Resource sizing must include reconnect and
recovery control headroom: retained execution owners count against the aggregate
bound. These library tests are not a deployed serving claim. The Core/site release
and fleet activation gates remain separate.
