//! Exact-handle adaptations required by the existing setup store protocol.
use super::*;
use windows_sys::Wdk::Storage::FileSystem as nt;
#[cfg(test)]
mod tests;

pub(super) fn reopen(
    original: &File,
    owner: &[u8],
    directory: bool,
    access: u32,
    sharing: u32,
    write_through: bool,
) -> Outcome<File> {
    let before = information(original, directory).map_err(|_| MutationError::Protection)?;
    acl::validate(original, owner, true).map_err(|_| MutationError::Protection)?;
    let handle = unsafe {
        ReOpenFile(
            original.as_raw_handle(),
            access | READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE_ACCESS,
            sharing,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                }
                | if write_through {
                    FILE_FLAG_WRITE_THROUGH
                } else {
                    0
                },
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(MutationError::Protection);
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let after = information(&file, directory).map_err(|_| MutationError::Protection)?;
    if (
        before.dwVolumeSerialNumber,
        before.nFileIndexHigh,
        before.nFileIndexLow,
    ) != (
        after.dwVolumeSerialNumber,
        after.nFileIndexHigh,
        after.nFileIndexLow,
    ) {
        return Err(MutationError::Protection);
    }
    acl::validate(&file, owner, true).map_err(|_| MutationError::Protection)?;
    if write_through {
        let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
        let mut mode: nt::FILE_MODE_INFORMATION = unsafe { zeroed() };
        if unsafe {
            nt::NtQueryInformationFile(
                file.as_raw_handle(),
                &mut io,
                &mut mode as *mut _ as _,
                size_of::<nt::FILE_MODE_INFORMATION>() as u32,
                nt::FileModeInformation,
            )
        } != 0
            || mode.Mode & nt::FILE_WRITE_THROUGH == 0
        {
            return Err(MutationError::Protection);
        }
    }
    Ok(file)
}

impl NtfsGuard<'_> {
    /// Recover a fixed uncommitted temporary slot. Never call after an attempted
    /// publication in the current operation. Both names are independently pinned;
    /// the sole temporary link must identify a different file from the original.
    pub fn discard_uncommitted(
        &mut self,
        original: &LeafName,
        temporary: &LeafName,
    ) -> Outcome<()> {
        if self.uncertain {
            return Err(MutationError::CommitUnknown);
        }
        if original == temporary || original == &self.name || temporary == &self.name {
            return Err(MutationError::Invalid);
        }
        let pinned = &self.directory.pinned;
        // Retain this data-read handle (denies write/delete) through cleanup.
        let target = native::read_file(pinned, original)?;
        let Some(candidate) = native::inspect_file(pinned, temporary)? else {
            return Ok(());
        };
        let candidate_id = record_identity(&candidate)?;
        if target
            .as_ref()
            .map(record_identity)
            .transpose()?
            .is_some_and(|v| {
                v.volume_serial == candidate_id.volume_serial && v.file_id == candidate_id.file_id
            })
        {
            return Err(MutationError::Protection);
        }
        let deleting = reopen(
            &candidate,
            &pinned.owner,
            false,
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            false,
        )?;
        if record_identity(&deleting)? != candidate_id {
            return Err(MutationError::Protection);
        }
        // Acquire the full-flush authority before attempting deletion.
        let parent = reopen(
            pinned.file(),
            &pinned.owner,
            true,
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            true,
        )?;
        drop(candidate);
        let mut disposition = nt::FILE_DISPOSITION_INFORMATION { DeleteFile: true };
        let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
        self.uncertain = true;
        let status = unsafe {
            nt::NtSetInformationFile(
                deleting.as_raw_handle(),
                &mut io,
                &mut disposition as *mut _ as _,
                size_of::<nt::FILE_DISPOSITION_INFORMATION>() as u32,
                nt::FileDispositionInformation,
            )
        };
        // Following successful disposition, close is the only permitted operation.
        drop(deleting);
        if status != 0 {
            return Err(MutationError::CommitUnknown);
        }
        native::flush(&parent).map_err(|_| MutationError::CommitUnknown)?;
        drop(target);
        self.uncertain = false;
        Ok(())
    }
}
