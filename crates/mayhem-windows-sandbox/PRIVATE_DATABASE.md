# Protected Windows database handles

`PrivateDatabaseFile` opens a local NTFS regular file relative to retained,
validated ancestor handles. The parent must have a private owner/DACL; reparse
points, remotely mapped paths, multiple hard links and reserved lock names are
rejected. Existing-only recovery refuses missing and empty files. Creation uses
open-or-create without truncation, and validates the exact opened handle.

The data handle allows no read, write or delete sharing. This remains enforced
until that handle closes, independently of an advisory lock implementation or
the database library's unsupported-lock fallback. Ancestors remain pinned until
the data handle drops. There is no raw writable handle, clone, path-reopen,
permission repair or automatic corruption reset in the public API.

The bounded operation is the requested IO buffer. Reads and writes use explicit
offsets, serialize against length/flush operations, reject overflow and terminate
on a zero-byte short read/write. Disk operations remain synchronous; no hard
disk-time deadline is claimed. Creation requests write-through and a full flush;
database sync requests a full file flush. Existing redb immediate-commit and
recovery rules still own transaction correctness. This layer does not add its
own journal or infer authorization from file existence.

`mayhem-proxy/src/storage.rs` connects this handle to redb's `StorageBackend` on
Windows. Attempts, capacity, presence, conformance, negotiation and buyer recovery
use that adapter. Unix still uses the original protected file constructor and
redb `create_file` path. The change adds no filesystem work to token delivery,
no receipt/history traversal, and no changes to balances, receipt schemas or
payment-policy decisions.

Three native NTFS fixtures cover exclusive access and ancestor retention,
reopen/missing/empty/hard-link rejection, and concurrent positioned IO with
zero-fill, EOF and offset/length bounds. These fixtures currently cross-compile;
they have **not run on Windows**. The exact redb adapter source also cross-checks
against the pinned redb dependency. Existing local Unix storage/accounting tests
pass. Full native Windows database/financial recovery, actual OS enforcement and
performance acceptance remain required before release. This document does not
claim power-loss testing or enable production serving.
