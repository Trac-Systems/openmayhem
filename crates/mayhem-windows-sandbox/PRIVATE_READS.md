# Protected Windows reads

`read_private_file(path, maximum_bytes)` provides a read-only Windows boundary
for `mayhem-proxy::connector::config::private_file`. It returns zeroizing bytes
or a refusal; it never retries through an ordinary filesystem read. Unix
behavior is unchanged. `validate_private_directory(path)` checks an existing
directory only. Its result is a snapshot, **not** a write, lock or persistence
capability. Protected setup writes and Run persistence remain separate work.

## Authority and path rules

The caller supplies a local absolute drive path and an explicit byte limit.
UNC paths, alternate data streams, device names, relative components and
ambiguous trailing dots/spaces are rejected. The optional `\\?\` drive-path
prefix does not relax those checks. Paths are bounded to 30,000 UTF-16 units,
128 components and 255 UTF-16 units per component.

The root must resolve to a local volume GUID root. Each child is opened
relative to its already-open parent using `NtOpenFile`, with reparse traversal
refused. Every ancestor handle stays open until the read completes. Handles
share reads only, withholding write/delete sharing. The final object must be a
regular disk file with exactly one hard link; reparse, offline and recall
objects are rejected. All metadata and security checks use those handles.

The final file or validated directory must belong to the current process user.
Effective allow entries may grant access only to that user, SYSTEM and the
local Administrators group. Ancestors may also belong to the exact Windows
TrustedInstaller service SID, which owns Windows Resource Protection paths.
This exception applies only to ancestors; the final private object still needs
the current user's ownership and the original private ACL. Other service SIDs,
including the All Services group, are not trusted by this exception.
Other users may have ancestor read/traverse or create-child access,
but not deletion, ownership/DACL changes, attribute/EA mutation or unknown
access rights. Unknown effective ACE forms are refused. Missing/null DACLs are
refused. The helper never repairs ACLs or takes ownership; other ancestor owners
still cause refusal. The fixed TrustedInstaller SID and its OS role are documented
in [Microsoft's Windows permissions reference](https://learn.microsoft.com/en-us/archive/msdn-magazine/2008/november/access-control-understanding-windows-file-and-registry-permissions#interpreting-security-descriptor-string_aces).

This protects against other ordinary users, not the current user, SYSTEM,
Administrators, TrustedInstaller, a malicious kernel or a malicious filesystem. It does not claim
historical secrecy if trusted software previously exposed the same bytes.

Reads are bounded by the supplied cap and performed through the same final
handle. Length, file identity, last-write time, link count and private ACL are
checked again before returning. Partial buffers are zeroized on failure. The
helper performs no network access or credential logging, and opens no file for
write. These are synchronous OS operations; a wall-clock I/O deadline is not
claimed.

## Verification and remaining gate

The library and an exact connector-call harness cross-check for
`x86_64-pc-windows-msvc`. Seven protected-read fixtures pass on native Windows 11
x86_64 (build 26300), including a TrustedInstaller-owned system root. They cover bounded private
reads, public ACLs, unsafe ancestor grants, writable existing handles, hard
links, junctions and path replacement while handles are pinned.

This establishes the protected-read boundary on that host, not complete Windows
provider acceptance. The wider storage suite has five outstanding mutation/
recovery failures; full proxy setup and contained worker execution remain open.
On an isolated native Windows test machine, the focused command is:

```powershell
cargo test -p mayhem-windows-sandbox private_files::tests
```

The fixtures contain synthetic bytes only and create their own explicitly
private temporary directory. A failure to create a junction or satisfy the
ancestor policy is a failed test, not a skipped proof.

The implementation follows the documented [sharing behavior of CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[relative object names and no-reparse handling](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes),
and [NtOpenFile's existing-object interface](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntopenfile).
