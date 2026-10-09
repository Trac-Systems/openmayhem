# Published category routing

A saved profile can use `target.kind = "taxonomy_category"` with an exact
`taxonomy: {release_id, release_hash, entry_id, schema_revision}` pin plus the
existing explicit `variants`, `tags` and `market_allowlist` restrictions.
Legacy category-family, exact-market and exact-offer targets retain their wire.

The gateway reuses the operator-configured published-registry origin. HTTPS is
required except an explicit literal-loopback HTTP opt-in for local acceptance.
No request, provider, label, alias or response supplies an origin or fallback.
The `registry::publication::Reader` now provides `pin_taxonomy`,
`taxonomy_members` and `taxonomy_match`, sharing its existing concurrency,
deadline and entry/byte cache budgets. Taxonomy pins and field-definition pins
are separate administrative identities. Neither is capability evidence.

Release manifests are strictly decoded and domain-separated BLAKE3-verified;
release/reference/ETag, required nulls and exact model claims must agree. Known
changed documents must match their release delta. Canonical observation metadata
is historical publication context, not a current capacity or provider-trust
assertion. A manifest network, when present, must match the selected catalog.
Old pinned releases remain readable after later publications. Nothing consults
an unpinned head to reinterpret an old category.

## Bounded traversal and exact admission

The resolver holds one immutable canonical catalog snapshot for its session. A
step reads at most one taxonomy membership page (16 scopes, no more than 256
indexed SITE rows) and one ordinary candidate index page. Empty continued pages
are progress. There is no total category membership count limit or conversion
into a 64-element family list. The reader permits at most 100 scopes per page,
32 exact model tuples per match call and 256 KiB per page/match response; a pinned
release is at most 32 KiB. Owned session metadata and cached releases share the
existing resource accounting and retention bounds.

Canonical index version 4 adds an exact model-tuple-to-market index. Family and
model scopes are disjoint under SITE's publication contract, and each scope prefers
its own index when sampled cardinalities are unknown. A fully exhausted smaller
provider/price driver can still win. Older local derived indexes become unhydrated and
refresh through the existing bounded catalog mechanism; startup never scans a
ledger or synchronously rebuilds the entire old catalog. The original policy,
rail and scope bind every cursor. No publication digest changes.

Every candidate still passes endpoint/rail/provider/context and the complete
signed rate-map restrictions, then actual descriptor, request-control,
capability evidence, metering, maximum-cost and presence checks. Unsupported
variants/tags, verified operators and data-handling requirements remain errors.
Labels and claimed families cannot confer assurance. Minimum-cost claims remain
`lowest_maximum_among_validated_candidates`, only after every matching scope is
exhausted and no unknown candidate remains. Pending work never claims a winner;
resource/retention failure requires an explicit fresh resolution.

Quote, continuity and new admission use the exact indexed `/match` endpoint for
the selected canonical model tuple, never a membership walk. The resulting
membership value has private fields and cannot be deserialized from user JSON.
Generic synchronous `Policy::check_offer` refuses taxonomy targets without this
proof. Preparation materializes controls before request hashes; estimate and new
admission validate that exact body again. Original accepted-job replay occurs
before current metadata reads, preserving its original terms during an outage.
A new request fails closed when its pinned membership cannot be read.

## Local acceptance boundaries

`tests/taxonomy.rs` consumes actual SITE Nest/PostgreSQL release/membership/match
fixtures and rejects malformed, substituted and missing metadata. Directory tests
exercise exact model indexes and scope-bound continuations. The gateway's
`taxonomy_publication_reaches_same_running_resolver_estimate_execution_and_replay`
test runs one HTTP listener/process and makes newly published category/field
fixtures visible without restarting it. It exercises control materialization,
quote, execution, original replay, metadata outage, a later release, and 132
scopes plus an empty continued page. The metadata listener replays real exported
SITE responses; synthetic extra empty scopes test bounded progression. Canonical
execution uses the existing isolated signed canonical fixture and actual worker;
this is not a production publisher, real upstream model, or live-money proof.

Focused commands from the Core checkout (use the operator's configured Cargo
build directory if desired):

```
cargo test -p mayhem-proxy --test taxonomy
cargo test -p mayhem-proxy --test directory candidates
cargo test -p mayhem-gateway --lib taxonomy_publication_reaches -- --nocapture
```
