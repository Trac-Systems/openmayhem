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
6. `refresh` records canonical progress, including exact never-admitted expiry
   and recovery of a lost countersignature from canonical acceptance. `confirmed()` is historical evidence,
   not a fresh model-dispatch permit. Paid execution must still obtain current
   funding and capacity evidence independently.

The full owned request remains available for independent result verification
after restart. A canonically closed financial outcome or proven never-admitted
expiry enables retention pruning. No local timer, socket error or process exit
removes an unresolved signed intention or proves an upstream model stopped.

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
below. Authenticated pre-acceptance transport is implemented in `negotiation::Channel`;
the trusted session dispatcher and automatic supervisor startup are still required.
Bounded provider proposal orchestration and buyer non-admission recovery are
described below. Provider reconciliation of partial signing and capacity retirement
is implemented; the opt-in session controller and rotating recovery runner are
described in [SERVING.md](SERVING.md). Missing local state alone cannot release a
signed or uncertain intention.
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
fixture authenticates its loopback participants; real relay/Noise acceptance remains
a separate unfinished requirement.

## Authenticated pre-acceptance transport

`negotiation::Channel` transports Request, Proposal, Offer and Accepted in that order.
Its immutable Context binds the network, both peers, session, logical buyer attempt,
exact request fingerprint, offer, rail and settlement policy. Constructing or parsing
Context does not authenticate a caller: the trusted session dispatcher must supply it
after admission/identity checks. That dispatcher is not implemented by this transport.

The buyer sends its normalized request. The provider proposes a public adapter plus
opaque connection, reservation and capacity identifiers. A Proposal is a set of claims,
not a capacity capability. The buyer independently obtains its canonical quote, applies
its own resource/spend policy and durably signs the request-derived terms. The provider
must use its actual Runtime and ProviderNegotiation to check and save its countersignature.
The wire checks real signatures, the original buyer signature, message order and exact
context/proposal bindings. It exposes no raw signing service or private upstream mapping.

After Accepted, both parties can promote the same authenticated connection to the paid
exchange. Negotiation and execution have separate frame purposes and commitment domains;
stale negotiation fragments cannot become execution commands. Promotion still does not
reserve money or authorize POST. Canonical financial admission and durable shared capacity
checks remain mandatory. A sanitized Refused response terminates negotiation without
creating a paid channel.

Recover starts a separate recovery exchange for the same logical context. The provider
returns its original durable countersignature; the buyer compares/retains it against its
original purchase. Lost delivery or acknowledgment must not create another purchase,
signature, reservation or upstream execution. Older-contract recovery remains possible;
new Request negotiation requires the current compiled contract. Closing or losing the
channel does not release a signed obligation or execution capacity.

Transport reuses the bounded SC-Bridge framing described in [EXCHANGE.md](EXCHANGE.md).
Negotiation sends have a caller-configured total control deadline across all fragments;
receives require an explicit finite control wait. These are not generation deadlines.
Timeout, cancellation, malformed framing or invalid received context poisons the channel.
Reconnect uses durable state, never a guessed successful or cancelled outcome.

Local checks cover the full negotiation-to-paid path on every supported JSON endpoint
and payment rail, streaming request acceptance, lost acknowledgment after buyer receipt,
fragmentation, replay, tampered identities/context/request/signatures, control timeout
and failed promotion before acceptance. They use the loopback SC-Bridge double and local
canonical accounting fixture. Resource-budgeted session dispatch, full-duplex supervision
and real relay/Noise acceptance remain required.

## Provider proposal controller

`negotiation::provider::Controller` accepts an authenticated Request, checks a fresh
canonical offer against the actual operator-approved adapter/connection/policy, validates
the request, then reserves one shared capacity lease. Provider/network, recipe, endpoint
contract, connection revision, capacity group and declared concurrency must agree. It
creates the public Proposal from those actual objects; caller-supplied slot or connection
claims cannot substitute for them. No money or inference is executed by this controller.

Identical concurrent or reconnected unsigned negotiations return the same proposal and
lease. A changed session/context for the same attempt is rejected. Already retained
execution/signing intentions require recovery instead of a new proposal. Pending count,
per-buyer count, individual/aggregate request bytes and blocking storage operations have
explicit operator limits. Request validation precedes capacity allocation. No history or
receipt scan is needed; existing intention lookup is indexed by logical invocation.

An exact received buyer Offer is checked again against fresh canonical terms and the
reserved runtime before countersigning starts. From that point, errors or cancellation
retain the allocation for recovery. Successful signatures remain durable before return;
only pending protocol memory is freed. Execution capacity remains occupied until the
existing paid executor reconciles a known outcome. Recover returns the original saved
signature, without repricing, re-signing or creating another financial reservation.

Before provider countersigning starts, explicit cancellation or the configured unsigned
proposal lifetime can release its never-dispatched capability. Lifetime never caps
inference or replaces financial expiry. Cleanup is serialized with acceptance and keeps
the pending entry if capacity cleanup fails. Losing a caller's reply does not discard
controller-owned state. The supervisor must invoke the bounded unsigned-expiry method
even without new incoming inference; this scheduling is not implemented by the library.

