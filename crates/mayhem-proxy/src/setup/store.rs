//! One bounded atomic draft file. A stable protected lock serializes processes;
//! fsync + same-directory rename preserves the prior or new complete revision.
use super::*;

fn file_limit(name: &str) -> usize {
    if matches!(name, "draft.json" | "draft.next") {
        MAX_DRAFT_BYTES
    } else if name.starts_with("wizard-managed-") || name == "wizard-managed.next" {
        4 * 1024 * 1024
    } else {
        MAX_BYTES
    }
}

pub struct Store {
    pub(super) directory: PathBuf,
}
impl Store {
    /// The caller chooses an existing private directory. No other provider,
    /// wallet, financial store or upstream process is opened or initialized.
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let store = Self {
            directory: directory.into(),
        };
        let _guard = Guard::open(&store.directory)?;
        Ok(store)
    }
    pub fn create(&self, input: Input) -> Result<Review> {
        input.validate()?;
        let connection = input.connection()?;
        let guard = Guard::open(&self.directory)?;
        if guard.read()?.is_some() {
            return Err(Error::Conflict);
        }
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
        let record = Record {
            schema_version: 1,
            id: Digest::hash("mayhem/proxy/setup-id/v1", &[&nonce]),
            revision: 1,
            input,
            connection,
            checked: None,
            probe_scope: None,
            probe: None,
            admission: None,
            publication: None,
        };
        guard.write(&record)?;
        record.review()
    }
    pub fn inspect(&self) -> Result<Review> {
        Guard::open(&self.directory)?
            .read()?
            .ok_or(Error::Missing)?
            .review()
    }
    pub fn update(&self, expected_revision: u64, input: Input) -> Result<Review> {
        input.validate()?;
        let connection = input.connection()?;
        let guard = Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        if record.publication.as_ref().is_some_and(|p| !p.complete()) {
            return Err(Error::PublicationRecovery);
        }
        // A setup ID cannot be transferred to another wallet or network.
        require(
            input.network == record.input.network
                && input.provider_pubkey == record.input.provider_pubkey,
        )?;
        record.next(expected_revision)?;
        record.retain_probe_configuration()?;
        record.input = input;
        record.connection = connection;
        record.checked = None;
        guard.write(&record)?;
        record.review()
    }
    /// Local shape/binding check only; credentials are not loaded and no
    /// upstream operation, conformance probe or canonical admission occurs.
    pub fn check(&self, expected_revision: u64) -> Result<Review> {
        let guard = Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        record.next(expected_revision)?;
        if record.input.connection()? != record.connection {
            return Err(Error::ConnectionChanged);
        }
        record.checked = Some(record.binding()?);
        guard.write(&record)?;
        record.review()
    }
}

#[cfg(unix)]
use rustix::fs::{flock, open, openat, renameat, unlinkat, AtFlags, FlockOperation, Mode, OFlags};
#[cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};
#[cfg(unix)]
pub(super) struct Guard {
    directory: File,
    _lock: ExclusiveLock,
}

