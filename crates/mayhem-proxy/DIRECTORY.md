# Proxy offer directory

This local implementation exposes public, admitted proxy publications separately
from native models. It does not dispatch inference, reserve credit, charge a
customer or change a presence subscription. Published rates are not execution
quotes. No production activation is implied by these components.

The gateway's explicit `mayhem use --proxy-config PATH` control owns canonical
catalog refresh and signed presence. With no proxy configuration, the read API
returns `proxy_directory_disabled`; native endpoints retain their behavior.

## Read API

Both endpoints use the existing gateway's authentication rules:

- `GET /v1/proxy/offers`: `kind` (`llm` or `decisions`), `family_id`,
  `name_prefix`, `endpoint`, `minimum_context`, `rail`, `limit`, `cursor`.
  Endpoint values are the declared wire names (`openai_chat_completions`,
  `openai_completions`, `openai_responses`, `mayhem_decisions`). Rails are
  `fiat`, `tnk`, `tap`. Default page size is 50, maximum 100.
- `GET /v1/proxy/offers/{market}/{provider}/{slot}`: exact public offer lookup,
  independent of filters or pages. Missing/deleted records return 404. An existing
  withdrawn record remains inspectable with `active: false`.

Pages return `query_key`, `snapshot`, `entries`, `previous_cursor`, `next_cursor`
and bounded `scanned_candidates`. No exact total or global rank is calculated.
Entries retain the full canonical descriptor, membership, offer, digest and all
rates, including request fees and minimum session amounts. Atomic-unit amounts
retain their exact wire representation. No input/output prices from different
suppliers are combined. Family labels come from canonical catalog data.

`catalog_eligible` means only that current canonical registration and policy
validate. It does not mean Available, Verified, reserved capacity or an execution
quote. Operator verification currently returns `unknown`. A UI must show checking
until a genuine live presence overlay is connected. Actual admission still needs
the buyer's constraints, current signed offer, health/capacity and financial gate.

## Paging and resource behavior

The private redb cache contains four bounded name/family indexes per market.
Market descriptors are immutable by content identity. Traversal walks indexed
markets and their provider/slot ranges with at most 256 candidates and 128 KiB of
entry data per page. There is no total public-offer cap, whole-catalog copy,
history scan or per-browser snapshot. Reads hold one short MVCC transaction.

Search is explicitly case-insensitive model-name **prefix** search and name
ordering. Price/availability sorting and additional taxonomy/trust filters are
not exposed until their indexed implementation and authoritative sources exist.
Endpoint/context/rail predicates consume bounded continuation pages. Therefore a
page may be empty while still containing a next cursor; clients must retain that
cursor and allow continued browsing, without an unbounded automatic fetch loop.

Cursors bind the exact query and catalog content snapshot, and can traverse in
both directions. Ordinary ledger advancement and identical refresh rows do not
expire them. Actual public row changes invalidate a traversal with HTTP 409
`proxy_directory_cursor_expired`; the client restarts that query visibly while
preserving filters and the selected stable market/provider/slot ID. Full snapshot
replacement and explicit private-cache reconstruction also invalidate cursors.
An exactly full final page can have a conservative continuation to an empty final
page. Returning from it includes the prior boundary row.

Derived-index upgrades discard only derived indexes and incomplete staging,
retain committed public data, and rebuild through bounded background hydration.
No canonical/native/financial store is reset or scanned at startup. Catalog
incarnation plus content revision fences old cursors across private-cache rebuilds.

Gateway directory disk work runs in a separate bounded blocking pool admission
(eight active reads, no waiting queue). A disconnected HTTP reader retains its
permit until its bounded database work ends. Overload returns retryable 503
`proxy_directory_busy`, without taking inference slots. Replies use `no-store`.
Other public errors distinguish disabled, temporarily unavailable, invalid query,
expired cursor and missing offer; internal paths and storage errors are omitted.

## Local verification and remaining integration

Fixtures exercise large multi-page offer traversal, sparse filters, 600 empty
markets, case/family/endpoint separation, backward navigation, byte budgets,
empty-terminal backtracking, direct selection, no-op refresh, repricing,
withdrawal/deletion and both preceding private-index migrations. These are local
canonical fixtures, not deployed proxy providers or paid-network acceptance.

The separate website catalog consumes these read shapes. Retail API normalization,
taxonomy administration, live availability subscriptions, buyer category routing,
Studio/MCP and paid dispatch remain separate integration work. Their release and
mainnet gates are unchanged.
