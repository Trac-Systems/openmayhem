//! Standalone NTFS namespace commits. No setup, database or financial callers.
use super::*;
use std::io::Write;
mod native;
#[cfg(test)]
mod tests;

/// Resource bound, not a financial limit. Includes the existing tokenizer cap.
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MutationError {
    #[error("invalid protected mutation")]
    Invalid,
    #[error("protected NTFS authority rejected")]
    Protection,
    #[error("protected mutations require local NTFS")]
    UnsupportedFilesystem,
    #[error("protected storage is busy")]
    Busy,
    #[error("protected destination already exists")]
    Conflict,
    #[error("protected storage operation failed")]
    Storage,
    #[error("publication outcome is uncertain; retain and reconcile the original identity")]
    CommitUnknown,
}
type Outcome<T> = std::result::Result<T, MutationError>;

/// One component only; never an absolute path, stream, device or traversal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafName(String);
impl LeafName {
    pub fn new(value: &str) -> Outcome<Self> {
        if value.len() > 255
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        {
            return Err(MutationError::Invalid);
        }
        components(&format!("C:\\{value}")).map_err(|_| MutationError::Invalid)?;
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitIdentity {
    pub volume_serial: u32,
    pub file_id: u64,
    pub bytes: u64,
}
fn record_identity(file: &File) -> Outcome<CommitIdentity> {
    let v = information(file, false).map_err(|_| MutationError::Protection)?;
    Ok(CommitIdentity {
        volume_serial: v.dwVolumeSerialNumber,
        file_id: (u64::from(v.nFileIndexHigh) << 32) | u64::from(v.nFileIndexLow),
        bytes: (u64::from(v.nFileSizeHigh) << 32) | u64::from(v.nFileSizeLow),
    })
}

/// A pinned existing private directory. No raw handles or writable paths escape.
pub struct NtfsDirectory {
    pinned: Pinned,
}
impl NtfsDirectory {
    pub fn open_existing(path: &Path) -> Outcome<Self> {
        // Internal target opens during rename need write sharing. Still deny
        // deletion/rename of this directory; ancestor sharing and ACLs unchanged.
        let pinned =
            Pinned::open_with_final_sharing(path, true, FILE_SHARE_READ | FILE_SHARE_WRITE)
                .map_err(|_| MutationError::Protection)?;
        native::require_ntfs(pinned.file())?;
        Ok(Self { pinned })
    }
    pub fn try_lock(&self) -> Outcome<NtfsGuard<'_>> {
        let name = LeafName::new(".mayhem-ntfs.lock")?;
        acl::validate(self.pinned.file(), &self.pinned.owner, true)
            .map_err(|_| MutationError::Protection)?;
        let file = native::lock_file(&self.pinned, &name)?;
        native::lock(&file)?;
        // This flush also retains a newly created lock's namespace. Never
        // delete a lock on drop: that could split cooperating processes.
        if let Err(error) = native::flush(&file) {
            native::unlock(&file);
            return Err(error);
        }
        Ok(NtfsGuard {
            directory: self,
            file,
            name,
        })
    }
}

pub struct NtfsGuard<'a> {
    directory: &'a NtfsDirectory,
    file: File,
    name: LeafName,
}
impl Drop for NtfsGuard<'_> {
    fn drop(&mut self) {
        native::unlock(&self.file);
    }
}
impl<'d> NtfsGuard<'d> {
    pub fn read(&self, name: &LeafName, maximum: usize) -> Outcome<Option<Zeroizing<Vec<u8>>>> {
        if name == &self.name || maximum > MAX_BYTES {
            return Err(MutationError::Invalid);
        }
        let Some(file) = native::read_file(&self.directory.pinned, name)? else {
            return Ok(None);
        };
        super::read_file(&file, &self.directory.pinned.owner, maximum)
            .map(Some)
            .map_err(|_| MutationError::Protection)
    }

