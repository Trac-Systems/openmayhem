# Pinned proxy tokenizer data

Proxy LLM speed observations use the operator-approved local tokenizer JSON and
its existing BLAKE3 digest. They do not identify remote weights and do not change
normalized billable input/output units. Upstream reported token counts remain
telemetry. Decisions do not acquire a tokenizer or token-speed requirement.

The shared setup factory reads only the protected approved file, checks its exact
pin, validates it through the explicitly configured bundled worker, and imports a
private copy before atomically saving the original bundle. There is no model-name
guess, URL downloader, executable tokenizer plugin, Hub access, remote code, or
automatic replacement of an old pin. An approved tokenizer asset is still an
explicit operator prerequisite; this does not provide a universal tokenizer for
unknown custom model identities.

The provider host retains only bounded bytes, connection/recipe binding and stream
capture metadata. Tokenizer JSON parsing, regex/normalizer execution and encoding
run in a retained `mayhem-proxy-worker --tokenizer-stdio-v2` child. A bounded
source-owned actor warms the exact artifact at startup and reuses its parsed data;
subsequent samples transfer only their bounded fields and nonce. Both decoder and
tokenizer modes install the same OS containment before reading IPC; tokenizer mode first sets its fixed local
resource limits. Tokenizer parallelism is disabled. IPC carries bytes, exact pin,
ABI/release and unpredictable invocation nonce; it carries no paths, upstream
credentials, wallets, receiver addresses or billing authorizations.

Limits are separate from inference/financial capacity:

- Approved artifact: at most 64 MiB; captured output: at most 4 MiB per sample.
- At most 1,024 captured channels and 16 workers per configured source. Managed
  startup also retains its existing aggregate tokenizer bytes/workers limits.
- No unbounded waiting queue: the source mailbox and active tasks are each bounded
  by configured workers, and admission fails immediately when full. Capture takes
  its source permit before retaining output. Successful replies release capture
  permits; failed/cancelled child jobs retain them through reaping. Idle children
  retain their separate process permits until shutdown.
- Artifact initialization and each count use the configured local worker deadline,
  capped at 10 seconds. On expiry the parent requests termination and awaits OS reaping while retaining all permits. Reaping
  is additional time; the entire caller future is not claimed to finish in exactly
  10 seconds. A reused worker has no cumulative CPU lifetime limit; a single count
  cannot evade the parent deadline by keeping the child alive. These are tokenizer
  deadlines, never generation or remote-job deadlines. The one-shot offline-check
  mode additionally uses a fixed 5-second child CPU ceiling on Unix; Windows
  refuses that legacy mode and only exposes the retained v2 launcher.
- Tokenizer-mode Rust allocations: 512 MiB, including transient reallocations;
  Linux also has a 768 MiB address-space ceiling. The allocator ceiling is not an
  assertion that whole-process RSS has the same value. OS/runtime overhead is
  separate. Before IPC, decoder mode irreversibly disables allocation accounting
  (one relaxed mode read remains per allocation); it has no new whole-generation
  CPU deadline or heap cap. Tokenizer mode cannot select that bypass.
- At most 128 source actors exist process-wide, with one runtime thread each.
  Managed startup retains its tighter aggregate configured tokenizer worker bound.
  Source drop shuts down idle workers and retains the actor until its children
  have been reaped. There is no global cache, disk cache or unbounded retention.

Managed startup validates before capacity configuration and serving. Conformance
enablement receives the buyer controller's explicit trusted worker launcher.
Passive serving and already-authorized recovery probes attach the same source
identity. Missing/malformed data, sandbox failure, pressure, timeout, excessive
expansion, invalid offsets or process failure produce no native-speed observation;
there is no in-process fallback and no fabricated zero/positive speed. Existing
readiness/freshness policy decides whether available evidence is sufficient.

Healthy warmed counts still run at verified completion, so only count/IPC time
remains on that path. A source warms one child at startup; additional configured
parallel slots or cold recreation after failure require loading the same pin;
a request cannot substitute an unknown or changed artifact. The initial reparse-per-response
prototype was rejected after an actual 7 MB public BPE fixture showed about one
second of debug-build artifact loading per response. In the retained child, the
same fixture counted 701 visible tokens in a median 14.2 ms across 48 jobs. Twelve
larger 11,201-token jobs took 197–227 ms with observed RSS around 150–167 MiB.
After the one-way allocation-accounting bypass, the existing ordinary decoder
fixture measured 5.4 ms median startup/handshake and 0.75 ms short JSON
decode/reap across twelve local processes. These are local macOS debug observations,
not a model identity, capacity certification or cross-load latency guarantee. Native Windows enforcement remains unverified and release-blocking;
this change introduces no nonisolated Windows fallback.

Counting semantics stay fixed: one encode per completed visible output field,
without padding, truncation, stochastic BPE, prefix retokenization or native cache
growth. Tokens spanning the first observed boundary are excluded. The original
stream timestamps define the rate; parser duration is not attributed to the model.
Buffered output, backpressure, short output and unverifiable results remain unknown.

## Windows retained tokenizer boundary

The existing trusted LPAC launcher starts only `--tokenizer-stdio-v2`, suspended
until its single-process, kill-on-close Job has exact 768 MiB process and Job
committed-memory limits. Before reading IPC or parsing an artifact, tokenizer
initialization independently verifies that exact ceiling and the full existing
LPAC capability, stdio, no-child-process and mitigation policy. The ordinary
decoder's 1 GiB Job cannot satisfy this tokenizer check. The tokenizer's 512 MiB
counted Rust heap remains a separate, stricter allocation limit installed first.
The decoder verifier and all Unix resource limits are unchanged.

The retained Windows worker has no cumulative CPU lifetime cap. Each artifact
load/count retains the existing parent deadline, Job termination and OS-reap
ownership; errors cannot fall back to an unrestricted parser. Legacy
`--tokenizer-stdio-v1` is explicitly refused on Windows because its separate
one-shot CPU guarantee is not implemented by that launcher.

The candidate's `windows_tokenizer` (four cases) and `windows_containment`
(two cases) pass with the actual bundled executable on Windows 11 x86_64 build
26300. This covers retained UTF-8 counts, exact pin rejection, source provisioning,
heap-limit recovery, unrestricted/legacy refusal and process-slot release.
Other Windows architectures/builds and complete installation/managed Run remain
separate acceptance requirements. This does not approve any tokenizer identity.
See [the containment boundary](WORKER_CONTAINMENT.md) for native security proof.

Focused local checks (build the actual worker before the internal semantic cases):

```sh
cargo build -p mayhem-proxy --bin mayhem-proxy-worker
cargo test -p mayhem-proxy --test tokenizer
cargo test -p mayhem-proxy --lib health::native::tests
cargo test -p mayhem-proxy --lib tokenizer_deadline_and_cancel
cargo test -p mayhem-proxy --test execution native_
cargo test -p mayhem-proxy --test setup bootstrap
```

These are local synthetic-data checks, not permission to obtain external assets,
run paid probes, alter native serving, publish offers, or change a production pin.
