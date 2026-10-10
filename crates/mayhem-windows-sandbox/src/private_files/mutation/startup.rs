//! One protected recovery record for transient Windows sandbox registration.
//! Never delete this file: deleting a locked inode would split launch authority.
use super::*;
use std::io::{Read, Seek, SeekFrom, Write};
use windows_sys::Win32::System::IO::OVERLAPPED;

pub(crate) const PREFIX: &str = "mayhem-proxy-";
pub(crate) const RECORD_BYTES: usize = 64;

pub(crate) fn valid_name(name: &str) -> bool {
    name.len() == RECORD_BYTES
        && name.strip_prefix(PREFIX).is_some_and(|nonce| {
            nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

pub(crate) struct StartupJournal {
    file: File,
    _pinned: Pinned,
}
impl StartupJournal {
    pub(crate) fn open(parent: &Path) -> Outcome<Self> {
        let mut pinned =
            Pinned::open_with_final_policy(parent, true, FILE_SHARE_READ | FILE_SHARE_WRITE, false)
                .map_err(|_| MutationError::Protection)?;
        native::require_ntfs(pinned.file())?;
        let directory =
            native::startup_directory(&pinned, &LeafName::new(".mayhem-proxy-startup-v1")?)?;
        pinned.handles.push(directory);
        let file = native::startup_file(&pinned, &LeafName::new("identity")?)?;
        let mut overlap: OVERLAPPED = unsafe { zeroed() };
        // Serialize only registration/CreateProcess/unregistration, never the
        // child's inference. The kernel releases this lock on parent death.
        if unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                1,
                0,
                &mut overlap,
            )
        } == 0
        {
            return Err(MutationError::Protection);
        }
        Ok(Self {
            file,
            _pinned: pinned,
        })
    }

    pub(crate) fn previous(&mut self) -> Outcome<Option<String>> {
        let len = self
            .file
            .metadata()
            .map_err(|_| MutationError::Storage)?
            .len();
        if len > RECORD_BYTES as u64 {
            return Err(MutationError::Protection);
        }
        // Only the very first write can have a short length. Registration is
        // forbidden until the complete 64-byte name has been flushed. Later
        // records overwrite in place without truncating or clearing this file.
        if len < RECORD_BYTES as u64 {
            return Ok(None);
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| MutationError::Storage)?;
        let mut bytes = [0u8; RECORD_BYTES];
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| MutationError::Storage)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| MutationError::Protection)?;
        if !valid_name(text) {
            return Err(MutationError::Protection);
        }
        Ok(Some(text.to_owned()))
    }

    /// Call only after removing the previous registration. A crash during this
    /// overwrite therefore has no active old/new identity to lose. After the
    /// flush succeeds the new identity can be registered, and its name survives
    /// parent death. Never clear the record after cleanup (another crash edge).
    pub(crate) fn record(&mut self, name: &str) -> Outcome<()> {
        if !valid_name(name) {
            return Err(MutationError::Invalid);
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| MutationError::Storage)?;
        self.file
            .write_all(name.as_bytes())
            .map_err(|_| MutationError::Storage)?;
        native::flush(&self.file)
    }
}
impl Drop for StartupJournal {
    fn drop(&mut self) {
        native::unlock(&self.file);
    }
}
