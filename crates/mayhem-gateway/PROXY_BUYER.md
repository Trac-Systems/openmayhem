# Proxy buyer integration

This is local integration groundwork, not an enabled paid HTTP endpoint. The
read-only directory and `mayhem use --proxy-config` do not authorize spending.
Native requests and their runtime/payment paths remain unchanged.

`openai::proxy_request` resolves exact offers independently of native model names.
Its internal candidate model selector is
`proxy/offer/<market digest>/<provider public key>/<offer slot digest>`. Friendly
market/category selectors remain separate work. A malformed proxy selector never
becomes a native selection.

The candidate request envelope has a `proxy` object containing the complete
`prices` rate map, per-request fee cap, session-minimum cap, total-spend cap,
payment rail, pinned settlement policy, output allowance and optional minimum
context/throughput and verified-operator requirement. The gateway strips only
that envelope before passing the owned request to the provider protocol. This
syntax is not advertised as a usable public API until the complete dispatcher is
wired and accepted.

The gateway owner supplies a separate resolved policy revision and explicit
epoch lifetimes. Request identity includes the authenticated buyer/key owner,
endpoint, exact selector, full request, every price/rail/filter control and that
policy. Reordering JSON does not change the identity. Identical text from another
invocation is not an idempotency key; the caller must bind its opaque key to this
fingerprint and preserve the original billing/session identity across retries.

Candidate resolution reads the current indexed catalog and the same signed
presence eligibility used by the proxy control plane. It does not reserve a
slot. Prices, accepted rail, advertised served context, endpoint and exact
membership/recipe must match. The current directory has no authenticated T4
evidence, so a verified-only request fails closed rather than trusting an
operator label. The ordinary LLM throughput floor remains in shared presence
eligibility; Decisions do not invent a token-speed guarantee.

At most eight candidate reads run concurrently. Each permit remains with its
blocking storage operation if the HTTP caller disconnects. There is no
all-catalog subscription, history scan, admission queue or per-output-token read.

## Mandatory dispatch ownership

Before wiring this parser into Chat/Decisions handlers, the gateway must own:

- Protected durable HTTP job/idempotency → billing attempt, supplier, endpoint,
  policy, rail and authenticated key attribution.
- Durable key-budget exposure before a buyer signature can leave. Reserve the
  actual accepted terms' maximum cost, not an unrelated caller spending ceiling.
  Unknown signing/execution outcomes retain the original exposure until resolved.
- A separately enabled buyer controller, existing wallet authority, protected
  negotiation/recovery journals, bounded session and decoder resources, and
  joined shutdown. Discovery alone must never enable paid work.
- Durable verified output before acknowledging its receipt. Recovery of a
  canonically paid purchase reads the saved answer; a paid receipt without an
  answer is not a successful response and must not trigger a second purchase.
- One bounded recovery supervisor covering pre-publication negotiation as well
  as published reservations. Reconnect using the original Recover/Status path;
  do not resend Execute or fall through to a different rail/native provider.

`mayhem_proxy::buyer_controller` supplies the owned negotiation/execution layer.
It rechecks the provider proposal and a fresh canonical quote, persists signing
and acceptance, confirms funding, independently verifies output/receipts, and
requires explicit owner hooks for budget authorization and result retention.
Those hooks are integration boundaries, not implementations of HTTP persistence.

The first controller increment covers nonstreaming Chat and Decisions. Streaming,
Completions/Responses execution, category routing, automatic bounded recovery,
retail accounting and Studio/MCP dispatch remain acceptance work. No request may
advertise support based only on the parser recognizing its endpoint.
