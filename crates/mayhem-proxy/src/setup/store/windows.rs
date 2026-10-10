//! The existing setup JSON protocol over owned protected NTFS authority.
use super::*;
use mayhem_windows_sandbox::{LeafName, MutationError, NtfsDirectory, NtfsGuard, PublishMode};
use std::sync::Mutex;

pub(in crate::setup) fn error(error: MutationError) -> Error {
    match error {
        MutationError::Invalid => Error::Invalid,
        MutationError::Busy => Error::Busy,
        MutationError::Conflict => Error::Conflict,
        MutationError::Storage => Error::Storage,
        MutationError::CommitUnknown => Error::CommitUnknown,
        MutationError::Protection | MutationError::UnsupportedFilesystem => Error::Protection,
    }
}
pub(in crate::setup) struct Guard {
    locked: Mutex<NtfsGuard<'static>>,
}
impl Guard {
    pub(in crate::setup) fn open(path: &Path) -> Result<Self> {
        let directory = NtfsDirectory::open_existing(path).map_err(error)?;
        Ok(Self {
            locked: Mutex::new(directory.into_lock().map_err(error)?),
        })
    }
    pub(in crate::setup) fn read(&self) -> Result<Option<Record>> {
        let record: Option<Record> = self.read_json("draft.json")?;
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }
    pub(in crate::setup) fn read_json<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>> {
        let name = LeafName::new(name).map_err(error)?;
        let locked = self.locked.lock().map_err(|_| Error::Protection)?;
        let Some(bytes) = locked
            .read(&name, file_limit(name.as_str()))
            .map_err(error)?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| Error::Invalid)
    }
    pub(in crate::setup) fn write(&self, record: &Record) -> Result<()> {
        record.validate()?;
        self.write_json("draft.json", "draft.next", record)
    }
    pub(in crate::setup) fn write_json<T: Serialize>(
        &self,
        name: &str,
        temporary: &str,
        value: &T,
    ) -> Result<()> {
        let limit = file_limit(name);
        let name = LeafName::new(name).map_err(error)?;
        let temporary = LeafName::new(temporary).map_err(error)?;
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::Invalid)?);
        require(bytes.len() <= limit)?;
        let mut locked = self.locked.lock().map_err(|_| Error::Protection)?;
        if let Some(original) = locked.read(&name, limit).map_err(error)? {
            serde_json::from_slice::<serde_json::Value>(&original).map_err(|_| Error::Invalid)?;
        }
        locked
            .discard_uncommitted(&name, &temporary)
            .map_err(error)?;
        let mut pending = locked.prepare(temporary, &bytes, limit).map_err(error)?;
        pending
            .publish(name, PublishMode::Replace)
            .map(|_| ())
            .map_err(error)
    }
}
