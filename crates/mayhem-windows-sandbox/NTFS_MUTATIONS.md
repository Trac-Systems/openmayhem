# Protected NTFS mutations

The API now backs Windows setup JSON storage and first-install bundle
publication. The separate exclusive database backend is wired into proxy redb
stores; it retains the database handle for the backend lifetime. This does not
establish complete Windows onboarding, Run or mayhemd persistence acceptance.
The storage fixtures pass on native Windows 11 x86_64 (build 26300). Complete
provider acceptance and actual power-loss durability remain separate requirements.
Cross compilation alone is not native proof.

`NtfsDirectory::open_existing` retains the existing protected traversal and ACL
rules, then requires the opened local volume to report NTFS with persistent
ACLs. The final directory permits write sharing for internal namespace target
opens, while denying delete sharing; ancestors retain their existing sharing.
The read-only API is unchanged. No new trusted principals or permission repair
are introduced.

`try_lock()` or ownership-transferring `into_lock()` uses one fixed private
`.mayhem-ntfs.lock` per directory. Its
nonblocking exclusive byte lock lives with `NtfsGuard`, which all readers and
writers of this namespace must use. The lock file remains on disk; it is never
unlinked/replaced on release. Unsupported locking is an error, and no named
mutex or unlocked fallback exists. OS calls are synchronous; only lock
acquisition is explicitly nonblocking, not arbitrary disk I/O.

`LeafName` accepts only canonical lowercase ASCII letters, digits, dot, dash and
underscore, within 255 bytes and the existing device/traversal checks. This
avoids case aliases for reserved lock/temporary names. No paths or alternate
streams can enter child operations. Each read/write has a caller cap, with a
64 MiB resource ceiling matching the existing maximum tokenizer artifact.

## Commit ordering

1. `guard.prepare(temporary, bytes, cap)` creates a new private regular file
   using handle-relative `NtCreateFile`, an explicit protected DACL and
   `FILE_WRITE_THROUGH`. Existing files are never truncated. It writes bounded
   bytes and calls `NtFlushBuffersFileEx` with flags zero.
2. `PendingFile::publish(destination, mode)` validates the source and any
   existing target, then latches the exact intended destination before invoking
   one same-volume `FileRenameInformation` request. `CreateNew` rejects an
   existing destination; `Replace` permits a validated regular target. There is
   no copy/delete, readonly override or multi-rename fallback.
3. A second full flush through the retained source handle includes metadata
   and the device cache. A metadata-only target handle must identify that same
   volume, file ID and byte length before success is acknowledged.

The source handle excludes other writers/deletion and stays in `PendingFile`
with its identity and locked directory. Before rename, an error leaves an
uncommitted temporary and the original target. Once rename is attempted,
errors return `CommitUnknown`; the API neither rolls back nor retries rename.
`reconcile()` only re-flushes and checks the exact original target identity.
If the target remains the old file, uncertainty remains. Dropping the pending
object closes handles only and never promotes or deletes a temporary.

Crash recovery must continue using the existing typed application records and
their original operation IDs/digests. This primitive deliberately does not
introduce a redo log that treats leftover temporary files as authorization.
`discard_uncommitted(original, temporary)` supports the existing setup store's
fixed temporary slot. It independently retains the original or proves absence,
requires a private regular single-link temporary with a distinct identity, and
deletes only that exact handle under the guard before a full parent flush.
Malformed originals fail closed. An ambiguous delete/rename poisons that guard
against later cleanup or mutation; dropping an uncertain pending object does
not erase this latch. A new operation still uses its typed original record,
never the temporary, as authority. No raw writable handle is exported.

## First-install directory publication

`guard.stage_directory(temporary, entries, maximum)` accepts an explicit tree
of `DirectoryEntry::Directory` and `DirectoryEntry::File` records. Every path is
a nonempty sequence of validated `LeafName` components; directory parents must
be declared, and duplicate paths, file parents and reserved lock names fail
before any staging mutation. Input order does not matter. The plan is bounded
to 128 entries, depth 8, and 64 MiB aggregate file bytes, or a smaller caller
maximum. An empty private directory is also a valid plan.

Creation is relative to retained parent handles, with explicit owner/private
DACL and write-through mode for every directory and file. Files are fully
flushed and closed. Directories are then fully flushed and closed from leaves
to root. Only the staged root handle and original parent guard survive into
`PendingDirectory`; no descendant handle or writable path is exposed.