    /// First-create only. Errors can leave an uncommitted private temporary;
    /// this API never promotes or deletes such a file during recovery.
    pub fn prepare<'g>(
        &'g mut self,
        temporary: LeafName,
        bytes: &[u8],
        maximum: usize,
    ) -> Outcome<PendingFile<'g, 'd>> {
        if temporary == self.name || maximum > MAX_BYTES || bytes.len() > maximum {
            return Err(MutationError::Invalid);
        }
        let mut file = native::create_file(&self.directory.pinned, &temporary)?;
        file.write_all(bytes).map_err(|_| MutationError::Storage)?;
        native::flush(&file)?;
        let identity = record_identity(&file)?;
        if identity.bytes != bytes.len() as u64 {
            return Err(MutationError::Storage);
        }
        Ok(PendingFile {
            guard: self,
            file,
            temporary,
            identity,
            attempted: None,
            committed: false,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishMode {
    CreateNew,
    Replace,
}

/// Retains the original file and locked directory on success and uncertainty.
/// Dropping it closes handles only; it never removes, retries or rolls back.
pub struct PendingFile<'g, 'd> {
    guard: &'g mut NtfsGuard<'d>,
    file: File,
    temporary: LeafName,
    identity: CommitIdentity,
    attempted: Option<LeafName>,
    committed: bool,
}
impl PendingFile<'_, '_> {
    pub fn identity(&self) -> CommitIdentity {
        self.identity
    }
    pub fn temporary(&self) -> &LeafName {
        &self.temporary
    }
    pub fn attempted_destination(&self) -> Option<&LeafName> {
        self.attempted.as_ref()
    }

    pub fn publish(&mut self, destination: LeafName, mode: PublishMode) -> Outcome<CommitIdentity> {
        self.publish_inner(destination, mode, |_| Ok(()))
    }
    fn publish_inner(
        &mut self,
        destination: LeafName,
        mode: PublishMode,
        mut checkpoint: impl FnMut(Stage) -> Outcome<()>,
    ) -> Outcome<CommitIdentity> {
        if self.attempted.is_some() {
            return Err(MutationError::CommitUnknown);
        }
        if destination == self.temporary || destination == self.guard.name {
            return Err(MutationError::Invalid);
        }
        let pinned = &self.guard.directory.pinned;
        native::validate_file(&self.file, &pinned.owner)?;
        if record_identity(&self.file)? != self.identity {
            return Err(MutationError::Protection);
        }
        // Validate, then release the old target handle before rename. The
        // stable namespace lock serializes callers; no unsafe-target fallback.
        if let Some(file) = native::inspect_file(pinned, &destination)? {
            if mode == PublishMode::CreateNew {
                return Err(MutationError::Conflict);
            }
            if record_identity(&file)?.file_id == self.identity.file_id {
                return Err(MutationError::Protection);
            }
        }
        checkpoint(Stage::BeforeRename)?;
        // From this point even a syscall failure is conservatively uncertain.
        self.attempted = Some(destination.clone());
        native::rename(&self.file, pinned.file(), &destination, mode)
            .map_err(|_| MutationError::CommitUnknown)?;
        checkpoint(Stage::AfterRename).map_err(|_| MutationError::CommitUnknown)?;
        self.finish()?;
        checkpoint(Stage::AfterFlush).map_err(|_| MutationError::CommitUnknown)?;
        self.committed = true;
        Ok(self.identity)
    }
    fn finish(&self) -> Outcome<()> {
        let destination = self.attempted.as_ref().ok_or(MutationError::Invalid)?;
        let pinned = &self.guard.directory.pinned;
        native::validate_file(&self.file, &pinned.owner)
            .map_err(|_| MutationError::CommitUnknown)?;
        if record_identity(&self.file).ok() != Some(self.identity) {
            return Err(MutationError::CommitUnknown);
        }
        native::flush(&self.file).map_err(|_| MutationError::CommitUnknown)?;
        let target = native::inspect_file(pinned, destination)
            .map_err(|_| MutationError::CommitUnknown)?
            .ok_or(MutationError::CommitUnknown)?;
        if record_identity(&target).ok() != Some(self.identity) {
            return Err(MutationError::CommitUnknown);
        }
        Ok(())
    }
    /// Re-flush and inspect only. Never repeats rename or selects another file.
    /// If the old target still exists, uncertainty remains for the caller.
    pub fn reconcile(&mut self) -> Outcome<CommitIdentity> {
        self.finish()?;
        self.committed = true;
        Ok(self.identity)
    }
    pub fn is_committed(&self) -> bool {
        self.committed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    BeforeRename,
    AfterRename,
    AfterFlush,
}
