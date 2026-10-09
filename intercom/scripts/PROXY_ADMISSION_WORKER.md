# Proxy admission worker (local implementation; disabled by default)

This is an off-ledger fee verifier and a separate permit issuer. It never calls
native deposit, bridge, credit, payout or balance mutation code. The existing
canonical admission permit and publication gate remain the only registration
authority. Synthetic acceptance does not activate production collection.

SITE owns signed-wallet challenges, durable invoices, exclusive receiver
allocation, payment-reference discovery, cross-purpose evidence claims and
phase-specific outbox leases. A verifier certifies one exact observed payment;
SITE preserves it even when the invoice is short. An issuer receives the original
immutable permit body plus a sorted evidence set, independently checks the exact
invoice/reference/receipt bindings and total, then signs only the existing
`proxyAdmissionSigningBytes`. SITE must persist the exact completion before ACK.
No worker chooses a new permit nonce, allocation, entitlement or issuance revision.

## Wire and custody

`tests/fixtures/proxy-admission-worker-v1.json` is the executable shared contract:
TAP, TNK, FIAT and two-transfer TAP top-up with excess. Outer names are snake_case;
missing or extra fields are rejected. Required nullable fields are explicit.
FIAT `amount_base_units` / permit `accepted_amount` are minor currency units.
TAP/TNK amounts are token base units. `accepted_value_au` is the existing protocol
fee allocation, `10000000000000000000` AU. No rates, quote lifetime, receiver,
finality count or permit epoch duration are invented by this worker.

All actions use the fixed configured SITE origin and POST
`/internal/proxy-admission-worker/{pull,renew,retry,review,evidence,permit}`.
Separate verifier/issuer bearer credentials are mandatory at the SITE boundary;
credential scopes must enforce phase and configured rail access before parsing
payment work. The verifier process has no issuer key; the issuer has no receipt
retrieval credential, buyer wallet or bridge configuration.

- Pull: `{schema_version:1,purpose:"proxy_admission_fee",phase,rails}`; `rails` is
  distinct, sorted and nonempty. Response adds `work` (the fixture or `null`) and
  omits `rails`.
- Base completion identity: `{schema_version,purpose,phase,invoice_id,
  invoice_revision,lease_token}`.
- Renew: base; response `{schema_version,purpose,phase,lease_expires_at_ms}`.
- Retry: base plus `code,delay_seconds`; review: base plus `reason`.
- Evidence/permit: fixture bodies. Success ACK is exactly
  `{schema_version:1,purpose:"proxy_admission_fee",phase,accepted:true}`.
  Retry/review can return this same ACK.

An ambiguous completion causes only an exact retry. Different roles cannot send
the other role's completion. A worker retains at most one active job; SITE owns
global lease concurrency. Each run has a 15-second deadline, renews short leases,
and refuses completion after expiry. Pending underlying reader cleanup retains
that worker's active slot. Requests and work are bounded to 16 KiB; upstream
TAP/Stripe receipt reads to 256 KiB; policy reads to 8 KiB. Redirects are refused.
Trusted origins require HTTPS; literal loopback HTTP needs explicit operator opt-in.
No URL, credential, alternate origin or execution mapping is accepted from work.

Invoice commitment uses the existing canonical protocol encoder and BLAKE3:
`mayhem/proxy/admission-invoice/v1\0` + stable JSON of
`{invoice_id,invoice:<without invoice_commitment>}`. Receipt commitment uses domain
`mayhem/proxy/admission-evidence/v1` over
`{invoice_commitment,payment_reference,receipt}`. The issuer's aggregate commitment
uses `mayhem/proxy/admission-evidence-set/v1` over
`{invoice_commitment,receipts:[{payment_reference,receipt}]}` sorted by physical key.
These are the existing protocol's UTF-8 domain/NUL/canonical-body digest convention,
not a newly substituted derive-key convention. Observed wall time and changing
confirmation counts are excluded from immutable receipt identity.

## Canonical policy and receipts

