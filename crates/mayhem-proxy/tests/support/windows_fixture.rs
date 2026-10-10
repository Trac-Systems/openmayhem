//! A newly created user-owned NTFS directory. Never repair an existing ACL.
use mayhem_windows_sandbox::{LeafName, NtfsDirectory};
use std::path::{Path, PathBuf};
pub struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    pub fn new() -> Self {
        let parent = PathBuf::from(
            std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
                .expect("native tests require an explicit user-owned private NTFS fixture parent"),
        );
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).unwrap();
        let name = format!("worker-{}", blake3::hash(&random).to_hex());
        let mut guard = NtfsDirectory::open_existing(&parent)
            .unwrap()
            .into_lock()
            .unwrap();
        let mut pending = guard
            .stage_directory(LeafName::new(&format!("{name}.next")).unwrap(), &[], 1)
            .unwrap();
        pending.publish(LeafName::new(&name).unwrap()).unwrap();
        Self(parent.join(name))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
