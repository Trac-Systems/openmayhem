//! Opt-in durable dynamic children. Existing nonpersistent children are unchanged.
//! Only authenticated local control requests can add these. Retain full config in
//! protected transactional storage, never reconstruct credentials from public status.
use super::{validate_children, ChildConfig};
use anyhow::{ensure, Context, Result};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const CHILDREN: TableDefinition<&str, &[u8]> = TableDefinition::new("supervised_children_v1");
const MAX_CHILDREN: u64 = 256; // Local processes, not offers, markets or public list size.
const MAX_CHILD_BYTES: usize = 64 * 1024;

pub struct Store {
    path: PathBuf,
    database: Option<Database>,
}
pub type Shared = Arc<Mutex<Store>>;

pub fn capabilities() -> Vec<&'static str> {
    if cfg!(unix) {
        vec!["persistent_children_v1", "persistent_child_inspect_v1"]
    } else {
        Vec::new()
    }
}

impl Store {
    pub fn load(home: &Path) -> Result<(Shared, Vec<ChildConfig>)> {
        let path = home.join("supervisor-private").join("children.redb");
        let mut store = Self {
            path,
            database: None,
        };
        // Missing storage is the legacy behavior; do not create files, or change
        // native startup, unless persistence was explicitly requested previously.
        let mut children = Vec::new();
        if path_exists(
            store
                .path
                .parent()
                .context("missing private supervisor directory")?,
        )? {
            ensure!(
                path_exists(&store.path)?,
                "persistent supervisor store is missing; refusing to reset it"
            );
            let db = store.database()?;
            let read = db.begin_read()?;
            let table = read.open_table(CHILDREN)?;
            ensure!(
                table.len()? <= MAX_CHILDREN,
                "persistent child quota exceeded"
            );
            for item in table.iter()? {
                let (key, value) = item?;
                ensure!(
                    value.value().len() <= MAX_CHILD_BYTES,
                    "persistent child is too large"
                );
                let child: ChildConfig = serde_json::from_slice(value.value())?;
                ensure!(
                    key.value() == child.name,
                    "persistent child identity mismatch"
                );
                children.push(child);
            }
            validate_children(&children)?;
        }
        Ok((Arc::new(Mutex::new(store)), children))
    }
    fn database(&mut self) -> Result<&Database> {
        if self.database.is_none() {
            let existing = path_exists(&self.path)?;
            let file = private_file(&self.path, !existing)?;
            ensure!(
                !existing || file.metadata()?.len() > 0,
                "persistent supervisor store is empty; refusing to reset it"
            );
            let mut builder = Database::builder();
            builder.set_cache_size(1024 * 1024);
            let database = builder.create_file(file)?;
            if existing {
                let read = database.begin_read()?;
                read.open_table(CHILDREN)
                    .context("existing supervisor store has no child registry")?;
            } else {
                let mut write = database.begin_write()?;
                write.set_durability(Durability::Immediate)?;
                {
                    write.open_table(CHILDREN)?;
                }
                write.commit()?;
            }
            self.database = Some(database);
        }
        Ok(self.database.as_ref().expect("initialized above"))
    }
    fn add(&mut self, child: &ChildConfig) -> Result<()> {
        validate_children(std::slice::from_ref(child))?;
        let bytes = serde_json::to_vec(child)?;
        ensure!(
            bytes.len() <= MAX_CHILD_BYTES,
            "persistent child is too large"
        );
        let mut tx = self.database()?.begin_write()?;
        tx.set_durability(Durability::Immediate)?;
        {
            let mut table = tx.open_table(CHILDREN)?;
            ensure!(
                table.get(child.name.as_str())?.is_none(),
                "persistent child already exists; remove it before replacing it"
            );
            ensure!(
                table.len()? < MAX_CHILDREN,
                "persistent child quota exceeded"
            );
            table.insert(child.name.as_str(), bytes.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }
    fn inspect(&self, name: &str, expected: &str) -> Result<Option<bool>> {
        let Some(database) = &self.database else {
            return Ok(None);
        };
        let read = database.begin_read()?;
        let table = read.open_table(CHILDREN)?;
        let Some(value) = table.get(name)? else {
            return Ok(None);
        };
        ensure!(
            value.value().len() <= MAX_CHILD_BYTES,
            "persistent child is too large"
        );
        let child: ChildConfig = serde_json::from_slice(value.value())?;
        ensure!(child.name == name, "persistent child identity mismatch");
        Ok(Some(child_config_hash(&child)? == expected))
    }
    fn remove(&mut self, name: &str) -> Result<bool> {
        let Some(database) = &self.database else {
            return Ok(false);
        };
        let mut tx = database.begin_write()?;
        tx.set_durability(Durability::Immediate)?;
        let removed = { tx.open_table(CHILDREN)?.remove(name)?.is_some() };
        tx.commit()?;
        Ok(removed)
    }
}

pub async fn add(store: Shared, child: ChildConfig) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("persistent child store lock failed"))?
            .add(&child)
    })
    .await
    .context("persisting supervised child")?
}
pub fn child_config_hash(child: &ChildConfig) -> Result<String> {
    let mut bytes = b"mayhem/supervisor/child-config/v1\0".to_vec();
    bytes.extend(mayhem_proto::stable_json_bytes(&serde_json::to_value(
        child,
    )?)?);
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

pub async fn inspect(store: Shared, name: String, expected: String) -> Result<Option<bool>> {
    tokio::task::spawn_blocking(move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("persistent child store lock failed"))?
            .inspect(&name, &expected)
    })
    .await
    .context("inspecting supervised child")?
}

