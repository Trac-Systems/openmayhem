//! Explicit asynchronous job capabilities, not retry or settlement authority.
use super::{check, transform, Error, Result};
use crate::{
    attempts::{Digest, Record, RemoteId},
    connector::config::Operation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub submit_status: u16,
    pub job_id_path: Vec<String>,
    pub poll_interval_ms: u64,
    pub control_timeout_ms: u64,
    pub status_path: Vec<String>,
    pub statuses: BTreeMap<String, Status>,
    pub poll: Control,
    pub result: Control,
    pub cancel: Option<Cancel>,
    pub lookup: Option<Lookup>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub job_id_field: Vec<String>,
    /// Optional explicit echo. Without it, binding is the exact authenticated
    /// HTTP request to the locally fixed operation, never a caller-supplied URL.
    pub response_id_path: Option<Vec<String>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub control: Control,
    pub idempotent: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    pub submit_key_field: Vec<String>,
    pub lookup_key_field: Vec<String>,
    pub response_key_path: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Ready,
    Cancelled,
    Failed,
    Missing,
}
impl Control {
    fn validate(&self) -> Result<()> {
        transform::path(&self.job_id_field)?;
        check(!self.job_id_field.is_empty())?;
        if let Some(path) = &self.response_id_path {
            transform::path(path)?;
            check(!path.is_empty())?;
        }
        Ok(())
    }
    pub(crate) fn body(&self, id: &RemoteId) -> Result<Vec<u8>> {
        let mut value = Value::Object(Map::new());
        insert(
            &mut value,
            &self.job_id_field,
            Value::String(id.as_str().into()),
        )?;
        serde_json::to_vec(&value).map_err(|_| Error)
    }
    pub(crate) fn check_response(&self, value: &Value, id: &RemoteId) -> Result<()> {
        if let Some(path) = &self.response_id_path {
            check(transform::read(value, path).and_then(Value::as_str) == Some(id.as_str()))?;
        }
        Ok(())
    }
}
pub(crate) fn insert(target: &mut Value, path: &[String], value: Value) -> Result<()> {
    let (first, rest) = path.split_first().ok_or(Error)?;
    let map = target.as_object_mut().ok_or(Error)?;
    if rest.is_empty() {
        check(!map.contains_key(first))?;
        map.insert(first.clone(), value);
        Ok(())
    } else {
        insert(
            map.entry(first)
                .or_insert_with(|| Value::Object(Map::new())),
            rest,
            value,
        )
    }
}
impl Job {
    pub(super) fn validate(&self) -> Result<()> {
        check(
            matches!(self.submit_status, 200 | 201 | 202)
                && (1000..=3_600_000).contains(&self.poll_interval_ms)
                && (100..=60_000).contains(&self.control_timeout_ms),
        )?;
        for path in [&self.job_id_path, &self.status_path] {
            transform::path(path)?;
            check(!path.is_empty())?;
        }
        check(
            (1..=16).contains(&self.statuses.len())
                && self.statuses.keys().all(|s| transform::key(s))
                && self.statuses.values().any(|v| *v == Status::Ready),
        )?;
        self.poll.validate()?;
        self.result.validate()?;
        if let Some(cancel) = &self.cancel {
            cancel.control.validate()?;
        }
        if let Some(lookup) = &self.lookup {
            for path in [
                &lookup.submit_key_field,
                &lookup.lookup_key_field,
                &lookup.response_key_path,
            ] {
                transform::path(path)?;
                check(!path.is_empty())?;
            }
        }
        Ok(())
    }
    pub(crate) fn operations(&self) -> Vec<Operation> {
        let mut ops = vec![Operation::JobPoll, Operation::JobResult];
        if self.cancel.is_some() {
            ops.push(Operation::JobCancel);
        }
        if self.lookup.is_some() {
            ops.push(Operation::JobLookup);
        }
        ops
    }
    pub(crate) fn key(&self, record: &Record) -> Digest {
        Digest::hash(
            "mayhem/proxy/upstream-job-key/v1",
            &[
                record.invocation.as_str().as_bytes(),
                &record.attempt.to_le_bytes(),
                record.binding.request_hash.as_str().as_bytes(),
                record.binding.recipe_digest.as_str().as_bytes(),
                record.binding.connection_digest.as_str().as_bytes(),
            ],
        )
    }
    pub(crate) fn submit_body(&self, body: &[u8], record: &Record, max: usize) -> Result<Vec<u8>> {
        self.submit_body_with_key(body, &self.key(record), max)
    }
    pub(crate) fn submit_body_with_key(
        &self,
        body: &[u8],
        key: &Digest,
        max: usize,
    ) -> Result<Vec<u8>> {
        let mut value: Value = serde_json::from_slice(body).map_err(|_| Error)?;
        if let Some(lookup) = &self.lookup {
            insert(
                &mut value,
                &lookup.submit_key_field,
                Value::String(key.as_str().into()),
            )?;
        }
        transform::bounded(&value, max)?;
        serde_json::to_vec(&value).map_err(|_| Error)
    }
    pub(crate) fn lookup_body(&self, record: &Record) -> Result<Vec<u8>> {
        let lookup = self.lookup.as_ref().ok_or(Error)?;
        let mut value = Value::Object(Map::new());
        insert(
            &mut value,
            &lookup.lookup_key_field,
            Value::String(self.key(record).as_str().into()),
        )?;
        serde_json::to_vec(&value).map_err(|_| Error)
    }
    pub(crate) fn check_lookup(&self, value: &Value, record: &Record) -> Result<()> {
        let lookup = self.lookup.as_ref().ok_or(Error)?;
        check(
            transform::read(value, &lookup.response_key_path).and_then(Value::as_str)
                == Some(self.key(record).as_str()),
        )
    }
    pub(crate) fn id(&self, value: &Value) -> Result<RemoteId> {
        RemoteId::new(
            transform::read(value, &self.job_id_path)
                .and_then(Value::as_str)
                .ok_or(Error)?,
        )
        .map_err(|_| Error)
    }
    pub(crate) fn status(&self, value: &Value) -> Result<Status> {
        self.statuses
            .get(
                transform::read(value, &self.status_path)
                    .and_then(Value::as_str)
                    .ok_or(Error)?,
            )
            .copied()
            .ok_or(Error)
    }
}
