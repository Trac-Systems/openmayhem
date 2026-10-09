# Proxy operator identity verification

`require_verified_operator` means that the exact provider public key signing the
selected canonical proxy offer has an active canonical provider record and a
current admin-verified KYB record. It reuses the existing native identity facts;
it does not claim T4 inference integrity, hardware attestation, model identity,
capabilities or data-handling guarantees. Provider labels and summaries never
satisfy it. No new ledger command or backfill is required.

The configured local peer exposes the read-only
`POST /v1/proxy/operator-state` relay. It authenticates the existing canonical
admin service, binds a fresh requester nonce and network, and reads only
`prov/<provider>` and `kyb/<provider>` in one signed snapshot. Its public response
contains status and the existing proof hash, not legal names, KYB references or
admin signatures. The relay request is at most 1 KiB; the response is at most
4 KiB. Both the service and Rust reader permit four concurrent reads. Timed-out
canonical work retains its service permit until cleanup. There are no scans,
subscriptions, persistent identity caches, inference calls or ledger writes.

The Rust reader pins network identity, checks nonce, proof continuity and epoch
against the catalog, and retains a single proof high-water mark. An observation
is usable for at most 15 seconds from read start, checked with monotonic and wall
clocks. It cannot be constructed by deserialization. Quotes expire no later than
that observation. Requests without the filter do not make this additional read.

Selection distinguishes definite canonical absence/revocation/inactivity from
unavailable, malformed or unknown evidence. The former excludes a candidate;
the latter leaves it unresolved and prevents a complete cost-ranking claim.
Before a new purchase reaches owner authorization, a fresh read rechecks the
operator and exact offer. Existing accepted jobs replay their original terms
before these new reads, including during revocation or service outage.

This is a current authenticated observation, not an identity lease: revocation
can occur after any completed read. A directory entry without such an observation
continues to report `operator_verification: "unknown"`; it is not a reusable
authorization token. Older peers lacking the relay fail unavailable for the
verified-only path. They do not affect ordinary unfiltered requests.

Local acceptance uses synthetic keys, a signed canonical test view, the real
admin KYB command and the existing fixture backend. It does not establish live
provider, real-payment or production rollout readiness.
