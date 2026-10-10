//! Private disposable fixtures; never modify an existing host directory ACL.
use super::*;
#[cfg(windows)]
use mayhem_windows_sandbox::{LeafName, NtfsDirectory, PublishMode};
#[cfg(unix)]
use std::{io::Write, os::unix::fs::OpenOptionsExt};

#[cfg(unix)]
pub(super) type PrivateDirectory = tempfile::TempDir;
#[cfg(windows)]
pub(super) struct PrivateDirectory(PathBuf);
#[cfg(windows)]
impl PrivateDirectory {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}
#[cfg(windows)]
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn private_tempdir() -> PrivateDirectory {
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }
    #[cfg(windows)]
    {
        let parent = PathBuf::from(
            std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
                .expect("explicit private native Windows fixture parent required"),
        );
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = parent.join(format!("setup-{}", blake3::hash(&nonce).to_hex()));
        private_directory(&path);
        PrivateDirectory(path)
    }
}

pub(super) fn private_directory(path: &Path) {
    #[cfg(unix)]
    {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(windows)]
    {
        let mut guard = NtfsDirectory::open_existing(path.parent().unwrap())
            .unwrap()
            .into_lock()
            .unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let mut pending = guard
            .stage_directory(LeafName::new(&format!("{name}.next")).unwrap(), &[], 1)
            .unwrap();
        pending.publish(LeafName::new(name).unwrap()).unwrap();
    }
}

pub(super) fn private(path: &Path, bytes: &[u8]) {
    #[cfg(unix)]
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    #[cfg(windows)]
    {
        let mut guard = NtfsDirectory::open_existing(path.parent().unwrap())
            .unwrap()
            .into_lock()
            .unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let mut pending = guard
            .prepare(
                LeafName::new(&format!("{name}.next")).unwrap(),
                bytes,
                bytes.len().max(1),
            )
            .unwrap();
        pending
            .publish(LeafName::new(name).unwrap(), PublishMode::Replace)
            .unwrap();
    }
}

pub(super) fn write_evidence(path: &Path, value: &Value) {
    assert!(
        !path.exists(),
        "do not overwrite previous acceptance evidence"
    );
    private(path, &serde_json::to_vec_pretty(value).unwrap());
}

pub(super) fn assert_private_file(path: &Path) {
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    #[cfg(windows)]
    mayhem_windows_sandbox::read_private_file(path, 16 * 1024 * 1024).unwrap();
}
