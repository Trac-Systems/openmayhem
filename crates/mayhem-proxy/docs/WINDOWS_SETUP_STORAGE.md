# Windows guided-setup storage integration

Windows now uses the same setup records, revisions, validation and generated
configuration as Unix. `setup/store/windows.rs` holds an owned protected NTFS
directory and nonblocking namespace lock. Updates retain bounded JSON, check
the original, recover only a distinct validated unpublished temporary, then
write/flush/replace using the NTFS API. Ambiguous mutation returns
`CommitUnknown`; the operation never retries or promotes temporary contents.

`setup/bootstrap/windows.rs` generates the existing Flow/Connection/Probe/Run
bundle, stages a private tree, runs the existing cross-component validators,
and publishes it with one no-replace directory operation. It does not create
capacity state, dispatch a probe, collect money, publish an offer or start a
provider. The probe path's Windows directory check uses the same protected
validation boundary. Unix filesystem algorithms and financial policy are
unchanged.

The destination parent must already be a protected local NTFS directory owned
by the current user. The final bundle name is lowercase ASCII letters/digits,
underscore or dash, at most 64 bytes. The NTFS tree is bounded to 128 entries,
depth 8 and 64 MiB aggregate file bytes, including tokenizer and configuration.
The existing tokenizer pin and isolated validation are required for LLMs.
Installation validation explicitly waits for its temporary tokenizer to be reaped
before publishing the staged bundle. Only the fixed private image-recovery record
may remain in the worker directory. Normal retained tokenizer operation is unchanged;
this wait is not added to inference requests or token accounting.
No permission repair, public ACL, network filesystem or ordinary-I/O fallback
is provided. SYSTEM/Administrators and the current user remain trusted, as in
the protected-reader boundary.

Pinned ancestor directories allow read/write sharing but never delete sharing.
Sharing does not grant a caller write access: owner/ACL and no-reparse checks
still apply. This permits a trusted process to publish a sibling configuration
while an existing worker/database remains open. Private read-only leaves still
deny writers, database handles remain exclusive, and pinned paths cannot be
renamed or replaced. Native checks cover both successful sibling publication
and refusal of those forbidden operations.

Before publication, failure leaves a private uncommitted staging tree and no
new final bundle. After an attempted rename, an unknown result is retained as
unknown: no automatic removal, promotion or replacement occurs. Staging is
bounded per attempt; abandoned trees are not automatically garbage-collected.
Recovery must inspect the exact original bundle and its typed retained IDs.

## Validation and remaining gates

Native Windows 11 x86_64 build 26300 now has passing sandbox, protected storage,
decoder/tokenizer, setup and conformance-store checks. The full CPU CLI also
builds natively with MSVC; this supersedes the earlier cross-compilation-only
evidence and missing local Windows SDK limitation. These proofs do not establish
other Windows architectures or complete fresh-install/managed Run acceptance.

`tests/setup_windows.rs` contains opt-in actual factory/store/discovery fixtures:
restart/CAS/partial-slot recovery, locking, protected reference validation,
existing-bundle immutability, and no publication of invalid input. They perform
no payment work. Discovery uses only an isolated loopback HTTP fixture to check
the authenticated GET and credential cleanup. All three pass on native Windows
11 x86_64 build 26300, alongside the 25 protected-storage cases. On a native
Windows host with the required toolchain and a dedicated already-private NTFS
fixture parent:

```powershell
$env:MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT = 'C:\private-fixtures'
cargo test -p mayhem-windows-sandbox private_files::
cargo test -p mayhem-proxy --test setup_windows -- --ignored --test-threads=1
cargo test -p mayhem-proxy --lib runtime_directories_create_reopen -- --ignored --test-threads=1
```

The fixture creates and removes only a random child under that supplied parent.
The existing Unix six-case setup store suite and generated bootstrap acceptance
remain separate regression evidence. A connected bootstrap case returned Busy
in the parallel subset and passed when run alone; no production workaround was
introduced.

The native storage and setup tests exercise competing-open refusal, process
lock release, original durable reopening and restart/CAS recovery with the
exclusive redb adapter. Pre-save credential-backed `/models` preview now uses the
same protected NTFS creation/publication and exact-file cleanup operations.
The connector loads a sensitive header and cleanup must succeed before a usable
client returns; no network is dispatched during credential construction. Existing
credential files are never removed. A failed or ambiguous storage operation
can leave a private scratch file and does not trigger deletion retries.

Managed startup now creates missing runtime directories with protected NTFS
first-create publication, and validates existing directories without permission
repair, recursive parent creation or replacement. Supervised Windows wallet
passwords use the same bounded protected-file reader, retaining the existing
trailing newline handling and never placing the password in child arguments.
The managed-directory create/reopen/refusal test also passes on native Windows
11 x86_64 build 26300. The seven conformance-store tests pass using private NTFS
fixtures. Complete mayhemd/Run and fresh-install/restart proof remain separate
requirements; these component checks do not establish complete onboarding.

The Windows supervisor now implements `persistent_children_v1` using the same
protected exclusive NTFS database boundary. The first explicit persistent child
creates the private registry; ordinary startup with no registry creates none.
Restart reads the exact saved child configuration. Missing, empty, malformed,
unrelated or aliased existing storage is refused, never reset. The registry
retains its existing child/record bounds and authenticated local control API.
No existing host ACL is changed and there is no unrestricted filesystem fallback.

Opt-in supervisor and full shared setup acceptance use the same private parent:

```powershell
cargo test -p mayhemd persistent::windows::tests:: -- --ignored --test-threads=1
$env:MAYHEM_SETUP_CLI_BINARY = 'C:\candidate\mayhem.exe'
$env:MAYHEM_SETUP_DAEMON_BINARY = 'C:\candidate\mayhemd.exe'
cargo test -p mayhem-proxy --test setup run::run_cli_real_supervisor_publication_restart -- --ignored --exact --test-threads=1
```

The full setup fixture also requires its normal Node dependencies and a committed
candidate Intercom snapshot. It uses a disposable encrypted wallet and isolated
canonical ledger/bridge fixtures, not production accounts or payment rails. Its
Windows daemon shuts down through the fixture's bounded normal exit timer, then
restarts; it does not terminate any installed supervisor or provider.
