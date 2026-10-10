# Local proxy performance evidence

The opt-in `performance::local_http_durable_performance` integration test compares
direct loopback HTTP with the actual proxy executor: isolated decoder process,
request-bound validation, independently observed usage and durable request,
acceptance and result storage. It includes preparation in the proxy timer and
checks the retained result after every invocation. It neither sends real inference
nor publishes financial records. All files and services are disposable fixtures.

This is the marginal cost of that executor, **not** whole-platform latency. It
excludes buyer routing, public API/Studio/MCP transport, ledger settlement, TLS,
network distance and real model compute. Separate real-backend and composed
buyer/financial acceptance remain required. See [live acceptance](LIVE_ACCEPTANCE.md).

## Workload

- Two simultaneous requests use one executor and one journal, with two decoder
  slots. Each request retains its own exact result and observed usage.
- Streams contain 2,048 and 4,096 ordered, nonidentical content deltas, in groups of
  64 every 8ms after a 40ms fixture delay. Both direct and proxy paths receive the
  same bytes. Each size has three concurrent pairs. Content hash, order and count
  must match, including the final durable answer.
- Decisions contain 128 distinct questions with alternating answers and full
  probability maps. Eight concurrent pairs exercise 2,048 decisions per path;
  every answer must remain associated with the right question.
- The direct control is interleaved before each proxy pair. Backend counters
  verify the exact number of calls; no automatic retry masks a failed invocation.
- Reports retain every sample, preparation time, useful-output time, last-output
  time, completion time, pair skew and aggregate output throughput. Stream units
  are synthetic content events, not model tokens; decision units are questions.
- First samples are retained. Cold startup is not discarded to make a result
  pass. `debug_assertions` distinguishes unoptimized versus release evidence.

The fixture's one-MiB response bound fits the entire stream and the shared decoder
buffer allowance. Raising an advertised payload limit also changes the decoder's
worst-case resource reservation; do not silently exceed the pool budget or remove
its capacity guard just to make a benchmark run concurrently.

## Baseline before acceptance

Use new report paths. The test refuses to overwrite them. Run without unrelated
build/test load and use the same binary profile and machine for both phases.

```sh
MAYHEM_PROXY_PERF_PHASE=baseline \
MAYHEM_PROXY_PERF_REPORT=/absolute/private/baseline.json \
cargo test --release --locked -p mayhem-proxy --test execution \
  performance::local_http_durable_performance -- --exact --ignored --nocapture
```

Read the baseline, then save a numeric envelope **before** running the candidate.
The envelope has `fixture: "proxy-http-durable-perf-v1"` and a `cases` object with
`stream_2048`, `stream_4096` and `decisions_128`. Each case requires these positive,
finite numbers in milliseconds:

| Key | Meaning |
|---|---|
| `p95_first_ms` | Maximum observed proxy useful-output time (complete answer for JSON) |
| `p95_complete_ms` | Maximum observed proxy completion time, including durable result |
| `max_pair_completion_skew_ms` | Maximum difference within a concurrent pair |
| `direct_p95_complete_max_ms` | Environmental control; reject an overloaded comparison |

Record the baseline hash and the rationale in the envelope. These are acceptance
budgets for this controlled workload, never model generation timeouts or customer
limits. For small sample counts the reported nearest-rank p95 is effectively the
maximum; it is not a population-tail latency guarantee.

```sh
MAYHEM_PROXY_PERF_PHASE=compare \
MAYHEM_PROXY_PERF_ENVELOPE=/absolute/private/envelope.json \
MAYHEM_PROXY_PERF_REPORT=/absolute/private/comparison.json \
cargo test --release --locked -p mayhem-proxy --test execution \
  performance::local_http_durable_performance -- --exact --ignored --nocapture
```

The test reads budgets before dispatch, writes the report including any failures,
and then fails if a limit was exceeded. Keep failed evidence. Do not relax a
budget or remove cold samples after seeing the candidate. Investigate the stage
or build-profile difference, and rerun only when that produces new evidence.

This Unix harness does not replace Windows process-containment checks or a native
Linux measurement. There is no full-catalog/receipt scan or customer-content log
introduced by the test; production sources are unchanged.
