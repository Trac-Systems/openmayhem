# Proxy observations and speed preference

The opt-in gateway conformance store records what this gateway actually received
and validated. It does not attest remote weights, native tokenizer identity,
privacy, data handling, operator verification, maximum context or capacity.
Native routing and payment policy are unchanged.

Add `conformance` to the protected proxy gateway control configuration. It has
strict fields `schema_version: 1`, `tester` (the gateway wallet public key),
`ttl_ms` (1–3,600,000), `maximum_records` (1–1,000,000), `maximum_bytes`
(16 KiB–16 GiB), `minimum_interval_tokens` (2–1,000,000),
`minimum_interval_us` (1–3,600,000,000), `mappings`, and required nullable
`tokenizer`. No public request can configure these values or submit observations.
The tokenizer is either null or `{file, digest, limits}`; the file is protected
local tokenizer JSON, pinned by BLAKE3, with the existing `health::native::Limits`.
Relative files resolve against the protected control file. There is no remote
tokenizer download or executable mapping.

A mapping contains exact `field_id`, `schema_revision`, `definition_digest` and
`assertion`. The supported assertions are `valid_endpoint_output`,
`validated_stream`, `validated_tool_call` and `validated_json_schema`.
These produce only boolean observations from actual validated output. The pinned
published definition must be a compatible boolean filter definition; field names
and localized labels are never interpreted as executable semantics. A mapping is
not authorization to turn an endpoint result into an identity or privacy claim.
The public authenticated `GET /v1/proxy/conformance` advertises the configured
assertion mappings and their need for exact pinned-registry validation, expiry,
tokenizer digest and ranking basis. Its response is `private, no-store`; private
file paths and keys are omitted.

Ordinary, already-authorized inference supplies the gateway observations. Capture
begins before Execute, accepts only validated normalized output and commits only
after independent final-result/receipt verification and paid closure. Telemetry
failure never changes the purchase or authorizes retry. Replay and recovery do not
produce samples. The existing operator probe controller also returns a sealed,
non-deserializable conformance completion. An operator can retain it with
`Recorder::retain_probe` for an exact matching adapter/connection subject, but it
is labeled `provider_self_test` and cannot acquire independent assurance. Setup
drafts do not automatically become buyer-authoritative evidence, and this phase
does not add provider-evidence broadcast or public evidence import.

Signed records bind the canonical network, tester, source, process boot,
configuration digest, suite, exact offer digest/membership revision, endpoint
contract, recipe, connection revision/digest, original session/probe, request and
result digests, request class, observation time and expiry. The class includes
serialized input-size bucket, thinking/streaming controls, tool/schema/sampling
shape and explicit output controls. Prompt, upstream response, URL and credentials
are absent from stored public facts. A successful tiny request does not establish
support for a larger envelope. Registry predicates use both definition and
profile freshness/assurance constraints. Unsupported or absent observations remain
unknown; `verified`, operator verification, data handling and unimplemented
taxonomy predicates remain unavailable.

`preferred_speed` is supported for streaming LLM requests with comparable local
observations. It uses exact integer token/interval ratios, never rounded floats,
billable units, event counts, reported usage, or provider heartbeat speed. The
existing local tokenizer counts validated visible output once after completion;
the first output chunk is excluded from the generation interval. Samples retain
TTFT and total duration separately. Short samples, one timestamp, saturated
tokenizer workers, output bounds, consumer backpressure, or overlapping observed
local requests cannot establish ranking speed. Different request classes,
tokenizers or observed concurrency are incomparable. Upstream cache and load stay
explicitly unknown: this is a comparison of observed client conditions, not equal
known remote conditions or a universal performance promise. Decisions and
non-streaming requests do not silently substitute a latency preference.

The resolver preserves complete indexed scope traversal, explicit hard limits,
retail maximum caps and continuity. Missing/stale/incomparable evidence produces
`incomplete`, never a fastest claim over only the known subset. Successful speed
selection uses `ranking_basis: locally_tokenized_generation_rate` and
`ranking_claim: highest_observed_comparable_rate`. Equal speed falls back to the
existing maximum-cost and stable model tie break. Existing presence eligibility
and native throughput-floor policy still apply independently. Quotes expire no
later than their evidence; fresh admission revalidates evidence and checks the
actual proposed connection before any owner hold/signing callback. Already
retained purchase replay continues under its immutable original terms.

Storage is a separate protected redb database with an exact subject/class index,
bounded records and bytes, and an expiry index. Each write prunes at most 32
expired entries; reads never scan the catalog, ledger or history. Four bounded
storage/validation operations may run concurrently. Restart retains records but
requires a fresh observation before reuse. Monotonic elapsed time prevents a
backward wall-clock change from extending a running record. Exhausted local
telemetry storage leaves evidence unavailable and never changes catalog contents
or monetary authorization.

Local checks, from the candidate Core checkout with its assigned target directory:

```sh
cargo test -p mayhem-proxy --lib conformance::tests
cargo test -p mayhem-gateway --lib proxy_buyer::tests::conformance
```

The connected fixture uses real buyer/provider controllers, decoder workers,
durable gateway stores and a local signed canonical fixture. Its SC-Bridge is a
bounded protocol double; these checks do not claim real Noise relay, live
provider, production credentials or production deployment acceptance.
