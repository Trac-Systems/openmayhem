# Bundled decoder containment

HTTP, DNS, upstream credentials, financial storage and signing remain in the
trusted parent. The bundled decoder receives bounded IPC with model output and
validation rules. Provider recipes cannot select an executable or sandbox policy.
This boundary does not certify the remote provider's privacy or model identity.

On macOS the dedicated executable closes inherited descriptors above stderr,
then installs Apple's named `kSBXProfilePureComputation` profile before reading
IPC. Closing descriptors is required: the profile does not revoke access through
already-open files. Only stdin/stdout/stderr remain available to the decoder.
If descriptor cleanup or sandbox installation fails, the child exits before
its ready handshake; it never silently runs without that restriction.

The FFI is isolated in the executable. The protocol/financial library retains
`forbid(unsafe_code)`. The macOS SDK `sandbox.h` documents this named profile
and also marks `sandbox_init` deprecated. Therefore every supported macOS release
needs actual installation/decoder acceptance; do not assume future OS support or
replace a failing sandbox with an unrestricted retry. The private child does not
load extensions, discover local configuration or print sandbox diagnostics.

The security test opens a synthetic private file in the parent and deliberately
leaves its descriptor inheritable. It proves the descriptor reaches the child
and is closed during containment. It also checks denied file reads/writes,
connections to reachable TCP and Unix listeners, and child execution. Ordinary
worker tests cover valid JSON, UTF-8 streaming, schema checks, cancellations,
backpressure, crashes, retained resource permits and incremental processing.

Existing process counts, IPC byte reservations and parser response deadlines
remain in the parent. These do not constitute a total RSS or CPU allocation cap.
The generation/consumer wait remains separate from parser processing time.
No ledger, receipt-history or per-token health operation is added.

## Linux

The dedicated Linux executable supports little-endian 64-bit `x86_64` and
`aarch64`. Before consuming IPC it enumerates `/proc/self/fd`, closes every
descriptor above stderr and confirms closure, enables irreversible
`no_new_privs`, and installs a fixed seccomp allowlist with thread synchronization.
Missing/inaccessible procfs, denied filter installation, unsupported architecture,
or a kernel without `SECCOMP_RET_KILL_PROCESS` support causes refusal before READY.
Linux 4.14 or newer with seccomp filtering enabled is required; a surrounding
container policy can still make initialization unavailable. No unrestricted retry
or provider-selected filter is available.

The filter checks the syscall audit architecture and rejects x32 on x86_64.
It allows input only on stdin, output only on stdout/stderr, limited stdio
descriptor metadata, private anonymous non-executable allocation, private futexes,
process-local runtime bookkeeping, clocks, randomness and exit. Open/path access,
socket or IPC creation/descriptor transfer, process creation/execution/control,
executable mappings and all unlisted syscalls are denied. New syscall interfaces
are denied by default. No worker thread may be created after entry. Any fixed
mode-specific resource limits must be applied by trusted startup before entry;
the contained process cannot increase its own limits. All provider-controlled
data parsing remains after entry.

Rust's HashMap seed path uses `getrandom(GRND_INSECURE)` (with a NONBLOCK fallback
on older kernels); cryptographic `getrandom(0)` remains available. The filter
allows these three specific forms, never `/dev/urandom` file fallback. On an old
kernel before its entropy pool is ready, failure remains failure. Private heap
allocation and clocks do not imply bounded total RSS or CPU, and this filter is
not an information-flow or remote-provider privacy guarantee. The decoder's
existing bounded parser deadlines and parent kill/reap behavior remain separate
from generation waiting time; no overall generation timer is added.

The binary unit tests exercise the actual installed filter in an isolated child,
with deliberately inherited synthetic file/socket handles and reachable loopback
listeners. They verify denied external actions, allowed allocation/hash seeding/
clocks and ABI/argument rules. `tests/linux_containment.rs` adds an outer filter
that prevents installation: both worker modes must exit with no READY while stdin
is still open. Existing `tests/worker.rs` exercises the actual production binary's
JSON, UTF-8 streams, semantic/schema configuration, cancellation and accounting.
Run these checks on each supported native Linux architecture; success under an
emulator does not establish guest seccomp enforcement.

Primary references: Linux [seccomp filtering](https://docs.kernel.org/userspace-api/seccomp_filter.html),
[no_new_privs](https://docs.kernel.org/userspace-api/no_new_privs.html),
[seccomp ABI and architecture caveats](https://man7.org/linux/man-pages/man2/seccomp.2.html),
and [Rust's Linux random implementation](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/random/linux.rs).

Windows protected ACL/JobObject support and measured total decoder resource
limits remain separate release requirements. Windows still rejects decoder pool
creation where protection is unimplemented. Do not declare cross-platform
containment acceptance from a single operating-system or architecture test.
