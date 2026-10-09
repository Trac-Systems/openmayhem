# Durable purchase negotiation

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

Provider countersigning now uses `financial::provider::ProviderNegotiation`, as described
below. Authenticated pre-acceptance wire exchange, provider proposal orchestration and
automatic supervisor startup are still required. Never-admitted
signed intentions also need canonical expiry/requote reconciliation before that
automatic controller ships. For now they stay retained and count against the
bounded quota; silently deleting them could enable conflicting authorizations.
Public API/Studio/MCP serving and production deployment remain separate gates.

## Provider acceptance

The provider obtains `/v1/proxy/offer-state` through its own trusted Core peer. The
signed indexer service binds the query to the provider wallet and a new internal
challenge on every read. It returns the exact current offer, membership, billing
epoch, enabled settlement policy and current ready payout binding. It never reads
buyer balances/billing, exposes payout targets, reserves funds or appends a ledger
entry. Shared offer checks are identical to the buyer quote checks. Queries and
replies are bounded to 32 KiB and 128 KiB; freshness is 15 seconds from the request,
not extended by caching or replay. This is control-read freshness, not an inference
timeout. The Rust client trusts that authenticated canonical service; it does not
independently verify a Merkle proof.

Construct `provider::Runtime` from the actual operator-approved adapter, connection,
shared capacity authority/route and settlement policy. `Runtime::approve` verifies
the buyer's signature, owned JSON/stream request, metering allowance, canonical offer
and payout terms, connection fingerprint/revision and exact current reserved lease.
The lease must belong to this provider/network, logical buyer attempt, request hash,
proxy route and canonical shared group. Globally enabled policy is insufficient:
it must match this operator's approved policy. This object is not deserializable.

`ProviderNegotiation::accept` retains the owned request and runtime snapshot and
reserves result/outcome storage in the existing execution journal. Only after fresh
capacity/offer rechecks does it sign and durably save the exact countersignature.
Signature retrieval waits for the committing writer; failed durability acknowledgment
blocks signature retrieval until reopen. Buyer negotiation uses the same protection.
The executor's dispatch checks still apply after buyer publication confirms the hold;
a saved countersignature alone never proves funding or permits a model POST.

The additive local journal schema 7 retains original records from schemas 1–6,
including unknown execution. It does not fabricate historical provider signatures.
The signature table has bounded rows, shares journal byte quotas and is pruned only
with an already-closed attempt. `recover` returns the original signature after lost
responses or restart; it does not grant a new lease or reprice a purchase. Async
storage work holds a bounded executor permit until the actual disk operation ends,
even if its caller disconnects. No raw signing endpoint or worker key access exists.

Local tests cover all four JSON endpoint types and all three payment rails through
buyer signing, provider countersigning, canonical reservation, actual loopback HTTP
execution, receipt publication and pruning. Chat/Completions/Responses streaming
acceptance is checked on all rails; the existing paid streaming executor regressions
remain required. Tests also cover competing signed terms, unchanged-body replay,
wrong wallets, invalid signatures, request/metering/connection/lease mismatches,
withdrawn or dispatched capacity, storage exhaustion, reopen, abrupt process exit,
legacy journal migration, and signature-read fencing after failed fsync. These are
local fixture/accounting proofs, not live model or external payout acceptance.

## Public buyer evidence and execution handoff

New buyer purchases retain `buyer::Snapshot::Public`. They use `PublicAdapter`
with the public endpoint contract, the recipe digest pinned by canonical membership,
and their own bounded resource policy. Provider-specific translation remains in the
provider's private Adapter. The public verifier shares request/result validation and
metering rules with that adapter but cannot produce a dispatchable Request or upstream
body. It does not reconstruct or guess the private upstream model mapping. Receipt
approval and authenticated result delivery accept this buyer-owned public evidence.

Legacy private buyer snapshots deserialize and reserialize with the exact original
shape, preserving the purchase commitment and signatures. They are converted only
in memory for verification; recovery does not rewrite a previously signed intent or
silently renew/reprice it. Unknown or mixed public/private snapshot fields are rejected.
Provider journal snapshots and recipe hashes are unchanged.

An authenticated session may encounter the Prepared attempt retained during provider
countersigning, before canonical funding was attached locally. It may hand this into
paid admission only when the exact durable provider acceptance matches the session.
Unsigned drafts and different accepted terms cannot pass that check. The session
still checks canonical funding and current capacity before the first model POST;
signatures alone do not suffice. Local four-endpoint/three-rail checks now exercise
that session handoff and independent public buyer receipt verification. The bridge
fixture authenticates its loopback participants; real relay/Noise acceptance and the
pre-acceptance negotiation transport remain separate unfinished requirements.
