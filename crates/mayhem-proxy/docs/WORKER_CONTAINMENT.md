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

Remaining release requirements include Linux containment, Windows protected
ACL/JobObject support, tokenizer containment and measured total resource limits.
Linux currently retains the original process restrictions; it must not be
advertised as filesystem/network sandboxed by this macOS change. Windows still
rejects decoder pool creation where its protection is unimplemented. Do not
declare cross-platform containment acceptance from a macOS test.

For Linux, kernel [seccomp filter documentation](https://kernel.org/doc/html/latest/userspace-api/seccomp_filter.html)
and [Landlock documentation](https://docs.kernel.org/userspace-api/landlock.html)
describe different restrictions and limitations. Any implementation must test
its actual kernel/ABI, inherited handles and permitted system calls; a process
boundary or a successful filter installation alone is insufficient evidence.
