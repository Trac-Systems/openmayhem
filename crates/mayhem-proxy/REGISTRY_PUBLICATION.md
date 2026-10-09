# Trusted published registry reader

`mayhem_proxy::registry::publication` is an opt-in client for the SITE capability
registry's published metadata API. It supplies validated, pinned definitions to
the existing `registry::evaluate` and `registry::apply_controls` functions. An
operator can explicitly enable it for saved request-control preparation and
validation through the gateway proxy-control configuration described below. No
automatic registry origin, supplier selection or capability evidence is supplied.

The operator constructs `TrustedOrigin::https(origin)` and a `Reader`. The origin
must contain only a scheme, host and optional port: no credentials, path, query
or fragment. `TrustedOrigin::local_loopback_http(origin)` is a separate explicit
acceptance-test option for literal IPv4/IPv6 loopback addresses. Plain HTTP hostnames
and remote addresses are rejected. Request/provider bodies never select an origin
or endpoint. The client ignores proxy environment settings, follows no redirects,
sends no credentials and performs no automatic retry or alternate-source fallback.

The HTTPS origin is the administrative trust anchor. Canonical BLAKE3 hashes bind
the returned release and definition content, but are not publisher signatures or
proofs of provider capability. For definitions inherited from an earlier release,
the pinned lookup assertion comes from that trusted origin. The client does not
walk historical manifests to manufacture an independent membership proof.

## Public API

- `current()` returns a current release observed within `Limits.head_ttl`, or
  performs a refresh. A failed refresh never relabels an expired observation as
  current.
- `refresh_head()` explicitly fetches current release metadata. A lower revision,
  conflicting known revision or changed immutable identity is rejected. A direct
  successor must name the known release ID/hash as its parent. Larger revision
  jumps rely on the trusted source; no release-chain scan occurs.
- `pin_release(uuid)` obtains an exact retained release. Old reads remain allowed
  and never claim current-head freshness. A newly observed higher exact release
  raises the monotonic watermark and invalidates an older cached head's freshness.
- `lookup_exact(&pin, &references)` accepts 1–96 distinct ordered
  `{field_id,schema_revision}` references and returns an owned `Definitions`
  snapshot. Every missing reference is requested in one exact batch. Response
  order, cardinality, release identity/hash, document versions, definition
  identities/hashes and representation ETag must match. Missing, extra, duplicate
  or substituted references fail the complete batch. There is no newer-revision
  fallback or partial success.
- `resolve_closure(&pin, &roots)` additionally loads only the exact rule
  references reachable from those roots, in bounded batches at the same release.
  The total, including roots, is at most 96 definitions. Cycles are visited once.
  Referenced types/operators, endpoint applicability and request-control usage
  must agree. One reachable root graph cannot mix meanings of one field. Rules
  on filter-only definitions are rejected because the runtime does not execute
  them. No rule is executed and no default is inserted during loading.

`PinnedRelease` construction is private and binds the release to its configured
origin. The reader rejects a pin from a different origin. Its immutable metadata
is available through `metadata()`. Snapshots retain their own pinned definitions
when the head changes or cache entries are evicted. `Definitions::get(field_id,
schema_revision)` provides the lookup callback required by the existing registry
helpers; `documents()` exposes exact validated document projections.

```rust,ignore
let reader = Reader::new(TrustedOrigin::https(operator_origin)?, Limits::default())?;
let pin = reader.current().await?;
let definitions = reader.resolve_closure(&pin, &exact_references).await?;
let prepared = apply_controls(&request, endpoint, &endpoint_contract, &controls,
    |field, revision| definitions.get(field, revision))?;
```

Loading a definition never supplies an `Observation`, raises assessed assurance,
proves endpoint/connector support or grants operator verification. An absent
capability observation still evaluates to `Match::Unknown`. The actual endpoint
contract and normal authorization/economic limits remain mandatory at execution.
No metadata cache is shared with provider capability evidence.

## Bounds and cache behavior

The default operation deadline is 5 seconds, including all batches in a closure;
configuration permits 50 milliseconds through 30 seconds. A semaphore admits at
most four concurrent operations by default (configurable 1–16). Excess operations
fail immediately with `Busy`, without an unbounded queue. Head TTL defaults to 60
seconds and is constrained to 1–300 seconds. There is no background refresh task.

