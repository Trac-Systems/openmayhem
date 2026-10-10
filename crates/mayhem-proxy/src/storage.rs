//! Preserve the Unix redb path; Windows retains protected exclusive NTFS handles.
#[cfg(not(windows))]
pub(crate) type PrivateFile = std::fs::File;
#[cfg(windows)]
pub(crate) type PrivateFile = mayhem_windows_sandbox::PrivateDatabaseFile;

pub(crate) fn create(
    builder: &redb::Builder,
    file: PrivateFile,
) -> std::result::Result<redb::Database, redb::DatabaseError> {
    #[cfg(not(windows))]
    {
        builder.create_file(file)
    }
    #[cfg(windows)]
    {
        builder.create_with_backend(Backend(file))
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct Backend(PrivateFile);
#[cfg(windows)]
impl redb::StorageBackend for Backend {
    fn len(&self) -> std::io::Result<u64> {
        self.0.len()
    }
    fn read(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
        self.0.read(offset, out)
    }
    fn write(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.0.write(offset, data)
    }
    fn set_len(&self, len: u64) -> std::io::Result<()> {
        self.0.set_len(len)
    }
    fn sync_data(&self) -> std::io::Result<()> {
        self.0.sync_data()
    }
    // redb's default close is a no-op. The exclusive handle and ancestor guards
    // remain held until the backend itself drops; there is no advisory fallback.
}
