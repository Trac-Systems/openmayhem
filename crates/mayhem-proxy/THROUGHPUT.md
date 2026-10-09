# Proxy throughput observations

Status: opt-in local execution, operator probes and managed-session construction.
This is not a production activation or complete public admission integration.

The throughput floor uses locally counted visible output with an approved tokenizer.
Upstream usage counters, billable units and SSE event counts remain independent. The
tokenizer is tied to the protected connection fingerprint and adapter recipe digest;
changing either requires a matching source. Decision endpoints reject this source:
one short label is not useful evidence of an LLM generation rate.

## Data and counting

Trusted startup supplies local tokenizer JSON bytes, their raw BLAKE3 digest and explicit
limits to `health::native::Source::from_bytes`. This interface neither loads a path nor
fetches a URL. The Rust dependency is exactly `tokenizers` 0.22.2, with default features
disabled and `fancy-regex` enabled. There is no Python execution, remote tokenizer code
or Hugging Face Hub download in this path. Source review:
[dependency features](https://github.com/huggingface/tokenizers/blob/v0.22.2/tokenizers/Cargo.toml),
[encoding and offsets](https://github.com/huggingface/tokenizers/blob/v0.22.2/tokenizers/src/tokenizer/mod.rs).

Padding, truncation and stochastic BPE dropout are rejected. BPE and Unigram text
caches are disabled. The artifact is loaded once and shared, with an explicit bounded
number of measurement permits. A permit is acquired before retaining any output. If
none is free, inference proceeds with unknown speed; there is no waiting tokenizer queue.

Each visible text/reasoning/refusal/tool-argument channel accumulates only append-only
normalized deltas in bounded memory. Equal chat reasoning aliases count once. Function
names, call IDs, finish markers and usage metadata do not add generation tokens. After
the complete response passes endpoint/schema validation, each channel is encoded once,
outside the async executor. No growing-prefix retokenization occurs. Counts exclude
special tokens and tokens overlapping the first observed byte boundary, avoiding BPE
boundary overcounting. A cancelled waiter does not release its permit until the actual
blocking encode exits.

Configured bounds are validated: artifact up to 64 MiB, combined output up to 4 MiB,
up to 1,024 channels and 16 concurrent measurements per source. These are maximum valid
configuration values, not recommended operating defaults. Exceeding an output/channel
bound disqualifies measurement without truncating the actual delivered result. No prompt,
token IDs, text or tool arguments are persisted in health records. Final measurements
contain timing, interval-token count, tokenizer digest and separately labelled upstream
usage. No receipt/history scan, financial lookup or database write runs per token.

## Timing and local backpressure

An observed stream with a measurement permit uses a separate network reader. Its queue
holds at most eight 64 KiB pieces, plus one pending piece; copies prevent small queue
entries from retaining large backing buffers. The existing HTTP response-size limit
still bounds the transport's current body buffer. Dropping the request reader aborts
its owned task and releases the transport. Unmeasured streams retain direct reading.

Timing is captured before decoder IPC, journal synchronization and consumer delivery.
Pieces split from one HTTP chunk retain its timestamp. Already-buffered HTTP frames
polled without waiting retain the same observation timestamp, as do multiple SSE events
decoded from that read. A full read queue or pending consumer delivery invalidates the
speed sample; it never cancels or truncates the inference. A local reader failure is
not evidence of an upstream fault, and does not release an uncertain execution lease.

Rate is interval tokens divided by the time between first and last visible output.
There must be distinct observation times and the configured minimum number of interval
tokens (at least two). There is no minimum duration that rejects genuinely fast output.
Non-streaming replies, one buffered output batch, short labels, failed schema validation
and incomplete streams cannot certify speed. A native measurement is published only
after full validation. Its original network age survives tokenization and durable local
publication; delayed results cannot overwrite newer observations or renew stale evidence.

## What this proves and what remains

This measures observable output delivery using the approved tokenizer. It does not
prove the upstream's weights, internal token IDs, hidden reasoning, GPU speed or unused
physical sessions. Network buffering and invisible processing still limit attribution.
Tokenizer/model matching is an operator claim unless separately verified; T4 identity
does not turn it into cryptographic model attestation. Native price and billing-unit
rules are unchanged.

`Monitor::register_measured` makes the approved tokenizer and policy floor a route's
immutable admission requirement. Its public snapshot and bound live capacity source
then share the same qualification: missing/wrong-tokenizer or stale measurements yield
Checking with zero allowance, and below-floor measurements yield Degraded with zero
allowance. Fresh short replies do not refresh the old speed sample. Source freshness
includes the older of health and required speed evidence. Registration cannot silently
downgrade this policy; executor and probe setup reject conflicting tokenizer data.

Recovery uses this same eligibility even if ordinary HTTP/inference health remains
fresh. A valid but short/buffered probe can complete without establishing capacity: its
attempt budget remains spent and it backs off. Timer expiry alone never opens capacity.
Backoff starts at result evaluation, independently of the original network evidence age,
so durable publication time cannot consume the recovery delay. No probe bypasses physical
ceilings, uncertain requests, scope ownership or the operator's cost/attempt budget.

Observation-only `register` remains available for lower-level tests and decision routes;
it does not establish measured LLM admission. `Snapshot::meets_native_floor` additionally
checks a stricter buyer requirement without changing the shared operator floor. Durable
admission separately enforces shared ceilings and outstanding work. Managed startup must
select the required policy explicitly. Tokenizer provisioning, signed public availability,
gateway default/explicit floor handling and scheduled recovery integration remain required
before public activation. Unknown speed is never a measured passing rate or zero.

Local tests cover pinned data, byte offsets across BPE boundaries, UTF-8, multiple
channels, bounded accumulation, timing/age, slow consumers, real HTTP streaming for
chat/completions/Responses, operator probes and signed settlement fixtures. These are
not live-model benchmarks. Real upstream acceptance and tokenizer CPU/memory containment
under hostile inputs remain release requirements; bounded input and worker counts alone
do not establish a killable CPU limit for a blocking tokenizer.