A definition must satisfy the existing 16 KiB schema limit. Release responses
are bounded at 32 KiB. Lookup response bounds derive from the requested reference
count times the definition limit plus document/envelope overhead; a legitimate
large batch is not rejected by an arbitrary 256 KiB ceiling. Content-Length and
streamed bytes are both checked. Parsed snapshots are separately bounded by 96
documents and `MAX_SNAPSHOT_BYTES`, including serialized document metadata.
Snapshot retention by a caller is explicit ownership, not an unbounded reader
history. Responses must be JSON with a matching strong representation ETag;
redirects, unexpected status codes, unsupported content encoding and malformed
or oversized data are errors. A 404 is explicit `NotPublished`, never an empty
registry.

Release and definition entries share an independent FIFO metadata cache. Defaults
are 512 entries and 8 MiB; configurable maxima are 4,096 entries and 64 MiB. Both
count and serialized-byte budgets trigger eviction. Definition cache keys include
the exact release ID/hash and semantic reference, so snapshots never mix releases.
One bounded current-head observation and high-water identity remain outside FIFO
eviction. The watermark lasts for this reader instance; restart does not claim
durable rollback protection. Existing pins continue to carry their original
identity. No whole-registry scan or persisted full snapshot is needed.

The strict SITE wire uses explicit required nullable manifest parent fields and
the existing required nullable definition fields. Decimal-string release revisions
are bounded by signed PostgreSQL BIGINT; document/semantic revisions by positive
signed 32-bit integers. Unknown fields, malformed identities, noncanonical hashes
and inconsistent first-release/parent relationships are rejected.

## Focused validation

`tests/fixtures/registry-publication-v1.json` is an actual public SITE/Nest response
from an isolated PostgreSQL publication. It contains synthetic metadata only.
`tests/registry_publication.rs` uses that fixture and an in-process loopback HTTP
server to test exact wire/hash compatibility, origin boundaries, strict rejection,
monotonic head refresh, retained historical revisions, cache bounds, large valid
batches, deadlines/concurrency, closure limits and existing control/evidence
behavior. These tests make no live provider or payment calls.

## Connected saved request controls

The gateway proxy-control configuration accepts optional
`registry: {origin, allow_loopback_http}`. Omission keeps registry-dependent
controls unavailable. `origin` is fixed operator input; the HTTP option is only
for explicit literal-loopback local acceptance. The gateway uses the reader's
bounded defaults above. No request, profile, offer or provider supplies its URL.

Authenticated `POST /v1/proxy/profile/prepare` accepts exactly
`{schema_version:1, endpoint, request}`. The existing request must select one exact
proxy offer and carry a profile with explicit `request_controls`. The route
resolves the current published release, or the exact
`request.proxy.registry_release: {release_id, release_hash}` when supplied. It
loads the referenced semantic revisions and their closure, bounded to 96, then
applies controls against the selected supplier's actual endpoint descriptor.
Registry defaults are never inserted. Conflicting explicit values, unsupported
paths, descriptor limits, request limits and metering/price bounds fail closed.
Four preparation reads and four retained CPU validation permits bound work;
preparation and fresh validation have ten-second deadlines.

The response has kind `profile_preparation`, the explicit materialized `request`,
the pinned release, profile/control/content hashes, canonical offer/membership,
contract/recipe hashes, and bounded observation/expiry times. `preparation_hash`
uses domain `mayhem/proxy/profile-preparation/v1` over canonical response metadata,
excluding `request` and `preparation_hash`; `request_content_digest` and
`controls_hash` bind the exact body and controls separately. This avoids changing
request-number canonicalization or any existing retail fingerprint.

Callers must present that materialized request unchanged for their financial
quote and execution. New estimates and admissions revalidate the exact pinned
release, controls, descriptor and current offer. They never silently add fields
to the authorized body. Original-job replay runs before fresh registry reads, so
later publication changes or a registry outage do not reprice or invalidate an
accepted job. Preparation does not create a job, reserve credit/capacity or invoke
inference. Bad mapped values return `proxy_profile_controls_invalid`; missing
trusted registry/revisions/evidence return `proxy_profile_evidence_unavailable`.

Capability/data-handling predicates, verified-operator requirements and category
tags/variants still require authoritative observations and remain unavailable
when those are missing. Published definitions are semantics, not those
observations. Automatic category enumeration/ranking and native fallback are not
part of this path.

Gateway tests in `openai/proxy_buyer/tests/profile.rs` exercise real local HTTP
preparation and the existing quote/bridge/admission/replay machinery, with a
synthetic published registry and provider/financial doubles. The emitted fixture
is consumed unchanged by SITE's strict response/hash decoder. SITE separately
tests saved-profile ownership through real isolated PostgreSQL/Nest and a local
Core-transport double returning that actual projection. These are local boundary
checks, not real-provider or mainnet acceptance.
