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
sequence and does not cancel paid inference or settlement. Structural local
storage/signing failures remain fatal rather than being disguised as networking.

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
payment acceptance. The receiver/eligibility components still need automatic
gateway/catalog supervisor wiring and buyer-surface integration. Automatic
mayhemd installation, provider configuration/retirement UX and real-network
qualification remain. Production activation is gated separately.
