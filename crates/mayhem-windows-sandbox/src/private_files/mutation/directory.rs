//! First-install only: a complete bounded private tree, never a merge/replace.
use super::*;
use std::collections::BTreeMap;
#[cfg(test)]
mod tests;

const MAX_ENTRIES: usize = 128;
const MAX_DEPTH: usize = 8;

/// Explicit relative components. Directories, including empty ones, must be
/// declared before their children can be accepted (input order is immaterial).
pub enum DirectoryEntry<'a> {
    Directory {
        path: &'a [LeafName],
    },
    File {
        path: &'a [LeafName],
        bytes: &'a [u8],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryIdentity {
    pub volume_serial: u32,
    pub file_id: u64,
}
fn identity(file: &File, owner: &[u8]) -> Outcome<DirectoryIdentity> {
    let v = information(file, true).map_err(|_| MutationError::Protection)?;
    acl::validate(file, owner, true).map_err(|_| MutationError::Protection)?;
    Ok(DirectoryIdentity {
        volume_serial: v.dwVolumeSerialNumber,
        file_id: (u64::from(v.nFileIndexHigh) << 32) | u64::from(v.nFileIndexLow),
    })
}

type Plan<'a> = BTreeMap<Vec<String>, Option<&'a [u8]>>;
fn plan<'a>(entries: &[DirectoryEntry<'a>], maximum: usize) -> Outcome<Plan<'a>> {
    if entries.len() > MAX_ENTRIES || maximum > MAX_BYTES {
        return Err(MutationError::Invalid);
    }
    let mut total = 0usize;
    let mut result = BTreeMap::new();
    for entry in entries {
        let (path, bytes) = match entry {
            DirectoryEntry::Directory { path } => (*path, None),
            DirectoryEntry::File { path, bytes } => (*path, Some(*bytes)),
        };
        if path.is_empty()
            || path.len() > MAX_DEPTH
            || path.iter().any(|v| v.as_str() == ".mayhem-ntfs.lock")
        {
            return Err(MutationError::Invalid);
        }
        total = total
            .checked_add(bytes.map_or(0, <[u8]>::len))
            .ok_or(MutationError::Invalid)?;
        if total > maximum {
            return Err(MutationError::Invalid);
        }
        let path = path.iter().map(|v| v.0.clone()).collect::<Vec<_>>();
        if result.insert(path, bytes).is_some() {
            return Err(MutationError::Invalid);
        }
    }
    for path in result.keys() {
        if path.len() > 1 && !matches!(result.get(&path[..path.len() - 1]), Some(None)) {
            return Err(MutationError::Invalid);
        }
    }
    Ok(result)
}

impl<'d> NtfsGuard<'d> {
    /// Validates the entire plan before creation. At most 128 entries, depth 8,
    /// and 64 MiB aggregate file bytes (or a smaller explicit caller maximum).
    /// Failure leaves a private uncommitted tree; no automatic deletion occurs.
    pub fn stage_directory<'g>(
        &'g mut self,
        temporary: LeafName,
        entries: &[DirectoryEntry<'_>],
        maximum: usize,
    ) -> Outcome<PendingDirectory<'g, 'd>> {
        if self.uncertain {
            return Err(MutationError::CommitUnknown);
        }
        if temporary == self.name {
            return Err(MutationError::Invalid);
        }
        let plan = plan(entries, maximum)?;
        let pinned = &self.directory.pinned;
        let root = native::create_directory(pinned.file(), &pinned.owner, &temporary)?;
        let original = identity(&root, &pinned.owner)?;
        let mut directories = BTreeMap::<Vec<String>, File>::new();
        let mut inspected_directories = Vec::new();
        directories.insert(Vec::new(), root);
        // Lexicographic order places every proper prefix before its children.
        // All handles are derived from this newly-created root, never paths.
        for (path, bytes) in &plan {
            let parent = directories
                .get(&path[..path.len() - 1])
                .ok_or(MutationError::Invalid)?;
            let leaf = LeafName::new(path.last().ok_or(MutationError::Invalid)?)?;
            if let Some(bytes) = bytes {
                let mut file = native::create_staged_file(parent, &pinned.owner, &leaf)?;
                file.write_all(bytes).map_err(|_| MutationError::Storage)?;
                native::flush(&file)?;
                if record_identity(&file)?.bytes != bytes.len() as u64 {
                    return Err(MutationError::Storage);
                }
                // All descendant file handles close before publication.
            } else {
                let directory = native::create_directory(parent, &pinned.owner, &leaf)?;
                inspected_directories.push((path.clone(), identity(&directory, &pinned.owner)?));
                directories.insert(path.clone(), directory);
            }
        }
        // Reverse prefix order flushes/closes every descendant before its parent.
        while directories.len() > 1 {
            let (_, directory) = directories.pop_last().ok_or(MutationError::Storage)?;
            native::flush(&directory)?;
        }
        let root = directories
            .remove(&Vec::new())
            .ok_or(MutationError::Storage)?;
        native::flush(&root)?;
        if identity(&root, &pinned.owner)? != original {
            return Err(MutationError::Protection);
        }
        Ok(PendingDirectory {
            guard: self,
            root,
            temporary,
            original,
            attempted: None,
            committed: false,
            readable: false,
            inspected_directories,
        })
    }
}