/// The parent operation owns this lock, not decoder children launched on other
/// threads. CLOEXEC closes an inherited descriptor only once the child execs;
/// closing the parent's descriptor alone can leave its lock held until then.
#[cfg(unix)]
pub(super) struct ExclusiveLock(File);
#[cfg(unix)]
impl ExclusiveLock {
    pub(super) fn acquire(file: File) -> Result<Self> {
        flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|_| Error::Busy)?;
        Ok(Self(file))
    }
}
#[cfg(unix)]
impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        // Only unlock the open description acquired by this guard. Other
        // owners open independently and retain their own exclusion afterward.
        let _ = flock(&self.0, FlockOperation::Unlock);
    }
}
#[cfg(unix)]
impl Guard {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let directory = File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|_| Error::Protection)?,
        );
        let meta = directory.metadata().map_err(|_| Error::Protection)?;
        if !meta.is_dir()
            || meta.mode() & 0o077 != 0
            || meta.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(Error::Protection);
        }
        let lock = File::from(
            openat(
                &directory,
                "draft.lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| Error::Protection)?,
        );
        Self::protected(&lock, 0)?;
        let lock = ExclusiveLock::acquire(lock)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }
    fn protected(file: &File, max: usize) -> Result<()> {
        let meta = file.metadata().map_err(|_| Error::Protection)?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.mode() & 0o077 != 0
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.len() > max as u64
        {
            return Err(Error::Protection);
        }
        Ok(())
    }
    fn file(&self, name: &str) -> Result<Option<File>> {
        match openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                Self::protected(&file, file_limit(name))?;
                Ok(Some(file))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(_) => Err(Error::Protection),
        }
    }
    pub(super) fn read(&self) -> Result<Option<Record>> {
        let record: Option<Record> = self.read_json("draft.json")?;
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }
    pub(super) fn read_json<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>> {
        let Some(file) = self.file(name)? else {
            return Ok(None);
        };
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        let maximum = file_limit(name);
        file.take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        require(bytes.len() <= maximum)?;
        Ok(Some(
            serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?,
        ))
    }
    pub(super) fn write(&self, record: &Record) -> Result<()> {
        record.validate()?;
        self.write_json("draft.json", "draft.next", record)
    }
    pub(super) fn write_json<T: Serialize>(
        &self,
        name: &str,
        temporary: &str,
        value: &T,
    ) -> Result<()> {
        // Names are fixed or content-addressed by setup callers, never supplied by a declaration.
        let _ = self.file(name)?;
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::Invalid)?);
        require(bytes.len() <= file_limit(name))?;
        // A crash before rename leaves only an uncommitted temporary file. Never
        // promote it on resume; the original durable draft remains authoritative.
        if self.file(temporary)?.is_some() {
            unlinkat(&self.directory, temporary, AtFlags::empty()).map_err(|_| Error::Storage)?;
        }
        let mut file = File::from(
            openat(
                &self.directory,
                temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| Error::Protection)?,
        );
        file.write_all(&bytes).map_err(|_| Error::Storage)?;
        file.sync_all().map_err(|_| Error::Storage)?;
        renameat(&self.directory, temporary, &self.directory, name).map_err(|_| Error::Storage)?;
        self.directory.sync_all().map_err(|_| Error::CommitUnknown)
    }
}

#[cfg(all(test, unix))]
mod lock_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    #[test]
    fn finished_draft_lock_is_not_retained_by_a_child_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let guard = Guard::open(directory.path()).unwrap();
        // A duplicate shares the same kernel lock as a descriptor inherited at
        // fork. Hold it deliberately past exec to make that otherwise brief
        // scheduling window deterministic, without unsafe fork hooks.
        let inherited = guard._lock.0.try_clone().unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read -r release"])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(inherited))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let original_still_held = matches!(Store::open(directory.path()), Err(Error::Busy));
        drop(guard);
        let reopened = Guard::open(directory.path());
        // Reap before assertions, including on the expected failing baseline.
        let released = child.stdin.take().unwrap().write_all(b"done\n");
        let exited = child.wait();
        released.unwrap();
        assert!(exited.unwrap().success());
        assert!(original_still_held);
        assert!(
            reopened.is_ok(),
            "completed draft stayed busy while the child held its inherited descriptor"
        );
        // Child exit must not release the replacement owner's independent lock.
        assert!(matches!(Store::open(directory.path()), Err(Error::Busy)));
        drop(reopened);
        Store::open(directory.path()).unwrap();
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::{error as windows_error, Guard};
#[cfg(not(any(unix, windows)))]
pub(super) struct Guard;
#[cfg(not(any(unix, windows)))]
impl Guard {
    pub(super) fn open(_: &Path) -> Result<Self> {
        Err(Error::Protection)
    }
    pub(super) fn read(&self) -> Result<Option<Record>> {
        Err(Error::Protection)
    }
    pub(super) fn write(&self, _: &Record) -> Result<()> {
        Err(Error::Protection)
    }
    pub(super) fn read_json<T: serde::de::DeserializeOwned>(&self, _: &str) -> Result<Option<T>> {
        Err(Error::Protection)
    }
    pub(super) fn write_json<T: Serialize>(&self, _: &str, _: &str, _: &T) -> Result<()> {
        Err(Error::Protection)
    }
}
