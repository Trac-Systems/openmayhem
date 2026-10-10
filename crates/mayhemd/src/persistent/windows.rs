//! Persistent children use the existing protected NTFS database boundary.
//! No permission repair, shared handles or second open of an unchecked path.
use super::*;
use mayhem_windows_sandbox::{LeafName, NtfsDirectory, PrivateDatabaseFile};

pub(super) fn private_file(path: &Path, create: bool) -> Result<PrivateDatabaseFile> {
    let parent = path.parent().context("missing supervisor directory")?;
    if create && !path_exists(parent)? {
        let home = parent.parent().context("missing supervisor home")?;
        let mut guard = NtfsDirectory::open_existing(home)?.into_lock()?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("supervisor directory nonce failed"))?;
        let temporary = LeafName::new(&format!(".supervisor-{}", blake3::hash(&nonce).to_hex()))?;
        let mut pending = guard.stage_directory(temporary, &[], 0)?;
        pending.publish(LeafName::new(
            parent
                .file_name()
                .and_then(|s| s.to_str())
                .context("invalid supervisor directory name")?,
        )?)?;
    }
    // The handle retains validated ancestors and exclusive access throughout
    // redb's lifetime, including failed initialization and recovery.
    Ok(PrivateDatabaseFile::open(path, !create)?)
}

pub(super) fn file_length(file: &PrivateDatabaseFile) -> std::io::Result<u64> {
    file.len()
}

pub(super) fn create_database(
    builder: &redb::Builder,
    file: PrivateDatabaseFile,
) -> Result<Database> {
    Ok(builder.create_with_backend(Backend(file))?)
}

#[derive(Debug)]
struct Backend(PrivateDatabaseFile);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = PathBuf::from(
                std::env::var_os("MAYHEM_WINDOWS_SETUP_FIXTURE_PARENT")
                    .expect("explicit current-owner private NTFS fixture parent required"),
            );
            let mut guard = NtfsDirectory::open_existing(&parent)
                .unwrap()
                .into_lock()
                .unwrap();
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).unwrap();
            let name = format!("supervisor-test-{}", blake3::hash(&nonce).to_hex());
            let mut pending = guard
                .stage_directory(LeafName::new(&format!("{name}.next")).unwrap(), &[], 0)
                .unwrap();
            pending.publish(LeafName::new(&name).unwrap()).unwrap();
            Self(parent.join(name))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn child() -> ChildConfig {
        serde_json::from_value(
            serde_json::json!({"name":"synthetic-proxy", "command":"unused-fixture-command",
            "args":["fixture-argument"], "env":{"SYNTHETIC_ONLY":"retained-value"}}),
        )
        .unwrap()
    }

    #[test]
    #[ignore = "requires isolated native Windows private NTFS fixture parent"]
    fn windows_persistent_children_reopen_exactly_and_do_not_create_legacy_state() {
        let fixture = Fixture::new();
        let (store, restored) = Store::load(&fixture.0).unwrap();
        assert!(restored.is_empty());
        assert!(!fixture.0.join("supervisor-private").exists());
        let child = child();
        let expected = child_config_hash(&child).unwrap();
        store.lock().unwrap().add(&child).unwrap();
        assert!(store.lock().unwrap().add(&child).is_err());
        assert!(
            Store::load(&fixture.0).is_err(),
            "second database owner must fail"
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .inspect(&child.name, &expected)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .inspect(&child.name, &"0".repeat(64))
                .unwrap(),
            Some(false)
        );
        let mut oversized = child.clone();
        oversized.args.push("x".repeat(MAX_CHILD_BYTES));
        assert!(store.lock().unwrap().add(&oversized).is_err());
        drop(store);
        let (store, restored) = Store::load(&fixture.0).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(
            serde_json::to_value(&restored[0]).unwrap(),
            serde_json::to_value(&child).unwrap()
        );
        assert!(store.lock().unwrap().remove(&child.name).unwrap());
        drop(store);
        let (store, restored) = Store::load(&fixture.0).unwrap();
        assert!(restored.is_empty());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .inspect(&child.name, &expected)
                .unwrap(),
            None
        );
        drop(store);
    }

    #[test]
    #[ignore = "requires isolated native Windows private NTFS fixture parent"]
    fn windows_persistent_children_refuse_missing_empty_corrupt_unrelated_and_aliased_storage() {
        const FOREIGN: TableDefinition<&str, &str> = TableDefinition::new("unrelated_fixture");
        for kind in ["missing", "empty", "corrupt", "unrelated", "hardlink"] {
            let fixture = Fixture::new();
            let (store, _) = Store::load(&fixture.0).unwrap();
            store.lock().unwrap().add(&child()).unwrap();
            drop(store);
            let path = fixture.0.join("supervisor-private/children.redb");
            match kind {
                "missing" => fs::remove_file(&path).unwrap(),
                "empty" => fs::write(&path, []).unwrap(),
                "corrupt" => fs::write(&path, b"not a database").unwrap(),
                "unrelated" => {
                    fs::remove_file(&path).unwrap();
                    let db =
                        create_database(&Database::builder(), private_file(&path, true).unwrap())
                            .unwrap();
                    let tx = db.begin_write().unwrap();
                    {
                        tx.open_table(FOREIGN)
                            .unwrap()
                            .insert("retained", "original value")
                            .unwrap();
                    }
                    tx.commit().unwrap();
                }
                "hardlink" => fs::hard_link(&path, fixture.0.join("alias")).unwrap(),
                _ => unreachable!(),
            }
            let before = fs::read(&path).ok().map(|bytes| blake3::hash(&bytes));
            assert!(Store::load(&fixture.0).is_err(), "{kind}");
            if kind == "unrelated" {
                // Opening redb may maintain its own allocator/header pages.
                // The invariant is preserving the foreign schema/data without
                // creating a replacement child registry, not byte identity.
                let db = create_database(&Database::builder(), private_file(&path, false).unwrap())
                    .unwrap();
                let read = db.begin_read().unwrap();
                assert!(read.open_table(CHILDREN).is_err());
                assert_eq!(
                    read.open_table(FOREIGN)
                        .unwrap()
                        .get("retained")
                        .unwrap()
                        .unwrap()
                        .value(),
                    "original value"
                );
            } else {
                assert_eq!(
                    fs::read(&path).ok().map(|bytes| blake3::hash(&bytes)),
                    before,
                    "must preserve {kind} storage"
                );
            }
        }
    }
}