/// No descendant handles or writable paths escape. Dropping closes handles;
/// neither uncommitted trees nor uncertain originals are removed or promoted.
pub struct PendingDirectory<'g, 'd> {
    guard: &'g mut NtfsGuard<'d>,
    root: File,
    temporary: LeafName,
    original: DirectoryIdentity,
    attempted: Option<LeafName>,
    committed: bool,
    readable: bool,
    inspected_directories: Vec<(Vec<String>, DirectoryIdentity)>,
}
impl PendingDirectory<'_, '_> {
    /// Permit ordinary protected loaders to inspect the complete staged tree.
    /// Keeps the exact root object open; publication upgrades that same object,
    /// never reopens a caller path. No file mutation API becomes available.
    pub fn prepare_for_inspection(&mut self) -> Outcome<()> {
        if self.attempted.is_some() {
            return Err(MutationError::CommitUnknown);
        }
        let owner = &self.guard.directory.pinned.owner;
        if !self.readable {
            // The creation handle has DELETE access. A read-compatible handle
            // must initially share that access, then replace it before another
            // reopen can deny deletion. Each step retains the same object.
            self.root = recovery::reopen(
                &self.root,
                owner,
                true,
                FILE_TRAVERSE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )?;
            self.readable = true;
        }
        self.root = recovery::reopen(
            &self.root,
            owner,
            true,
            FILE_TRAVERSE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            false,
        )?;
        Ok(())
    }
    pub fn identity(&self) -> DirectoryIdentity {
        self.original
    }
    pub fn temporary(&self) -> &LeafName {
        &self.temporary
    }
    pub fn attempted_destination(&self) -> Option<&LeafName> {
        self.attempted.as_ref()
    }
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// One no-replace rename. A preexisting directory, even empty, is a conflict.
    pub fn publish(&mut self, destination: LeafName) -> Outcome<DirectoryIdentity> {
        self.publish_inner(destination, |_| Ok(()))
    }
    fn publish_inner(
        &mut self,
        destination: LeafName,
        mut checkpoint: impl FnMut(Stage) -> Outcome<()>,
    ) -> Outcome<DirectoryIdentity> {
        if self.attempted.is_some() {
            return Err(MutationError::CommitUnknown);
        }
        if destination == self.temporary || destination == self.guard.name {
            return Err(MutationError::Invalid);
        }
        let pinned = &self.guard.directory.pinned;
        if identity(&self.root, &pinned.owner)? != self.original {
            return Err(MutationError::Protection);
        }
        if native::inspect_directory(pinned.file(), &pinned.owner, &destination)?.is_some() {
            return Err(MutationError::Conflict);
        }
        checkpoint(Stage::BeforeRename)?;
        if self.readable {
            // Inspection can run a contained tokenizer which creates/removes its
            // temporary image in an originally empty worker directory. Re-flush
            // every declared directory bottom-up, checking its original identity;
            // never discover or authorize arbitrary added paths by scanning.
            self.flush_inspected_directories()?;
            self.root = recovery::reopen(
                &self.root,
                &pinned.owner,
                true,
                FILE_TRAVERSE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )?;
            self.root = recovery::reopen(
                &self.root,
                &pinned.owner,
                true,
                FILE_TRAVERSE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                true,
            )?;
            self.readable = false;
        }
        self.attempted = Some(destination.clone());
        self.guard.uncertain = true;
        native::rename(
            &self.root,
            pinned.file(),
            &destination,
            PublishMode::CreateNew,
        )
        .map_err(|_| MutationError::CommitUnknown)?;
        checkpoint(Stage::AfterRename).map_err(|_| MutationError::CommitUnknown)?;
        self.finish()?;
        checkpoint(Stage::AfterFlush).map_err(|_| MutationError::CommitUnknown)?;
        self.committed = true;
        self.guard.uncertain = false;
        Ok(self.original)
    }
    fn flush_inspected_directories(&self) -> Outcome<()> {
        let owner = &self.guard.directory.pinned.owner;
        let mut directories = BTreeMap::new();
        directories.insert(
            Vec::<String>::new(),
            self.root.try_clone().map_err(|_| MutationError::Storage)?,
        );
        for (path, expected) in &self.inspected_directories {
            let parent = directories
                .get(&path[..path.len() - 1])
                .ok_or(MutationError::Protection)?;
            let leaf = LeafName::new(path.last().ok_or(MutationError::Protection)?)?;
            let directory = native::inspect_directory(parent, owner, &leaf)?
                .ok_or(MutationError::Protection)?;
            if identity(&directory, owner)? != *expected {
                return Err(MutationError::Protection);
            }
            let directory = recovery::reopen(
                &directory,
                owner,
                true,
                FILE_TRAVERSE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                true,
            )?;
            directories.insert(path.clone(), directory);
        }
        while directories.len() > 1 {
            let (_, directory) = directories.pop_last().ok_or(MutationError::Storage)?;
            native::flush(&directory)?;
        }
        Ok(())
    }
    fn finish(&self) -> Outcome<()> {
        let destination = self.attempted.as_ref().ok_or(MutationError::Invalid)?;
        let pinned = &self.guard.directory.pinned;
        if identity(&self.root, &pinned.owner).ok() != Some(self.original) {
            return Err(MutationError::CommitUnknown);
        }
        native::flush(&self.root).map_err(|_| MutationError::CommitUnknown)?;
        let target = native::inspect_directory(pinned.file(), &pinned.owner, destination)
            .map_err(|_| MutationError::CommitUnknown)?
            .ok_or(MutationError::CommitUnknown)?;
        if identity(&target, &pinned.owner).ok() != Some(self.original) {
            return Err(MutationError::CommitUnknown);
        }
        Ok(())
    }
    /// Re-flush and inspect the retained root only; never rename, select another
    /// target, merge files or infer authorization from a leftover stage.
    pub fn reconcile(&mut self) -> Outcome<DirectoryIdentity> {
        self.finish()?;
        self.committed = true;
        self.guard.uncertain = false;
        Ok(self.original)
    }
}
