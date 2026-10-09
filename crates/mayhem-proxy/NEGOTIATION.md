# Durable buyer purchase signing

`financial::negotiation::BuyerNegotiation` connects the owned-request quote
builder to existing reservation/recovery accounting. It is a trusted-parent
component, not an HTTP signing service or permission for a connector to sign.

## Ordering and recovery

1. Before creating another quote, `lookup` the original logical billing ID and
   attempt. A lost reply must recover that purchase rather than create a new ID.
2. Build `PreparedPurchase` from the actual request and buyer policy as described
   in [QUOTES.md](QUOTES.md). Fetch a fresh buyer-bound quote.
3. `sign` checks that quote and the unlocked wallet's network/role, retains the
   original body, adapter, policy, prices and terms, and durably commits the buyer
   signature before returning it. Concurrent identical calls recover the same
   record. Different terms for that logical attempt are rejected, even if the
   provider/session/rail changes. No raw signing RPC is exposed.
4. `retain_provider_acceptance` verifies both signatures and requires the exact
   original terms and buyer signature. It commits the provider signature before
   publication can begin. An alternate but correctly signed purchase cannot
   replace the owned intention.
5. `publish` first hands the immutable authorization and original timestamp to
   `BuyerRecovery`, then uses its existing canonical publication path. A pending
   or lost acknowledgment leaves the same purchase recoverable. An already
   confirmed purchase, or one from an older contract version, is recovered through
   fresh canonical evidence rather than submitted as a new obsolete reservation.
6. `refresh` records canonical progress. `confirmed()` is historical evidence,
   not a fresh model-dispatch permit. Paid execution must still obtain current
   funding and capacity evidence independently.

The full owned request remains available for independent result verification
after restart. Only a canonically closed financial outcome enables retention
pruning. No timer, socket error, quote expiry or process exit removes an unresolved
signed intention or proves an upstream model stopped.

## Storage and load bounds

The private redb database has one owner, a fixed cache and configurable record and
total byte quotas. Each new intention reserves extra space for its countersignature,
canonical proof and closure before exposing a signature. Existing records can
finish using their allocated space after admission limits are lowered. Pruning
releases that allocation only after confirmed closure and its saved retention
deadline. Pending and closed scans are indexed and page-bounded; no receipt history
or per-token reads are involved.

Storage calls run through a bounded blocking executor. Cancellation of an await
does not cancel an in-progress commit or release its executor slot prematurely.
A commit/fsync failure fences subsequent writes until recovery/reopen; it cannot
return signed bytes as successfully persisted. Resource configuration still has
to budget request sizes and aggregate worker memory at supervisor integration.

## Verified scope and remaining work

Local tests cover all four JSON endpoint types and FIAT/TNK/TAP, identical and
conflicting concurrent requests, wrong signers, provider substitution, quotas,
lowered limits, lost/pending publication, reopen, abrupt process exit and injected
fsync/acknowledgment failure. They use actual local RPC/accounting with ephemeral
test identities. No external payment or real inference is claimed.

Provider-side offer preparation/countersigning, authenticated pre-acceptance wire
exchange and automatic supervisor startup are still required. Never-admitted
signed intentions also need canonical expiry/requote reconciliation before that
automatic controller ships. For now they stay retained and count against the
bounded quota; silently deleting them could enable conflicting authorizations.
Public API/Studio/MCP serving and production deployment remain separate gates.
