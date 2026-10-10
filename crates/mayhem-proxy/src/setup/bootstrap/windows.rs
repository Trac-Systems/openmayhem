//! First-install publication using the shared factory and protected NTFS tree.
use super::*;
use mayhem_windows_sandbox::{DirectoryEntry, LeafName, NtfsDirectory};

pub fn create(destination: &Path, host: Host, choices: Choices) -> Result<Bundle> {
    require(destination.is_absolute())?;
    let name = destination
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or(Error::Protection)?;
    require(
        !name.starts_with('.')
            && name.len() <= 64
            && name
                .bytes()
                .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || b"_-".contains(&v)),
    )?;
    let leaf = LeafName::new(name).map_err(store::windows_error)?;
    let parent = destination.parent().ok_or(Error::Protection)?;
    let authority = NtfsDirectory::open_existing(parent).map_err(store::windows_error)?;
    let mut guard = authority.into_lock().map_err(store::windows_error)?;
    let parent = fs::canonicalize(parent).map_err(|_| Error::Protection)?;
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(Error::Conflict),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(Error::Protection),
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
    let temporary = LeafName::new(&format!(
        ".proxy-bootstrap-{}",
        blake3::hash(&nonce).to_hex()
    ))
    .map_err(store::windows_error)?;
    let stage = parent.join(temporary.as_str());
    let mut files = Vec::<(Vec<LeafName>, Zeroizing<Vec<u8>>)>::new();
    let validation = generate(&stage, host, choices, |path, bytes| {
        let relative = path.strip_prefix(&stage).map_err(|_| Error::Protection)?;
        require(relative.components().count() == 1)?;
        let name = relative.to_str().ok_or(Error::Protection)?;
        files.push((
            vec![LeafName::new(name).map_err(store::windows_error)?],
            Zeroizing::new(bytes.to_vec()),
        ));
        Ok(())
    })?;
    let directories =
        ["state", "runtime", "worker"].map(|name| vec![LeafName::new(name).expect("fixed leaf")]);
    let entries = directories
        .iter()
        .map(|path| DirectoryEntry::Directory { path })
        .chain(
            files
                .iter()
                .map(|(path, bytes)| DirectoryEntry::File { path, bytes }),
        )
        .collect::<Vec<_>>();
    let mut pending = guard
        .stage_directory(temporary, &entries, 64 * 1024 * 1024)
        .map_err(store::windows_error)?;
    pending
        .prepare_for_inspection()
        .map_err(store::windows_error)?;
    validate_generated(&stage, validation)?;
    // Installation validation waits for reaping. Only its bounded recovery
    // record may remain; never publish a leaked image or an unrelated file.
    let worker = stage.join("worker");
    let entries = fs::read_dir(&worker)
        .map_err(|_| Error::Protection)?
        .take(2)
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|_| Error::Protection)?;
    require(entries.len() <= 1)?;
    if let Some(entry) = entries.first() {
        require(entry.file_name() == ".decoder-image-v1")?;
        let bytes = private_file(&entry.path(), 64).map_err(|_| Error::Protection)?;
        require(
            bytes.len() == 64
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)),
        )?;
    }
    pending.publish(leaf).map_err(store::windows_error)?;
    // No uncertain-error cleanup or automatic retry. The typed original bundle
    // remains the only source of restart authority, exactly as on Unix.
    Ok(Bundle {
        schema_version: 1,
        config_file: destination.join("wizard.json"),
        profile: "bounded_single_connection_v1",
        network_requests: 0,
        capacity_created: false,
        authorizes_probe: false,
        authorizes_publication: false,
        authorizes_run: false,
    })
}