`POST /v1/proxy/admission-policy` accepts only `{request_nonce}`. The trusted peer
adds its own fresh service nonce, signs the existing authenticated service request,
and verifies response nonce, requester, network, proof and age. The canonical
handler reads exactly `proxy/v1/config` in a signed, current snapshot and returns
public enablement, fee-policy hash, active issuers, maximum permit epochs and
canonical epoch/proof. It uses four bounded read permits; timeout does not release
a permit before underlying cleanup. No invoice, payment, provider or ledger scan
is performed. A worker independently re-reads this policy for verification/issuance.

TAP checks chain, token, exact transaction/log index, destination, positive actual
amount, successful receipt, finalized block and block hash/time. It does not fall
back to guessed confirmation finality. Stripe retrieves the exact succeeded event,
PaymentIntent and captured charge under the configured account/livemode, checks
invoice/purpose/operation metadata, and refuses refund/dispute evidence. A
successful Stripe payment is an observed collection fact, not a guarantee against
future reversal. TNK reuses the existing signed transfer scanner with an explicitly
configured wallet-free network/bootstrap, bounded lookback and finality. Its
frontier is pinned per scan; head advancement cannot expand query work. No synthetic
MSB position hash is presented as an actual block hash.

`reference_assigned_at_ms` must be the trusted discovery time of an actual payment,
never the time a user submitted a precomputed transaction hash. This is essential
for TNK, which supplies no authoritative transfer wall-clock timestamp. Late or
future references, late observed TAP/Stripe payment times, changed policy, expired
permits and reissue requests remain review cases. The worker does not reprice,
refund, reverse credit or charge a second admission fee.

## Operator configuration (no service enabled by these examples)

Launch only after explicit configuration using
`PROXY_ADMISSION_WORKER_ENABLED=1 PROXY_ADMISSION_WORKER_CONFIG=./operator-config.json node intercom/scripts/proxy-admission-worker.mjs`.
`PROXY_ADMISSION_WORKER_ONCE=1` performs one lease attempt. Without the enabled flag
the program exits before reading configuration or opening any connection.

Common config: `phase`, `api_origin`, `api_credential_file`, `core_origin`, exact
`network` identity, `fee_policy_hash`, `issuer_pubkey`, `allow_loopback_http`.
Issuer adds `issuer_key_file` (Ed25519 PKCS8 PEM) and explicit sorted `rails`.
Verifier adds one `verification` object:

- TAP: `rail:"tap",rpc_origin,chain_id,token_contract`.
- FIAT: `rail:"fiat",stripe_account,livemode,currency,credential_file`.
- TNK: `rail:"tnk",network,msb_bootstrap,channel,dht_bootstrap,state_dir,lookback,
  finality,reader_timeout_seconds`. Bootstrap/network must match the common
  canonical identity; lookback <=100000 and reader timeout <=10 seconds are work
  bounds, not replacement finality/fee policy. The reader uses no wallet.

## Acceptance and explicit remaining work

Run `node --test intercom/tests/proxy-admission-policy.test.js intercom/tests/proxy-admission-worker.test.mjs`.
Tests include actual authenticated policy relay, synthetic exact-log RPC and
separate worker credentials, immutable signature recovery after lost ACK,
underpayment/top-up/excess, resource bounds, and actual canonical publication
with no duplicate append or native fund changes. The local HTTP SITE queue in
this test is a fixture; real SITE persistence/ownership acceptance is separate.

This worker alone does not complete P3 or public onboarding. SITE collection
initiation, signed-wallet challenge/status recovery, real receiver allocation and
payment discovery must be connected and tested. Inline issuer work supports up to
32 receipts within the unchanged 16 KiB bound; larger lifetime payment sets must
remain durable and pending an indexed, snapshot-pinned paged issuance transport,
not be rejected or forgotten. Historical TNK evidence outside the configured
lookback needs explicit bounded recovery. Expired-permit reissue awaits reconciliation
of the exact original canonical append before any generation changes. Refunds,
reversals, late valuation, tax/fees and other D3 decisions remain explicit operator
policy work; no automated default is introduced here.
