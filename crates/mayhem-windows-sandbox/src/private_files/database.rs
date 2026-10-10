//! Exclusive NTFS database IO. No writable handle escapes or path is reopened.
use super::*;
use mutation::{LeafName, MutationError, native};
use std::{
    fmt, io,
    os::windows::fs::FileExt,
    sync::{Mutex, MutexGuard},
};

/// The data handle disallows all sharing independently of the database library's
/// advisory-lock support. Pinned ancestors deny rename for the entire lifetime.
/// This does not introduce a journal or alter the database's commit protocol.
pub struct PrivateDatabaseFile {
    file: Mutex<File>,
    _pinned: Pinned,
}
impl fmt::Debug for PrivateDatabaseFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateDatabaseFile")
            .finish_non_exhaustive()
    }
}
impl PrivateDatabaseFile {
    pub fn open(path: &Path, existing: bool) -> std::result::Result<Self, MutationError> {
        let parent = path.parent().ok_or(MutationError::Invalid)?;
        let name = LeafName::new(
            path.file_name()
                .and_then(|v| v.to_str())
                .ok_or(MutationError::Invalid)?,
        )?;
        if name.as_str() == ".mayhem-ntfs.lock" {
            return Err(MutationError::Invalid);
        }
        let pinned =
            Pinned::open_with_final_sharing(parent, true, FILE_SHARE_READ | FILE_SHARE_WRITE)
                .map_err(|_| MutationError::Protection)?;
        native::require_ntfs(pinned.file())?;
        let file = native::database_file(&pinned, &name, existing)?;
        if existing
            && file
                .metadata()
                .map_err(|_| MutationError::Protection)?
                .len()
                == 0
        {
            return Err(MutationError::Protection);
        }
        // Flush creation metadata before handing ownership to a durable store.
        native::flush(&file)?;
        Ok(Self {
            file: Mutex::new(file),
            _pinned: pinned,
        })
    }
    fn locked(&self) -> io::Result<MutexGuard<'_, File>> {
        self.file
            .lock()
            .map_err(|_| io::Error::other("private database IO lock failed"))
    }
    pub fn len(&self) -> io::Result<u64> {
        Ok(self.locked()?.metadata()?.len())
    }
    pub fn is_empty(&self) -> io::Result<bool> {
        self.len().map(|len| len == 0)
    }
    pub fn set_len(&self, len: u64) -> io::Result<()> {
        checked_end(len, 0)?;
        self.locked()?.set_len(len)
    }
    pub fn sync_data(&self) -> io::Result<()> {
        self.locked()?.sync_all()
    }

    pub fn read(&self, mut offset: u64, mut out: &mut [u8]) -> io::Result<()> {
        checked_end(offset, out.len())?;
        let file = self.locked()?;
        while !out.is_empty() {
            match file.seek_read(out, offset) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "private database short read",
                    ));
                }
                Ok(n) => {
                    offset += n as u64;
                    out = &mut out[n..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn write(&self, mut offset: u64, mut data: &[u8]) -> io::Result<()> {
        checked_end(offset, data.len())?;
        let file = self.locked()?;
        while !data.is_empty() {
            match file.seek_write(data, offset) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "private database short write",
                    ));
                }
                Ok(n) => {
                    offset += n as u64;
                    data = &data[n..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
fn checked_end(offset: u64, bytes: usize) -> io::Result<()> {
    if offset
        .checked_add(bytes as u64)
        .is_none_or(|end| end > i64::MAX as u64)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private database offset out of range",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
