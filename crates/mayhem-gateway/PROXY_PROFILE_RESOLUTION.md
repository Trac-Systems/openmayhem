# Read-only saved-profile resolution

`POST /v1/proxy/profile/resolve` requires an existing authenticated gateway key.
It reads the maintained canonical candidate index, actual authenticated public
descriptors, registry controls and signed presence. It creates no HTTP job,
financial negotiation, reservation, buyer signature, model request or receipt.
Normal gateway authentication/rate accounting remains in effect.

Start with the exact provider request and existing proxy controls, including the
full saved `profile`, but omit `request.model`:

```json
{
  "schema_version": 1,
  "kind": "start",
  "endpoint": "openai_chat_completions",
  "request": { "messages": [], "proxy": {} },
  "previous_model": null,
  "model_allowlist": null,
  "retail_ranking": null
}
```

The empty controls above illustrate placement, not a valid price authorization.
The ordinary complete controls/profile schemas apply. Optional `model_allowlist`
contains at most 200 exact selectors, matching the SITE key schema; null/omission
adds no restriction and an empty list permits no model. It can only restrict the
authenticated Core key. The saved policy is never rewritten to a narrower target.
Optional `retail_ranking` is `{ "margin_bps": 333, "max_cost_micro": "1000000" }`.
The margin must be a safe nonnegative integer; its positive micro cap cannot
exceed the saved policy's retail cap.

The server owns traversal from the initial cursor. It freezes the full input,
policy, rail, key model permissions, extra allowlist, optional retail projection,
registry release and immutable catalog snapshot. Only this continuation is
accepted; caller-supplied catalog cursors, changed bodies or skipped revisions
are rejected:

```json
{ "schema_version": 1, "kind": "continue", "resolution_id": "64-hex", "revision": 1 }
```

Each response includes the current revision and the next `continuation` object
or null. One active step owns a resolution. A repeated previous input revision
returns its original completed response, including unchanged quote expiry.
Work already started finishes under bounded ownership after an HTTP disconnect.
Sessions belong to the authenticating token; another token cannot read or resume
them. Revocation and current key permissions are checked on every HTTP step.

`status` is `pending`, `selected`, `retained_compatible`, `no_match`, `incomplete`
or `refresh_required`. Pending responses never contain a selection. They include
`pending_reason` (`scope_remaining`, `presence_pending` or `observation_budget`)
and `retry_after_ms: 500`; non-pending responses use null and zero. Counters and
bounded exclusion counts describe progression, not a list of all candidates.
The server keeps only one page and the best exact result. There is no total
candidate limit. A scope may be exhausted while a final presence check is pending.

Unknown descriptor, capability, operator, taxonomy, throughput or presence facts
are never treated as matching evidence. Unresolved candidates prevent a minimum
claim. Preferred-speed ranking explicitly reports unavailable until comparable
authenticated measurements are implemented. `retain_compatible` checks the exact
provided previous model first; success reports continuity retention, not minimum
cost. All new execution continues through ordinary exact-offer admission checks.

Selection contains the exact materialized `request` (with model and proxy
controls), `model`, the existing wholesale `maximum_estimate`, and nullable
`maximum_retail_cost_micro`. Shared `PurchaseRequest::maximum` supplies metering
and wholesale cost. Optional retail ranking marks up each rate and fixed charge
with a ceiling, then applies usage/granularity ceilings and the session minimum,
then rounds AU upward to micro units. Fixed-width wide intermediates preserve
the existing SITE BigInt arithmetic without changing accepted wholesale terms.
Comparison is `(maximum_retail_micro, model)` when projected, otherwise
`(maximum_wholesale_au, model)`. Per-rate rounding can reverse wholesale ordering;
wholesale ranking therefore makes no retail-cheapest claim. SITE still enforces
its current account/key restrictions and independently checks the frozen retail
projection before financial authorization.

Common response fields bind the original full start request via
`request_content_digest` (including proxy controls), the sorted/deduplicated extra
allowlist via `model_allowlist_digest`, and the full original policy via
`profile_hash`. Content digests use the existing typed SHA256 retail content
encoding. The response also carries the snapshot/query identity, registry pin,
retention times, projection echo, ranking basis/claim, scope completion and
`authorizes_execution: false`. The selected estimate retains its ordinary
content hash and expiry; resolution is not a capacity or price lease.

Operator `ProfileResolutionLimits` defaults are 64 retained sessions, 256 MiB of
accounted serialized retained state, four active steps, four indexed candidates
per step, ten-minute retention and a fifteen-second per-candidate observation
deadline. These are configurable shared resource budgets, not per-customer
concurrency or a catalog-size ceiling. Existing endpoint/runtime request limits
remain unchanged; 98 KiB bounds the extra controls/allowlist envelope. Retained
accounting reserves six input copies plus 1 MiB for page/descriptor metadata.
CPU tasks retain separate bounded permits after caller cancellation.
The supported buyer JSON configuration accepts optional `profile_resolution`
with these six fields; omission preserves the defaults. Validation happens in
protected config loading before startup. Programmatic callers can use
`Runtime::with_profile_resolution_limits`.

Temporary presence interests share the existing authenticated receiver. Their
union with explicit selected markets respects `max_markets`; dropping one reader
cannot remove another reader or an explicit selection. A new subscription has
no guaranteed retained-message delivery: it can reuse only still-fresh local
Table evidence, otherwise it waits for the next authenticated heartbeat. The
observation deadline records an unresolved exclusion and permits further scope
progress. A winning interest is retained through the original quote window,
bounded by resolution retention; cleanup occurs on requests and the existing
two-second runtime tick. Replay never renews it. Shutdown drops retained state.
The existing configured `max_presence_routes` storage quota and persistent
controller/withdrawal replay fences remain intact; a storage-short gateway must
increase its operator quota rather than erase fences or claim unknown routes are
absent.

Expired/restarted sessions return `proxy_profile_resolution_expired`; begin a
fresh resolution. Changed catalog content or failed bounded work returns
`refresh_required`, never false completion. Final selection rechecks the exact
current offer, membership, contract/recipe binding and fresh presence. It cannot
silently substitute a newer price/revision into a completed comparison.
