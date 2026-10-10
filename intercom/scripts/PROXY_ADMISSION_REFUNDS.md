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
  FIAT/TNK/TAP adapters are pulled. Permitting a rail in policy alone does not
  enable its sender; its custody configuration must also be supplied.
- `executor_key_file`, `executor_password_file`: encrypted Ed25519 PKCS8 and its
  exact passphrase bytes. No private key is supplied over the worker API.
- `journal_root`: existing canonical owner-only directory, mode0700. Records are
  fixed-name, bounded16KiB, mode0600, durable and immutable; no directory scan.
- `stripe`: exact `account`, `livemode`, `currency`, `credential_file`,
  `retry_window_ms` and `max_lookup_pages`. The retry window must be1000ms through
  23hours; choose it deliberately. The bounded lookup is scoped to one original
  PaymentIntent,100records/page, at most1–16configured pages. No global history scan.
  Set `stripe` to null for a crypto-only worker; existing FIAT configurations remain valid.
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
- Optional `tap`: exact fields `chain_id`, `token_contract`, `receiver`,
  `key_file`, `password_file`, `rpc_urls_file`, `max_gas`, `max_fee_per_gas`,
  `priority_fee`, `max_fee`. Custody is existing encrypted secp256k1 PKCS8;
  the password is read as exact bytes and the derived address must match
  `receiver`. Chain/token must match the original payment. `rpc_urls_file` is
  a protected JSON array of one to four operator URLs, ordered primary first.
  Credential paths are supported; use the approved paid RPC first and deliberate
  fallbacks after it. No caller-supplied URL or redirect is accepted. Fees are
  positive decimal integers: gas units, wei per gas and total wei respectively.
  The operator explicitly sets all four bounds. The estimator adds 20% gas margin
  within those limits; it never enlarges a limit when funds or fees are inadequate.
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
implemented here; each refund uses at most two small records. TAP also retains
one small claim per assigned Ethereum nonce; it is part of financial recovery
state, not an expendable log. No history or directory scan is used for recovery.

Tests use synthetic Stripe transport, temporary custody directories and ephemeral
keys, including the actual Core adapter/worker through Nest/Fastify/PostgreSQL.
No Stripe TEST/live refund or other money movement is claimed. The TNK adapter
uses the existing MSB signing/validation format, independently verifies a receipt
in a signed Hyperbee snapshot, and is exercised through actual SITE HTTP and
PostgreSQL with a synthetic validator transport. Production MSB opening with the
operator's stored custody, actual-rail acceptance, public review/status,
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

TAP retains an exact signed type-2 ERC-20 `transfer` plus an immutable nonce claim
before requesting SITE dispatch. The transaction codec checks chain, custody,
token, destination, amount, calldata, zero ETH value, nonce and every gas bound.
The nonce claim covers the whole chain/account, including returns of different
tokens, and prevents simultaneous refund workers using the same journal root from
claiming it for different operations. All return workers for the same custody
must share that durable root. Inventory other processes using that signing key:
there is no promise of exclusive nonces when an unrelated signer acts outside
the worker. An observable pending unrelated nonce defers; a consumed retained
nonce without this transaction requires review, never an automatic replacement.

Every retry first reads the retained transaction hash. A pending transaction or
unfinalized receipt does not rebroadcast. A lost broadcast ACK returns to
reconciliation; it does not blindly send through every fallback. If both receipt
and transaction are absent, and latest/pending nonce still match, it can broadcast
only the retained signed bytes. Fee increases never silently replace a transaction.
Insufficient TAP or ETH, or a fee above policy, defers with a specific reason.
The platform pays ETH gas in addition to the exact returned amount. The wallet
must have enough of both; a token balance is not gas funding.

Completion requires a successful receipt in a canonical finalized block, exactly
one transfer from the original custody to the approved destination for the exact
amount, and repeated receipt/block checks to reject observed changes during
verification. A finalized revert requires review. The retained delivery is replayed
unchanged after a lost SITE acknowledgement. Missing or contradictory journal,
nonce or chain evidence never authorizes fresh payment. Unsent nonce claims are
not automatically deleted on lease/approval expiry: expiry does not prove that
the signed transaction was never broadcast. Operator recovery/renewal must retain
this ambiguity and the original preparation.

TAP RPC calls occur only while processing a leased return, with a 15-second outer
worker bound, four-second endpoint attempts and bounded response sizes. An idle
worker polls SITE only. Read-only admission discovery is unchanged. The separate
refund RPC accepts a narrow method set and only signed raw transactions for
broadcast; it does not use RPC-managed signing, approvals or pool deposits.
The signing codec and its pinned production `ethers` dependency ship inside
Intercom's runtime layout; no sibling contracts checkout is required. Source
installations must install from the updated Intercom lockfile. Native packaged
dependency/build qualification remains part of the release gate.
Acceptance includes a real ERC-20 transfer on an ephemeral local EVM through the
actual worker, SITE HTTP and PostgreSQL, checking token balances, gas and completion
replay. Local EVM finality does not prove Ethereum mainnet confirmation timing.
No real TAP/TNK or Stripe funds were moved by these checks.


## Provider status access

Providers read approved return progress through their existing wallet-authenticated
setup flow; execution and reviewer credentials are never required. The guided CLI
has `n Admission returns`. Noninteractive callers use the existing wizard action
file with `{"action":"admission_returns","expected_revision":2,"after":null}`,
then the returned `return_page.next_cursor` as `after` for another page. Use the
current checked draft revision in place of the example revision.

The local provider dashboard exposes the same view under Admission fee returns.
It shows exact original-rail amounts, reviewed destinations, pending confirmation,
operator/funding/approval problems and completed returns. FIAT completion refers
to processor confirmation; bank posting may follow. It does not move money,
change a pending transaction, clear allocations, renew approvals, or overwrite
saved enrollment state. Its bounded response is authenticated against the exact
provider/network/page request. New admission collection can be disabled while
these reads remain available. Operator review/renewal workflows still need their
separate implementation and acceptance before financial activation.
