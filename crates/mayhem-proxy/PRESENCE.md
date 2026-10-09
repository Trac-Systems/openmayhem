# Signed proxy availability

Managed provider startup publishes `proxy.hb` on the existing authenticated
SC-Bridge sidechannel transport. This distinct signature format makes no native
enclave claims and leaves the native validator unchanged. Messages do not append
ledger transactions, authorize charges or reserve inference capacity.

Channels are separated by network/contract and market. Receivers subscribe to
selected markets with explicit local quotas, not an all-market feed. Publishers
subscribe to no inbound traffic. Receiver payload-filter clearing affects only
that connection, and channel restrictions remain enforced. Bridge access requires
authenticated literal-loopback transport, bounded queues/messages and operation
deadlines. Deployment must permit these public proxy channels in its sidechannel
policy; other channels' invitation requirements are not disabled.

## Evidence and timing

Only configured offers are read through the canonical provider-offer RPC. A
configuration file plus healthy upstream cannot publish availability: fee
admission, membership, exact offer, network and payout linkage must first pass
canonical observation. Background reads have separate bounded permits from paid
acceptance/settlement. Observation tries other advertised rails if one lacks its
payout link. Availability does not guarantee every rail can currently settle;
negotiation still validates the specifically selected rail.

| Bound | Candidate behavior |
|---|---|
| Ordinary heartbeat | Every 2 seconds, matching native providers |
| Maximum wire TTL | 60 seconds, matching native providers |
| Canonical observation lifetime | At most 15 seconds from the original RPC start |
| Canonical refresh | 5 seconds after completion; bounded fair route/rail rotation |
| Capacity change sampling | 250 ms; changes bypass the ordinary heartbeat wait |
| Health and speed validity | Original observation expiry, under the configured monitor policy |
| Shutdown withdrawal | Best effort with a 2-second total transport budget |

These are scheduling/expiry bounds, not a network-delivery guarantee. Transport
deadlines, local queues and configured route counts affect propagation; actual
network/load qualification remains required. The earlier canonical expiry caps
Ready heartbeats, so managed availability normally expires sooner than the
60-second wire maximum. Neither a fresh heartbeat nor a fresh canonical read
renews old health or generation-speed evidence. There is no new generation timeout.

Heartbeat transport failure reconnects only the availability task, with a bounded
500 ms exponential backoff capped at 5 seconds. It preserves the publisher's
sequence and does not cancel paid inference or settlement. If an individual
publication cannot read or sign its state, it attempts withdrawal instead of
claiming availability. Failure of the supervisor task itself remains fatal.

The publisher uses the same shared capacity authority as admission. Advertised
free slots already subtract active/uncertain work and shared constraints. Buyers
must not add capacity across aliases of the same backend. The provider still
atomically takes a lease on acceptance; advertised capacity is not a reservation.

Busy, Unavailable, Checking and Draining remain distinct. LLMs require fresh
native-token speed at the default 5 tokens/s floor or a higher explicit customer
requirement. Decision endpoints do not fabricate token rates and cannot satisfy
an explicit LLM-rate constraint. `Table::status` supplies the common availability
result intended for routing and catalog/UI. Request capabilities, context, trust
and payment terms remain additional filters.

## Authentication, restart and resource limits

The signature binds network/contract, provider, market, offer slot/digest,
membership digest/revision, controller fence/boot nonce, sequence, expiry,
capacity and original speed evidence. Messages are bounded to 8 KiB and contain
no prompts, upstream URLs or credentials. Acceptance resolves current records
from one complete fresh catalog snapshot, including provider/fee revocation and
family/endpoint/metering policy status.

The existing locked capacity store supplies an increasing restart fence. The
receiver persists one anti-replay watermark per admitted offer. Old sequences or
controllers cannot undo withdrawal. Different boot nonces at the same fence
make that offer unavailable; their capacity is never summed. A higher fence or
newer canonical membership revision replaces the conflict. Receiver restart
restores no live Ready state, only the watermarks that new messages must satisfy.

