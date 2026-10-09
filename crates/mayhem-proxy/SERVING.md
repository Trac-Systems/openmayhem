# Supervised provider sessions

`serving::Controller` assembles proposal negotiation, paid execution, retained
results and receipt delivery from the same owned runtime, journal, wallet,
financial client and isolated worker pool. It accepts a provider-authenticated
`negotiation::Channel`; it is not itself a public listener or a signing endpoint.

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

## Evidence and remaining integration

Local acceptance covers four JSON endpoints and all three payment rails, all three
LLM streaming endpoints, missing funding, reconnect without duplicate inference,
cancellation, generation outliving the control wait, malformed requests, quotas,
pending receipt publication without a connected buyer and canonical non-admission
retirement. Queue checks cover byte/count pressure, cancellation, disconnect and
actual writer acknowledgment. Fixtures use the real local signed financial RPC,
HTTP backend and isolated workers, with a bounded SC-Bridge protocol double.

Trusted public-session dispatch, automatic CLI/mayhemd startup, adaptive capacity,
public availability and real-network/payment acceptance are still required. These
library tests are not a deployed serving claim. The Core/site release and fleet
activation gates remain separate.
