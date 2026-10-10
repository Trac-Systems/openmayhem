//! Runtime directories use the same protected NTFS boundary as setup/stores.
use super::*;
use mayhem_windows_sandbox::{LeafName, NtfsDirectory};

pub(super) fn private_dir(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Protection);
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            // The NTFS validator rejects files, reparses, weak ACLs and remote
            // volumes. Never repair permissions or replace an existing object.
            return NtfsDirectory::open_existing(path)
                .map(|_| ())
                .map_err(|_| Error::Protection);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(Error::Protection),
    }
    let name = path.file_name().and_then(|s| s.to_str()).ok_or(Error::Protection)?;
    let destination = LeafName::new(name).map_err(|_| Error::Protection)?;
    let parent = path.parent().ok_or(Error::Protection)?;
    let mut guard = NtfsDirectory::open_existing(parent)
        .and_then(NtfsDirectory::into_lock)
        .map_err(|_| Error::Protection)?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| Error::Setup)?;
    let temporary = LeafName::new(&format!(".proxy-runtime-{}", blake3::hash(&nonce).to_hex()))
        .map_err(|_| Error::Protection)?;
    let mut pending = guard.stage_directory(temporary, &[], 0).map_err(|_| Error::Setup)?;
    // First-create only. An uncertain rename remains an error, never a retry,
    // deletion or permission reset. Reopening checks whichever original exists.
    pending.publish(destination).map_err(|_| Error::Setup)?;
    drop(pending);
    drop(guard);
    NtfsDirectory::open_existing(path).map(|_| ()).map_err(|_| Error::Protection)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an isolated native Windows private NTFS fixture parent"]
    fn runtime_directories_create_reopen_and_refuse_replacement_or_recursive_parents() {
        let parent = std::path::PathBuf::from(std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
            .expect("provide an existing isolated private NTFS fixture directory"));
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let root = parent.join(format!("runtime-test-{}", blake3::hash(&nonce).to_hex()));
        private_dir(&root).unwrap();
        private_dir(&root).unwrap();
        let state = root.join("state");
        private_dir(&state).unwrap();
        let original = state.join("retained");
        std::fs::write(&original, b"unchanged original state").unwrap();
        private_dir(&state).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"unchanged original state");
        assert!(matches!(private_dir(&original), Err(Error::Protection)));
        assert!(private_dir(&root.join("missing").join("child")).is_err());
        assert!(!root.join("missing").exists());
        assert!(matches!(private_dir(Path::new("relative")), Err(Error::Protection)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
