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
        if temporary == self.name {
            return Err(MutationError::Invalid);
        }
        let plan = plan(entries, maximum)?;
        let pinned = &self.directory.pinned;
        let root = native::create_directory(pinned.file(), &pinned.owner, &temporary)?;
        let original = identity(&root, &pinned.owner)?;
        let mut directories = BTreeMap::<Vec<String>, File>::new();
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
}
impl PendingDirectory<'_, '_> {
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
        self.attempted = Some(destination.clone());
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
        Ok(self.original)
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
        Ok(self.original)
    }
}
