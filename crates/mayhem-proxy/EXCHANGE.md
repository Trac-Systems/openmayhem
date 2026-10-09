# Authenticated proxy exchange

This library transports an **already dual-signed** proxy purchase over the
existing Core SC-Bridge session. It does not negotiate prices, collect the
admission fee, publish a provider, or replace canonical financial admission.

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

## Verification and remaining integration

Local tests exercise the real bridge client against a bounded authenticated
SC-Bridge protocol double, an actual HTTP backend fixture, isolated decoder and
verifier processes, and the existing signed canonical financial RPC fixture.
They cover all four JSON endpoints/three rails, the three streaming endpoints,
independent acknowledgments, legacy receipt/waiver recovery, cancellation before
and after dispatch, replay and malformed/foreign fragments. This is not a live
Noise/relay network proof or an external payout test.

Offer negotiation, the supervised full-duplex controller, automatic startup,
upstream job polling/cancellation, adaptive health, public API/Studio/MCP and
end-to-end real-network acceptance remain separate required integration work.
Do not run a blocking receive while holding a shared channel lock needed by
stream delivery; the supervised controller must own and schedule that I/O.
