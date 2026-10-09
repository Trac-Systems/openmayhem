# Proxy quote inputs and purchase construction

`financial::Client::quote` reads fresh canonical inputs through the existing
authenticated indexer service and configured trusted Core RPC. It does not
reserve money, acquire model capacity, sign a purchase or dispatch inference.
The Rust client relies on that authenticated service for canonical proof
verification; it is not an independent ledger verifier.

The service binds the request to the local buyer wallet and selected offer,
payment rail, settlement policy and logical billing ID. It checks the current
market, paid provider admission, offer revision, financial quota and verified
payout binding. It reads the buyer's existing aggregate holds, including native
holds, and the specific billing record if one exists. It does not enumerate
receipt or price history or return the payout target. All reads use one canonical
snapshot, checked before and after the operation.

Each invocation creates a new internal signed challenge, even when the caller
repeats its correlation nonce. An observation expires after 15 seconds measured
from the request; caching or rereading it cannot extend that lifetime. Request
and response bodies and concurrent control requests are bounded.

## Constructing a purchase

1. Resolve `PriceLimits` from the buyer's actual request/project/account policy:
   every unit rate, fixed request charge, session minimum and total logical
   purchase limit. There is no inferred consent from the account balance.
2. Construct `PurchaseRequest` from the owned outgoing request and selected
   adapter, plus explicit epoch lifetimes and an output billing-unit allowance
   for text endpoints. Validation preserves streaming, tools and structured
   output controls; unsupported requests fail before reservation.
3. `Observation::prepare_purchase` computes input units from the actual request.
   Decision requests reserve exactly their validated question count. Text
   requests reserve those input units plus the explicitly authorized output
   units. Native `max_tokens` is not treated as observable billing units.
4. Terms take the current offer, recipe, membership, payment binding and policy
   from the quote. The maximum hold is the exact fixed-offer cost of that usage
   envelope, checked against price limits, remaining logical budget and funds.
   A known active or uncertain previous attempt requires recovery first.
5. Keep the resulting request, execution snapshot, policy and exact terms for
   durable negotiation. `recheck_purchase` checks that unchanged proposal against
   a newly fetched observation before signing. It never silently reprices it.

`SessionBinding` contains controller-proposed session, reservation, connection
and capacity identifiers. These are commitments, not evidence of a real free
slot. The existing paid execution guard still independently verifies canonical
funding and a durable capacity lease before sending an upstream request.

## Integration status

Local tests construct purchases for Chat, Completions, Responses and Decisions
on FIAT/TNK/TAP, including the three supported streaming forms, and admit their
exact holds through the existing signed reservation/recovery path. Those tests
use ephemeral fixture signatures, not a completed production negotiation signer.
They also cover stale offers, changed canonical views, request/signature/replay
tampering, unready payout bindings, incompatible recipes, price/budget overruns
and preservation of native holds. No real model or external payment is involved.

The buyer-side durable signing and reservation handoff is documented in
[NEGOTIATION.md](NEGOTIATION.md). Provider-side countersigning, authenticated offer
exchange, supervisor startup and public API/Studio/MCP integration are still
required. Do not expose an arbitrary-bytes signing RPC or sign merely because a
quote was returned.