`PendingDirectory::publish(destination)` has no replace option. An existing
directory, even an empty one, is a conflict. Other target types are rejected.
The implementation attempts one handle-relative no-replace directory rename,
fully flushes the retained root, and checks the target's volume/file identity.
Before the attempt, failures leave the destination unchanged. A concurrent
target creation or any other error after the attempted rename returns
`CommitUnknown`. Reconciliation only flushes and checks that exact original
root; it never merges trees, retries the rename, chooses another destination,
deletes an original, or promotes an abandoned staging tree.

`prepare_for_inspection()` retains the same root object with read-compatible
access so existing protected loaders can validate a generated bundle. It first
releases the creation handle's delete access through an identity-checked interim
handle, then denies deletion before reporting successful inspection preparation.
This respects Windows' sharing rules; it never closes the last object reference
and reopens a pathname. Before publication, the originally
declared directories are reopened relative to that root, checked against their
original identities, and fully flushed bottom-up. This covers the contained
tokenizer's temporary worker image cleanup without scanning arbitrary paths.
The root is then reopened by retained handle with write-through/delete access,
without a caller-path lookup. The setup caller rejects a nonempty worker
directory after validation.

Windows setup invokes this primitive after generating the same typed bundle as
Unix and validating its configuration, references and pinned tokenizer.
There is no reboot recovery object or new journal format: callers must use
their existing typed retained operation/digest records for restart decisions.
Normal full flush is requested on directory handles as well as files; an
unsupported or failed flush is an error, never a successful no-op.

## Documented authority

Microsoft documents handle-relative creation and create-only dispositions in
[NtCreateFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile),
and same-volume replacement/no-replacement through a directory handle in
[FILE_RENAME_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information).
The NTFS [write-through description](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
explicitly covers rename metadata. A normal
[NtFlushBuffersFileEx](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntflushbuffersfileex)
flushes data and metadata and synchronizes the underlying storage cache.
This is the basis for the implemented ordering, subject to native validation
and the normal assumption that the storage stack honors flushes.

[LockFileEx](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)
documents immediate failure on contention and OS release after process exit;
release can be delayed. The fixed lock prevents overlapping protocol clients,
not malicious writes by the already-trusted current user/SYSTEM/Administrators.

[NtOpenFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntopenfile)
reopens the retained object using an empty relative name and explicit directory/
file options. This handle-relative pattern is also used by
[Microsoft WSL's ReopenFile helper](https://github.com/microsoft/WSL/blob/master/src/windows/common/filesystem.cpp).
Native execution showed the Win32 `ReOpenFile` path returning access denied for
the retained directories. No caller-path or ordinary-I/O fallback is used.
Every transition rechecks private ACL, regular/directory identity
and, when required, write-through mode. Exact temporary deletion uses
[FILE_DISPOSITION_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information);
after setting disposition, only closing that handle is attempted.

## Focused verification

Six file/lock native fixture cases plus one explicitly ignored subprocess helper
compile for `x86_64-pc-windows-msvc`. They cover create/replace/reopen, bounded and reserved
names, injected failures before rename/after rename/after final flush, actual
sharing-induced rename failure, hard-linked/public targets, and two-process
locking with owner termination. The helper is invoked by the bounded parent
test, not counted as independent enforcement proof. Existing protected-reader
fixtures still cover junctions and ancestor ACL/path replacement.
There is no injected prepare-write or pre-rename-flush failure test in this
slice; the before-rename hook runs after preparation and its flush succeeded.

Five additional native directory cases cover full nested/empty-directory
publication and reopening, complete-plan rejection before creation, preserved
empty/nonempty/file destination conflicts, the same three fault boundaries,
and an actual no-replace rename collision introduced after the precheck.
They do not inject write or staging-directory flush failures. These native
fixtures pass on native Windows.

Four integration-adaptation fixtures cover owned-lock lifetime, fixed-slot
cleanup with original/absence, alias/nonregular/uncertain refusal, read-only
inspection followed by publication, and replaced declared-directory refusal.
They include transient worker-file creation/removal but do not simulate power
loss during its final directory flush. Actual setup fixtures and remaining
toolchain limits are documented in
[WINDOWS_SETUP_STORAGE.md](../mayhem-proxy/docs/WINDOWS_SETUP_STORAGE.md).

Run on an isolated native Windows NTFS machine:

```powershell
cargo test -p mayhem-windows-sandbox private_files::
```

All 25 storage cases pass on native Windows 11 x86_64 (build 26300), including
seven protected-read cases, three database cases, and the mutation/recovery
cases above. The ignored lock-child helper is invoked by its parent test.
This proves process-failure recovery and the exercised flush/error behavior,
not actual power-loss durability; a process kill or injected error cannot prove
that the physical storage stack honors flushes during power loss.
