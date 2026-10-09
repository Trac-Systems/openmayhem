# Typed proxy fields and request controls

`registry` validates revisioned data for proxy capability filters and explicit
request controls. It is shared policy logic, not an activated taxonomy service:
admin persistence, authenticated publication, candidate observations and routing
must supply its inputs before it can authorize a real selection. Native routes,
pricing, settlement and endpoint implementations are not changed by this module.

The opt-in [`publication` reader](REGISTRY_PUBLICATION.md) obtains exact published
definitions from an operator-configured trusted SITE origin, pins release IDs and
hashes, and resolves bounded reference closures for these functions. It is a
separate metadata cache and is not wired into routing or capability evidence.

Definitions carry endpoint scope, typed allowed values, comparison operators,
localized labels/help, optional units, UI grouping, evidence requirements and
optional conditional rules. They cannot execute scripts or add new billing or
endpoint semantics. Semantic changes require a new revision; label/help/order/
group changes may retain a revision. Saved references always resolve their exact
revision. The digest is BLAKE3 of the domain `mayhem/proxy/registry-definition/v1`
plus a NUL and Core stable JSON; it is an integrity binding, not a signature.

Missing, unsupported, stale and insufficiently trusted observations are distinct.
None satisfies a hard filter. Trust comes from the caller's authenticated evidence
store, never a provider-supplied trust label. Effective freshness is the stricter
of registry and profile requirements, and required assurance is the stronger.
Thus `max_age_ms:null` in a profile cannot waive registry freshness. This module
cannot grant T4 or establish that a remote model is the claimed weight set.

Values retain exact types. Decimals are canonical strings with at most18 whole
and18 fractional digits and compare without floating-point conversion. Enums
preserve case, spaces and Unicode; they have no invented cross-vendor ranking.
Sets are sorted unique UTF-8 values. Integer values stay in interoperable JSON's
safe range. The shared fixture under tests/fixtures is also exercised by the API
profile schema; passing syntactic validation does not mean a field is supported.

`apply_controls` resolves exact definitions and verifies their paths and values
against the selected public `EndpointFamilyContract`. Transport, credentials,
model identity, routing and conversation bodies cannot be mapped as controls.
An explicit request value may satisfy a conditional rule; conflicting saved
values fail instead of overwriting it. Absent values remain unknown: UI defaults
are never injected. Referenced raw controls also have their rules checked.
The original body is unchanged on failure; only a fully validated new body is
returned. Numeric transport rejects a selected decimal that JSON would round.

Work is bounded by32 explicit controls,96 referenced definitions,16KiB per
definition,8 rules per definition and8 predicates per rule branch. Definition
lookups are exact indexed/cache reads. Cycles use a visited queue, not recursive
execution. The caller must enforce its normal parsed-request size and admission
limits. No registry-wide, receipt or price-history scans occur here.

Focused acceptance covers unknown/stale/trust semantics, exact decimal boundaries,
semantic revisions, explicit/raw conditional combinations, cyclic dependencies,
endpoint contracts, immutable failure behavior and45 API/Rust wire cases. These
checks do not prove a deployed registry, UI, real provider capabilities or payments.