Process restart initially preserves all durable capacity allocations, treating
prior-controller leases as uncertain. In-memory disappearance is not proof of cancellation.
The bounded reconciliation below may reclaim explicitly never-signed proposals. Partially
prepared/signed intentions still require separate recovery before automatic public startup.
This controller is local integration work, not production readiness. A single shared
runtime authority is required for native/proxy aliases; outside consumers remain unknown.

Local controller tests negotiate all four JSON endpoints on every rail, reject execution
before funding, then complete one real loopback HTTP request. They cover all three LLM
streaming request forms/rails, concurrent proposal replay, per-buyer/byte bounds, malformed
input, operator-policy mismatch, unsigned expiry, signing failure, shared native capacity,
failed cleanup and restart without a fabricated free slot. Existing receipt/payout and
streaming executor regression checks remain part of acceptance.

## Restart reconciliation of never-signed proposals

New controller proposals use a durable `Proposed` capacity phase. This phase cannot
dispatch inference. Before touching the signing journal or wallet, provider acceptance
atomically commits `Proposed -> Reserved` in the shared capacity store. Both the journal
and protected wallet signer require that fence. Failed durability cannot return permission
to sign. Existing `Reserved` records retain their old meaning and are never relabelled as
never-signed proposals; native allocations are unchanged.

`Controller::reconcile_unsigned(after, limit)` scans one indexed shared-group page of
at most 64 entries. It releases only this proxy route's prior-controller records whose
stored phase is still `Proposed`. This proves provider countersigning and dispatch never
started, independently of a missing journal row, elapsed time or ledger availability.
It does not release a financial hold, publish a receipt, cancel native work, or report
fresh serving capacity. Fresh upstream readiness remains independently required.

Current-controller proposals, other routes, native allocations, historical `Reserved`
records and any signing/dispatch uncertainty remain retained. The per-record transaction
rechecks the exact lease and old controller fence before removing its indexes/counters.
Recovery is replay-safe and resumes by seek cursor even when earlier rows were deleted;
the supervisor must wrap the cursor after each pass. No whole-history rebuild is used.

The tests reopen real stores and demonstrate reclaim followed by a new proposal, bounded
pagination with native/legacy allocations, refusal to reclaim live proposals, and retention
of both committed signatures and a signing failure after the durable capacity fence.
Partially prepared/signed provider records still need integration with canonical
expiry/absence reconciliation. Do not treat this capacity cleanup as proof that
those separate records can be erased.

## Canonical non-admission recovery

`/v1/proxy/intent-state` accepts the original buyer-signed terms. Either party may
query through its own trusted Core peer. Both the signed service envelope and the
buyer signature are verified; every read generates a new indexer challenge. The
reply binds the exact terms, requester, network, current canonical view and nonce.
It uses the same bounded 32 KiB request, 128 KiB response and 15-second control
freshness as negotiation observations. HTTP errors are not absence evidence.

The canonical service reads the exact permanent accepted-terms key and associated
financial identifiers. An existing acceptance is returned as admitted, regardless
of current prices, withdrawal, contract upgrades or elapsed time. Inconsistent
financial footprints reject the read. No current offer, payout readiness or buyer
balance is required, and unrelated billing data and payout targets are not exposed.

An absent intention becomes expired only after the COMPLETED canonical epoch
reaches its billing epoch. This follows the existing reservation rule: new admission
requires `billing_epoch == (pending_epoch ?? updated_epoch) + 1`. A pending epoch
alone is insufficient for local retirement. No ledger rule/version changes are
introduced by this observation. Already admitted work uses the existing financial
receipt/waiver/expiry path; this proof never refunds or cancels admitted execution.

Buyer negotiation stores retain exact private non-admission evidence, leave
`confirmed()` false, close their pending index and prune after their configured
retention. Reopen preserves that distinction; late signatures cannot reopen the
closed intention. An admitted reply can instead restore a lost countersignature
before recovering the real financial state. Ordinary confirmed-purchase refresh
continues to use one financial observation, not an extra intent read per turn.

The reservation-publication recovery queue also handles verified non-admission.
After an unsuccessful publication/financial observation, it may obtain this fresh
proof, retire the original publication and expose `expired_unadmitted`. The runner
counts it resolved and stops retrying that entry, including after restart. Local
expiry does not fabricate a financial receipt, release a buyer hold, or free a
provider capacity lease. Publication uncertainty with no such proof stays pending.

Local coverage includes all four endpoint families/three rails for buyer-only and
dual-signed intentions, late-signature refusal, lost ACK after actual admission,
reopen/prune/reuse, pending publication retirement, role authentication, fresh nonce
and replay checks, superseded offers/old contracts, corrupt footprints, bounded
reads and unchanged balances/native holds. Provider-side partial-signing recovery
and opt-in session supervision are implemented in [SERVING.md](SERVING.md);
the public dispatcher and automatic startup remain unfinished.