The explicit storage quota rejects new telemetry instead of deleting replay
protection. It is not a catalog listing limit. Watermarks cannot age out while an
old controller might keep signing. Wall-clock and monotonic elapsed-time checks
both constrain cached availability. There is no per-token work, receipt-history
scan or growing event log. Receiver storage/signature processing uses one bounded
task at a time outside inference. Health is coalesced counters; disk updates are
per accepted heartbeat, not per model output chunk.

## Evidence and remaining integration

Tests cover authenticated bridge delivery, channel/signature/registration binding,
revocation, stale evidence, explicit/default speed floors, low-speed recovery,
restart replay, duplicate stores, storage quotas, clock rollback and decisions.
Managed tests show actual paid HTTP work changing availability to Busy, and
withdrawal on shutdown, with FIAT/TNK/TAP reservation/result/receipt/closure.
An injected heartbeat transport failure during paid work also completes that
request and settlement, then reconnects with increasing signed sequence numbers.
Publishing itself makes no ledger transactions. Healthy unregistered startup
publishes nothing.

These are local bridge and canonical RPC fixtures, not real Noise/relay or live
payment acceptance. `presence::gateway::Gateway` now provides a selected-market
supervisor component: one receiver at a time, coalesced subscription replacement,
500 ms exponential reconnect backoff capped at 5 seconds, and the same persistent
anti-replay Table across reconnects. Empty selection opens no connection. Removed
markets fail closed immediately; subscription changes drain the old receiver
before a new one starts. Callers signal its stop watch and await completion;
bridge setup uses the configured operation deadlines. Health is bounded counters, including generic
receiver failures because the receiver does not yet expose typed failure causes.

Both routing and catalog consumers can call `Gateway::status`, which resolves
current canonical registration on every lookup and applies `Table::status` with
the same optional customer speed floor. Stale/revoked catalog state cannot return
Available. This is a bounded synchronous control lookup, not per-token work.
The optional gateway lifecycle owns catalog hydration through the existing catalog
supervisor. It defaults to a 5-second refresh with at most 20% scheduling jitter,
and a bounded RPC deadline compatible with the 15-second presence freshness bound.
Slow pagination or failed refreshes still fail closed when evidence expires.

Canonical CLI startup accepts explicit
`mayhem use --proxy-config /absolute/private/gateway-proxy.json`. It compares the
protected configuration with the gateway's existing trusted peer network,
canonical admin and configured bootstrap pins. Embedded-catalog development mode
cannot enable this option. No configuration means no proxy stores or control tasks.
The owner-only version-1 document selects network identity, literal-loopback peer
RPC and bridge, a bridge token file, private state directory, market/storage quotas
and the explicit selected markets. Catalog and replay state use separate protected
files; a missing member of an existing pair is rejected instead of recreating lost
fences. Unix file protections are required by this implementation.

`GatewayState` holds an optional proxy control handle. Its explicit lifecycle runs
the two background controls and joins both, including already-started catalog disk
work, on shutdown. Proxy failure reports degraded state without terminating native
HTTP service. Ctrl-C/SIGTERM stops new HTTP admission and joins proxy cleanup;
existing native streams do not add an unbounded shutdown wait. Startup reports
configuration only, not Ready state or paid-route authorization.

Local tests exercise reconnect with withdrawal/replay preservation, explicit
market replacement, immediate removal, quota rejection, stale/revoked canonical
status, backoff, idle selection and cancellation. Gateway/CLI tests also cover
default-disabled state, protected configuration, canonical identity mismatch,
local background hydration, shared presence selection and joined shutdown.
Read-only offer endpoints are described in [DIRECTORY.md](DIRECTORY.md). They now
add default-policy observations through `observe_registered`, reusing their
single catalog snapshot without changing subscriptions. The shared table checks
registration expiry against both wall time and the effective monotonic time
established by received evidence; clock rollback cannot keep an exhausted
registration available. Request-specific constraints still apply at admission.
Gateway service-installer configuration, provider retirement UX and real-network
qualification remain.
Persistent provider-controller installation is available through the explicit
`mayhem provider proxy add` command; it is not automatic gateway activation.
Production activation is gated separately.
