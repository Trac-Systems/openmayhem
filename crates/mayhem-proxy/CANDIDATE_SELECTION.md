# Indexed saved-profile candidates

`CatalogRead::proxy_candidates(&policy, rail, cursor, limit, now_ms)` provides
incremental **canonical candidate enumeration** for a saved `routing::Policy`.
It does not select a winner, execute inference, reserve capacity/credit, publish
metadata, or turn registry definitions into provider observations. The gateway
resolver consumes these pages, validates each actual request and owns the full
progression before making a ranking claim.

The supplied policy is fully validated and the selected rail must be one of its
explicit rails. Every returned publication passes its exact offer/market or
category-family/market-allowlist target, provider allow/deny, endpoint, rail,
served-context, complete rate-unit map and all per-unit/fixed charge ceilings.
The current market/membership/offer must agree. The profile's wholesale total,
retail cap, settlement policies and other fields remain bound in the query hash;
a candidate row is never a maximum usage quote or permission to spend that cap.

Capability/data-handling predicates, verified operators, tags/variants, throughput
and request controls still require their ordinary evidence/descriptor checks.
`requires_observation_resolution` and `requires_control_preparation` report those
pending profile requirements; they are never marked satisfied by enumeration.
`catalog_eligible` remains a separate canonical-registration fact. No provider
name, declared concurrency, category claim or index row establishes capacity,
T4 or remote weights. Native models and native indexes are unchanged.

## Maintained indexes and bounded work

Index version 4 maintains row-local entries to the existing transactional derived
index. Offer entries cover endpoint/rail, provider, exact unit-set, each rational
unit price, per-request charge and session minimum. Membership entries cover
served context; market entries cover claimed family and the exact canonical
model tuple (family, model ID, revision, quantization). A price or membership edit
updates only that canonical row's index keys, without scanning sibling offers.
There are at most 23 new index entries per active offer (three rails, 16 rates,
provider, unit-set and two fixed charges), one per active membership and two per
market. Withdrawal/deletion removes the prior entries in the same transaction.

Prices use integer-only sortable keys: a u128 whole part and 106 fractional
binary digits. Canonical granularity is at most `2^53-1`; unequal fractions with
those denominators differ by more than `2^-106`, so the key preserves exact
rational ordering, including equal fractions and u128 extremes. Final filtering
still calls the shared `PriceLimits::permits`, checking every unit and fixed
charge. No decimal display rounding is used for selection.

A bounded planner samples at most eight offers/parents per possible driver and
selects one deterministic driver for ordinary targets. Pinned taxonomy scopes
prefer their family/model index when sampled cardinalities are unknown; a fully
exhausted smaller provider/price driver can still win. This prevents repeated
whole-category scans while retaining selective hard-filter access. Market/member samples include bounded child
offer reads, so a single large market is not mistaken for a one-offer scope.
This is a selectivity heuristic, not an optimal query-plan or ranking claim.
Every page permits at most 256 examined driver/offer rows and 2,048 index seeks,
including planning and empty ranges, plus at most one exact parent lookup when
validating a nested continuation. It returns at most 100 publications and
128 KiB of publication data. Empty pages can have a continuation. These are
per-operation bounds; there is no total catalog or candidate count cap.

The cursor fixes the driver, query hash, content snapshot, range and nested
market/member-to-offer position. Updates that change content expire a cursor on
a newer `CatalogRead`; no-op refreshes preserve it. An already retained
`CatalogRead` remains an immutable MVCC snapshot and can finish its traversal
while the catalog advances. A downstream owner must bound how long/how many
such snapshots it retains, then revalidate current admission facts. Index-version
migration uses existing bounded rehydration, preserves public rows and does not
scan/rebuild the old catalog synchronously at startup.

## Downstream contract and minimum-cost ranking

The returned `CandidatePage` contains:

- `schema_version: 1`, `query_key`, `snapshot`;
- `ordering: "index_traversal_not_cost"` and diagnostic `index_driver`;
- full canonical `entries`, preserving individual provider/offer attribution and
  complete signed rates, membership, endpoint-contract and recipe hashes;
- `next_cursor`, `exhausted`, `scanned_candidates`, `index_reads`;
- the two pending-validation flags described above.

Begin with no cursor and continue until `exhausted` is true. The connected resolver
retains a bounded accumulator (best exact quote, its materialized request,
validation/exclusion state and continuation) instead of storing all candidates.
Each step can return `continue` and accept another bounded step. This allows a
large category to finish; broad scope is not permanently rejected merely for
exceeding one page. A cursor is an enumeration position, not authenticated proof
that a caller processed earlier pages. The resolver must own/verify the complete
progression from the beginning before claiming a minimum.

For each considered candidate, run the complete actual request through its
actual descriptor, pinned controls, authoritative evidence and shared maximum
metering calculation. Apply account/key/project/profile restrictions and the
same frozen retail pricing rules. Preserve failures/unknown validation honestly;
never label a partially checked scope globally cheapest. Only after completing
the relevant scope may the resolver compare exact maximum costs, including all
units, fixed charges and session minima, with deterministic ties. That is a
**lowest maximum cost**, not a forecast of actual cost. There is no single
request-independent price scalar that can rank arbitrary full rate maps and
custom contracts correctly. Preferred-speed ranking still needs comparable
fresh measurements. Final selection must recheck the concrete offer/evidence;
accepted-job replay retains its original binding.

## Focused local acceptance

The directory integration tests exercise category unions, exact market scopes,
provider/context/rail/price selectivity, empty continuation pages, full price maps,
rational limits, immutable read snapshots, cursor substitution, row updates,
withdrawals, restart and index migration. Existing directory, matching and catalog
regressions use the same derived index.

The explicit scale test creates 100,000 synthetic offers through the normal
bounded catalog hydration API, finds deep provider/context/rail/price matches
using selective index ranges, then traverses every offer exactly once to
completion. It uses a temporary database and no network or ledger scan:

```sh
cargo test -p mayhem-proxy --test directory hundred_thousand -- --ignored --nocapture
```

The rational-key unit test checks equivalent/extreme/adjacent fractions and
10,000 deterministic pairs against exact cross-products. These tests establish
local indexing behavior, not a connected gateway/Studio/MCP routing surface or
production evidence policy.


## Published taxonomy scopes

`proxy_candidates_in_scope` accepts one validated administrative family/model
scope for a `taxonomy_category` policy. Its cursor binds the complete original
policy, rail and exact source scope. It never expands the category into the
legacy `family_ids` array. The taxonomy reader and resolver continue across every
membership page and hold only one bounded scope page at a time. Calling ordinary
`proxy_candidates` for a taxonomy target fails closed. A manually constructed
scope can narrow read-only enumeration but cannot satisfy `Policy::check_offer`;
execution requires an exact reader-created membership proof. See
[TAXONOMY_ROUTING.md](TAXONOMY_ROUTING.md).
