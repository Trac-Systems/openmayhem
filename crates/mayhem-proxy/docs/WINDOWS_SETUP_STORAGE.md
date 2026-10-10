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
No permission repair, public ACL, network filesystem or ordinary-I/O fallback
is provided. SYSTEM/Administrators and the current user remain trusted, as in
the protected-reader boundary.

Before publication, failure leaves a private uncommitted staging tree and no
new final bundle. After an attempted rename, an unknown result is retained as
unknown: no automatic removal, promotion or replacement occurs. Staging is
bounded per attempt; abandoned trees are not automatically garbage-collected.
Recovery must inspect the exact original bundle and its typed retained IDs.

## Validation and remaining gates

The Windows sandbox and its native fixture sources cross-compile for
`x86_64-pc-windows-msvc`. An isolated harness also typechecks the exact Windows
store/bootstrap glue against that real crate; its domain types/validators are
stubs, so this is not a full proxy build. Full proxy Windows cross-compilation
in the development environment stops in `aws-lc-sys` because the Windows SDK
header `windows.h` is unavailable. Native Windows execution is unrun and
release-blocking, including ACL/sharing/durability, tokenizer containment,
database locking/restart and actual managed Run acceptance.

`tests/setup_windows.rs` contains two opt-in actual factory/store fixtures:
restart/CAS/partial-slot recovery, locking, protected reference validation,
existing-bundle immutability, and no publication of invalid input. They perform
no network/payment work and have not been built or run on Windows here. On a
native Windows host with the required toolchain and a dedicated already-private
NTFS fixture parent:

```powershell
$env:MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT = 'C:\private-fixtures'
cargo test -p mayhem-windows-sandbox private_files::
cargo test -p mayhem-proxy --test setup_windows -- --ignored --test-threads=1
```

The fixture creates and removes only a random child under that supplied parent.
The existing Unix six-case setup store suite and generated bootstrap acceptance
remain separate regression evidence. A connected bootstrap case returned Busy
in the parallel subset and passed when run alone; no production workaround was
introduced.

The exclusive redb adapter is already integrated separately; native tests must
still prove competing-open refusal, process-death recovery and original durable
state reopening. Pre-save credential-backed `/models` preview still has a
Windows refusal in `bootstrap::models_connection`; it requires a separate
protected temporary-credential lifecycle. Native Windows mayhemd/Run and fresh
install/restart proof remain outstanding. This storage slice does not claim
complete Windows onboarding.
