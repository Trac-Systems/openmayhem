# Authenticated proxy exchange

This library transports an **already dual-signed** proxy purchase over the
existing Core SC-Bridge session. It does not negotiate prices, collect the
admission fee, publish a provider, or replace canonical financial admission.

The preceding authenticated Request/Proposal/Offer/Accepted exchange is implemented
by `negotiation::Channel`; see [NEGOTIATION.md](NEGOTIATION.md). Successful negotiation
can promote the same connection into this paid exchange. Promotion verifies the exact
dual-signed terms but grants neither funding nor permission to dispatch by itself.

Construct `exchange::Session` with the original authorization, the unlocked
local network/wallet identity and its buyer/provider role. `Channel::connect`
uses a protected literal-loopback bridge and subscribes to that authorization's
session. Each message binds the authenticated remote peer, session, accepted
terms, direction, sequence and complete-payload digest. Upstream credentials
and private endpoint addresses never belong in this exchange.

Logical messages are bounded by the supplied resource configuration (at most
256 MiB), independently of the 32 KiB fragment size. Exact fragment sizes and
offsets prevent reorder, replay and tiny-fragment floods. A cancelled or failed
transfer invalidates the channel: reconnect to the same logical request. A
message-read deadline covers all its fragments and is not a generation timeout.
There is no unbounded event history or queue.

`Channel::into_duplex` is an explicit opt-in handoff to independent sending and
receiving owners of that same authenticated socket. It preserves queued events,
session ownership, accepted terms and both sequence counters; it opens no second
connection. The bridge actor allows one outstanding send/close RPC while receiving
events, with configured message, event-count and queued-byte bounds. An overflowing
queue closes the transport instead of retaining unbounded history. Its finite RPC
deadline does not limit generation duration or idle time awaiting model output.

Keep a logical `Receiver::receive` future alive while handling other events, or give
the receiver a dedicated bounded owner. Cancelling it partway through a fragmented
message invalidates that receiver. Dropping either half closes the transport, without
asserting that upstream inference stopped or any financial obligation was settled.
The established synchronous client remains unchanged unless explicitly handed over.

## Execution and receipts

1. The buyer sends `Execute` with its normalized request. The provider passes
   the authenticated `Received` to `Session::execute_json` or `execute_stream`.
   The existing paid executor still verifies fresh canonical funding, immutable
   request/offer bindings, shared capacity and durable dispatch intent before
   the upstream POST. The invocation is scoped to the network, buyer and billing
   attempt; changing prompt bytes or terms cannot evade that journal.
2. The provider sends provisional normalized `Stream` events where supported,
   followed by the durable normalized `Result`. A result alone is not a receipt
   or proof of payment. Tools must not execute from provisional argument fragments.
3. The buyer uses `Session::decode_result` with its **own** request and original
   acceptance snapshot. This reconstructs observed usage without receiving the
   provider's upstream job ID or trusting its reported token counters.
4. The provider signs the immutable terminal draft and sends `Receipt` (or an
   explicitly chosen `Waiver`). `BuyerRecovery::approve_receipt` independently
   verifies the original contract in the isolated verifier, recounts usage and
   checks the amount and signature. Only a durable approval can be signed.
5. The buyer sends its saved `Acknowledge`. `Session::acknowledge` retains that
   exact envelope and verifies canonical publication. Transport delivery alone
   is not confirmation. Financial closure and execution-capacity reconciliation
   remain independent requirements.

New receipt/waiver result commitments use the domain
`mayhem/proxy/public-result/v1`, covering the normalized response, original
binding and attempt. Usage and money have their separate signed commitments.
The private journal continues to hash its full retained response under the
unchanged owned-result domain. Old drafts missing `result_commitment` keep the
old private commitment policy and their original signatures; they are not
silently rewritten. Such legacy approvals still require their original evidence.

## Reconnect and cancellation

`Status` reads one owned attempt. Completed retained output is available for
recovery even before the receipt has been prepared. Repeating the same JSON
request may return that exact result without another inference POST. A repeated
stream returns `ExistingResult`; use status/result recovery rather than appending
a new completion onto previously delivered deltas.

`Cancel` durably records cancellation before signalling local execution. A
provably unsent request may release its capacity allocation. A sent request with
unknown upstream outcome stays occupied and cannot be retried or waived merely
because the socket closed or a timer elapsed. Closing a channel does not cancel
or settle its logical request.

A [verified broker refusal](REFUSALS.md) is retained as a terminal nonexecution outcome.
The provider may send `Failure` followed by a signed `Waiver`; status recovery returns
`AwaitingReceipt` and that same waiver without re-running inference. This is an offer for
explicit buyer zero-charge consent, not a refund or another dispatch. Buyer verification
binds the sanitized evidence to the original request/terms and rejects contradictory
received output or canonical receipts. Acknowledgment and canonical confirmation use the
existing closure path on the originally accepted rail. Unknown failures never qualify.

## Verification and remaining integration

Local tests exercise the real bridge client against a bounded authenticated
SC-Bridge protocol double, an actual HTTP backend fixture, isolated decoder and
verifier processes, and the existing signed canonical financial RPC fixture.
They cover all four JSON endpoints/three rails, the three streaming endpoints,
independent acknowledgments, legacy receipt/waiver recovery, cancellation before
and after dispatch, replay and malformed/foreign fragments. This is not a live
Noise/relay network proof or an external payout test.

The opt-in supervised controller and bounded periodic reconciliation runner are now
implemented in [SERVING.md](SERVING.md). They own independent read/write tasks and
retain in-flight work across connection loss. No blocking receive holds a channel
lock required for stream delivery. The authenticated opening and opt-in registered
dispatcher are implemented there as well. Automatic startup,
upstream job polling/cancellation, adaptive health, public API/Studio/MCP and
end-to-end real-network acceptance remain required integration work. Local controller
and transport tests do not prove automatic public serving.

[QUOTES.md](QUOTES.md) describes the fresh canonical inputs and request-derived
purchase builder that precede this already-signed exchange. The components here do not
replace durable signing or financial admission.
