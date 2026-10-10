# Platform admission-return worker

Local implementation only. Do not enable collections or return workers without
the approved commercial policy, reviewed credentials and deployment approval.
This does not make the one-time admission fee generally refundable.

The FIAT adapter preserves the exact Stripe refund request and stable idempotency
key in an owner-only journal **before** requesting dispatch authorization from
SITE. The signed review fixes the invoice, verified original payment, amount,
rail, method/destination and policy. The distinct executor key signs preparation
and delivery; neither provider nor admission-permit keys can exercise that role.
No retail credits, inference payouts or ledger mutations are performed.

`proxy-admission-refund-worker.mjs` is an explicit, disabled-by-default entry point.
Set `PROXY_ADMISSION_REFUND_WORKER_ENABLED=1` and
`PROXY_ADMISSION_REFUND_WORKER_CONFIG` to an operator-owned protected JSON file
only after approval. Its exact fields are:

- `api_origin`, `api_credential_file`: fixed SITE origin and the dedicated executor
  credential, not an admission verifier/issuer or retail token.
- `policy_file`: exact approved `RefundPolicy`, including public review/execution
  key sets. It must permit FIAT; the worker pulls FIAT only even if the approved
  policy also permits other rails.
- `executor_key_file`, `executor_password_file`: encrypted Ed25519 PKCS8 and its
  exact passphrase bytes. No private key is supplied over the worker API.
- `journal_root`: existing canonical owner-only directory, mode0700. Records are
  fixed-name, bounded16KiB, mode0600, durable and immutable; no directory scan.
- `stripe`: exact `account`, `livemode`, `currency`, `credential_file`,
  `retry_window_ms` and `max_lookup_pages`. The retry window must be1000ms through
  23hours; choose it deliberately. The bounded lookup is scoped to one original
  PaymentIntent,100records/page, at most1–16configured pages. No global history scan.
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
No Stripe TEST/live refund or other money movement is claimed. Delivery wire and
storage support all three rails, but this entry point currently executes FIAT only.
TNK/TAP execution integration, public status/review/approval renewal, actual-rail
acceptance and installed-package/release qualification still remain.