pub async fn remove(store: Shared, name: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("persistent child store lock failed"))?
            .remove(&name)
    })
    .await
    .context("removing persistent supervised child")?
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
#[cfg(unix)]
fn private_file(path: &Path, create: bool) -> Result<fs::File> {
    use rustix::fs::{fstat, open, FileType, Mode, OFlags};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let parent = path
        .parent()
        .context("missing private supervisor directory")?;
    if create && !path_exists(parent)? {
        fs::DirBuilder::new().mode(0o700).create(parent)?;
    }
    let metadata = fs::symlink_metadata(parent)?;
    let uid = rustix::process::geteuid().as_raw();
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
            && metadata.mode() & 0o077 == 0,
        "persistent children require an owner-only supervisor directory"
    );
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = open(
        path,
        if create {
            flags | OFlags::CREATE | OFlags::EXCL
        } else {
            flags
        },
        Mode::RUSR | Mode::WUSR,
    )?;
    let stat = fstat(&file)?;
    ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && stat.st_uid == uid
            && stat.st_mode & 0o077 == 0
            && stat.st_nlink == 1,
        "persistent children require an owner-only regular file"
    );
    Ok(file.into())
}
#[cfg(not(unix))]
fn private_file(_: &Path, _: bool) -> Result<fs::File> {
    anyhow::bail!("persistent child storage requires supported filesystem protection")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{symlink, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    fn temp() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mayhemd-persist-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn child() -> ChildConfig {
        super::super::tests::long_running_test_child("proxy-test")
    }
    #[test]
    fn durable_children_restore_exact_config_and_removal_survives_restart() {
        let home = temp();
        let (store, restored) = Store::load(&home).unwrap();
        assert!(restored.is_empty());
        assert!(!home.join("supervisor-private").exists());
        let mut child = child();
        child
            .env
            .insert("TEST_SECRET".into(), "fixture-only".into());
        store.lock().unwrap().add(&child).unwrap();
        assert!(store.lock().unwrap().add(&child).is_err());
        drop(store);
        let (store, restored) = Store::load(&home).unwrap();
        assert_eq!(
            serde_json::to_value(&restored[0]).unwrap(),
            serde_json::to_value(child).unwrap()
        );
        store.lock().unwrap().remove("proxy-test").unwrap();
        drop(store);
        let (store, restored) = Store::load(&home).unwrap();
        assert!(restored.is_empty());
        drop(store);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn malformed_or_unprotected_storage_is_not_reset_and_duplicate_owner_is_rejected() {
        let home = temp();
        let (store, _) = Store::load(&home).unwrap();
        store.lock().unwrap().add(&child()).unwrap();
        assert!(Store::load(&home).is_err()); // redb holds the controller lock.
        drop(store);
        let path = home.join("supervisor-private/children.redb");
        let original_len = fs::metadata(&path).unwrap().len();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::load(&home).is_err());
        assert_eq!(fs::metadata(&path).unwrap().len(), original_len);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, b"not a database").unwrap();
        assert!(Store::load(&home).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not a database");
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn aliases_and_oversized_entries_fail_before_activation() {
        let home = temp();
        let (store, _) = Store::load(&home).unwrap();
        let mut large = child();
        large.args.push("x".repeat(MAX_CHILD_BYTES));
        assert!(store.lock().unwrap().add(&large).is_err());
        store.lock().unwrap().add(&child()).unwrap();
        drop(store);
        let path = home.join("supervisor-private/children.redb");
        let copy = home.join("copy");
        fs::rename(&path, &copy).unwrap();
        symlink(&copy, &path).unwrap();
        assert!(Store::load(&home).is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&copy, &path).unwrap();
        assert!(Store::load(&home).is_err());
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn prior_installation_cannot_be_silently_reset_by_missing_empty_or_unrelated_store() {
        for kind in ["missing", "empty", "unrelated", "dangling"] {
            let home = temp();
            let (store, _) = Store::load(&home).unwrap();
            store.lock().unwrap().add(&child()).unwrap();
            drop(store);
            let path = home.join("supervisor-private/children.redb");
            fs::remove_file(&path).unwrap();
            match kind {
                "empty" => {
                    fs::write(&path, []).unwrap();
                }
                "unrelated" => {
                    let file = private_file(&path, true).unwrap();
                    drop(Database::builder().create_file(file).unwrap());
                }
                "dangling" => {
                    symlink(home.join("absent"), &path).unwrap();
                }
                _ => (),
            }
            assert!(Store::load(&home).is_err(), "{kind}");
            if kind == "empty" {
                assert_eq!(fs::metadata(&path).unwrap().len(), 0);
            }
            if kind == "missing" {
                assert!(!path.exists());
            }
            fs::remove_dir_all(home).unwrap();
        }
    }
}
