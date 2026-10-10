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

## Windows: implemented boundary, native acceptance outstanding

The decoder uses a separate, fixed-policy launcher in `mayhem-windows-sandbox`;
the native engine's existing configurable launcher is unchanged. Unsafe Win32
calls stay in that FFI crate, while the protocol/financial crate still forbids
unsafe code. The intended targets are 64-bit x86_64 and aarch64. Other Windows
architectures refuse initialization. LPAC, child-process restrictions and every
required mitigation must be available; unsupported systems fail before READY.
There is no ordinary Windows process-spawn fallback.

Each child receives a fresh derived AppContainer SID and no registered profile
or writable profile directory. Less Privileged AppContainer mode removes the
broad `ALL APPLICATION PACKAGES` grant. Its only named capability is a random
read/execute grant for one private staged bundled executable. The launcher
copies that image once per Pool, retains it across tokenizer/decoder launches,
and denies writes/deletion while children can use it. It accepts only local
drive paths, rejects reparse points, pins ancestors against rename/deletion,
and checks source/work-directory ownership and DACLs. The work directory must
start empty and grant no other user access. Source images may be readable by
others but cannot grant them write/delete/ownership rights. Only the current
user, SYSTEM and local Administrators are trusted for these ACL checks. An
installation with a different owner/ACL is refused rather than rewritten.

No artifact/model/workspace path, network, device or registry capability is
granted. Model output and pinned tokenizer bytes travel through bounded pipes.
Windows still provides the minimal LPAC system resources required to load/run
an executable; this is not a literal denial of every operating-system file.
The intended denial is access to ambient user/workspace data and external
effects, and must be verified on each supported Windows release. A privileged
administrator or kernel can change OS security/reporting policy; this boundary
does not protect against that authority or certify remote-provider privacy.

Creation uses an explicit three-handle inheritance list: stdin, stdout and NUL
stderr. No job, process, image-lock or caller file handle is inherited. The child
starts suspended, receives an unnamed Job with one active process, no breakaway,
kill-on-close and fixed process/job committed-memory limits, then resumes.
The trusted host ceiling is 1 GiB for ordinary decoding and 768 MiB for the
tokenizer mode (512 MiB counted heap plus 256 MiB for runtime/loader/stacks);
these are committed-memory limits, not measured RSS or the parent IPC reservation.
A failed assignment/resume kills and reaps the child.
Dynamic executable memory, Win32k calls, extension points, remote/low-integrity
image loads and child creation are restricted at creation. Before parsing any
IPC, the child checks LPAC/capability shape, stdio, Job limits and mitigations.
It never emits READY after a failed check.

The parent retains existing process/IPC permits through kill/reap and pipe
draining. Windows pipe adapters use bounded 64 KiB asynchronous file buffers;
a supervisor polls one process handle per acquired slot. No per-token, history,
financial or remote-provider operation is added, and there is no overall model
generation or retained-tokenizer lifetime timer. This does not establish a
total CPU allocation guarantee.

Native tests in the sandbox crate exercise private read/write denial, reachable
loopback denial, process creation/access denial, excluded inheritable handles,
heap/random/clock compatibility, Job memory refusal, drop/reaping and image
lifetime. `tests/windows_containment.rs` checks actual worker refusal outside
the launcher and contained schema/regex preparation through the shared Pool.
Run both on native Windows; compiling these tests is not enforcement evidence.
No Windows runtime was available for this implementation slice, so native
enforcement and actual decoder acceptance remain release-blocking. Retained v2
tokenizer initialization additionally verifies its exact 768 MiB Job limit and
all LPAC controls; legacy one-shot v1 remains refused on Windows. The separate
Windows tokenizer tests and native enforcement are not yet proved. Protected
provider setup storage now has protected NTFS integration; native end-to-end
acceptance remains separate. Native Linux/x86_64 acceptance passes 22 cases on
kernel 7.0.0 with the actual current worker: filter enforcement/refusal, streaming
decoder cancellation/capacity/backpressure, and bounded pinned tokenization.
The isolated native fixture used the original locked dependency versions and
checksums, no GPU, no network, and no production service/store access. Its
ignored filter-child entry is executed by the parent enforcement case.

Primary references: Microsoft's [AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation),
[LPAC setup](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer),
[creation attributes and mitigation masks](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute),
[Job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
and Chromium's [derived versus registered AppContainer identities](https://github.com/chromium/chromium/blob/main/sandbox/win/src/app_container_base.cc).
