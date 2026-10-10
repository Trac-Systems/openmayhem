# Platform admission-return worker

Local implementation only. Do not enable collections or return workers without
the approved commercial policy, reviewed credentials and deployment approval.
This does not make the one-time admission fee generally refundable.

The FIAT adapter preserves the exact Stripe refund request and stable idempotency
key in an owner-only journal **before** requesting dispatch authorization from
SITE. The signed review fixes the invoice, verified original payment, amount,
rail, method/destination and policy. The distinct executor key signs preparation
and delivery; neither provider nor admission-permit keys can exercise that role.
No retail credits, inference payouts or admission-contract mutations are performed.
An enabled crypto return necessarily sends its approved original-rail transfer.

`proxy-admission-refund-worker.mjs` is an explicit, disabled-by-default entry point.
Set `PROXY_ADMISSION_REFUND_WORKER_ENABLED=1` and
`PROXY_ADMISSION_REFUND_WORKER_CONFIG` to an operator-owned protected JSON file
only after approval. Its exact fields are:

- `api_origin`, `api_credential_file`: fixed SITE origin and the dedicated executor
  credential, not an admission verifier/issuer or retail token.
- `policy_file`: exact approved `RefundPolicy`, including public review/execution
  key sets. It must permit every configured adapter; only explicitly configured
  FIAT/TNK adapters are pulled. Permitting TAP in policy does not enable a sender.
- `executor_key_file`, `executor_password_file`: encrypted Ed25519 PKCS8 and its
  exact passphrase bytes. No private key is supplied over the worker API.
- `journal_root`: existing canonical owner-only directory, mode0700. Records are
  fixed-name, bounded16KiB, mode0600, durable and immutable; no directory scan.
- `stripe`: exact `account`, `livemode`, `currency`, `credential_file`,
  `retry_window_ms` and `max_lookup_pages`. The retry window must be1000ms through
  23hours; choose it deliberately. The bounded lookup is scoped to one original
  PaymentIntent,100records/page, at most1–16configured pages. No global history scan.
  Set `stripe` to null for a TNK-only worker; existing FIAT configurations remain valid.
- Optional `tnk`: exact fields `network`, `network_name`, `core_origin`, `state_dir`,
  `channel`, `dht_bootstrap`, `direct_peers`, `format`, `key_file`, `password_file`,
  `receiver`, `finality`, `reader_timeout_seconds`. `network` is the complete
  network ID/bootstrap/subnet/contract context. `network_name` is mainnet or
  testnet1 and must match the installed MSB configuration. Transport lists are
  bounded to16; the reader timeout is1–10seconds. `state_dir` is an existing
  canonical owner-only directory dedicated to this custody worker. Never reuse
  a running reader's store. `receiver` must match the existing custody key.
  `format` is encrypted_pkcs8 (Ed25519) or the existing encrypted trac_wallet
  format; the loader checks the public/secret pair. It never creates a wallet.
- `timeout_ms`: one bounded worker pass,100–15000ms; it is not a model execution
  timeout. Pending processor outcomes defer and release the lease for later checks.
- `poll_ms`: explicit1000–60000ms, `mode`: `once` or `watch`,
  `allow_loopback_http`: false except literal-loopback local fixtures.

JSON/configuration/password/credential files are read from protected canonical
paths; Unix owner/mode/symlink/hardlink checks fail closed. This custody entry point
does not pretend those checks implement Windows ACL validation. Requests cannot
provide filesystem paths, processor URLs or keys. TLS goes to the fixed Stripe
API, with redirects rejected and bounded response bodies/deadlines. Public worker
logs contain only outcome, refund ID and bounded reason code.

The adapter validates the signed review and original payment evidence, then
independently retrieves the PaymentIntent and original charge. Before sending it
checks remaining refundable value, dispute state, currency/account/live-mode and
the exact amount. It uses only the original payment method. After a lost response
it first queries the original payment's refunds (or the known exact refund ID),
checks metadata bindings, then independently retrieves the matching Refund.
Only Stripe `succeeded` produces signed completion. `pending` remains a retry;
`requires_action`, failed/canceled or contradictory data require review. A Stripe
success describes processor status, not a promise that the customer's bank has
already posted the refund.

An identical POST may be retried inside the configured conservative window using
the exact retained body/key. Both lease and retry window are checked immediately
before sending. The earliest journal timestamp never moves forward. After the
window, lookup can still confirm the original result, but absence never permits a
new POST: it becomes explicit review. Incomplete bounded lookup, duplicate matches,
corrupt journal or unknown state likewise cannot authorize a replacement refund.
See [Stripe idempotency rules](https://docs.stripe.com/api/idempotent_requests),
[refund creation](https://docs.stripe.com/api/refunds/create), and
[refund statuses](https://docs.stripe.com/api/refunds/object).

SITE retains signed final delivery evidence and a unique physical delivery key.
Completion replay survives a lost ACK without releasing the original allocation,
erasing the collection claim or changing gross receipt history. Delivery records
are immutable, and a later admission review cannot erase a return already made.
Financial journals must be backed up. Keep unresolved operations; confirmed
journals can be archived under the operator's approved financial retention policy,
not deleted by a timeout or a log-cleanup task. Automatic archive tooling is not
implemented here; each refund uses at most two small records.

Tests use synthetic Stripe transport, temporary custody directories and ephemeral
keys, including the actual Core adapter/worker through Nest/Fastify/PostgreSQL.
No Stripe TEST/live refund or other money movement is claimed. The TNK adapter
uses the existing MSB signing/validation format, independently verifies a receipt
in a signed Hyperbee snapshot, and is exercised through actual SITE HTTP and
PostgreSQL with a synthetic validator transport. Production MSB opening with the
operator's stored custody, actual-rail acceptance, TAP sending, public review/status,
approval renewal and installed-package/release qualification remain.

TNK writes a protected exact signed transfer before SITE grants dispatch. Only
absence at that transaction hash in a fresh canonical snapshot allows broadcast
of the retained bytes. Pending finality, a stale/forked reader, or an unavailable
canonical authority cannot authorize a replacement transaction. Signing and
amount conversion use existing MSB helpers; the platform wallet must cover the
approved amount plus the network fee, with no deduction from the return amount.
Missing custody funds defer with `refund_treasury_short`; expired authorization
is reviewable before dispatch. Exchange-origin senders are not inferred to be
safe return addresses: the signed review supplies the verified destination.

No history scan is used. After confirmed delivery, the original completion and
canonical prefix are retained, so an acknowledgement retry stays byte-identical
even as the ledger grows. A changed canonical prefix blocks replay and triggers
reconciliation. One unresolved transport operation retains its permit even after
the observing timeout; polling cannot accumulate more unresolved sends. Disabling
new admissions does not disable the canonical read used by an approved return.
SITE additionally requires crypto delivery from the original collection address.
