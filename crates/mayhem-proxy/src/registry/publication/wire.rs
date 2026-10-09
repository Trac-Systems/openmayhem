use super::{check, Error, Result};
use crate::registry::{identifier, required_nullable, Definition};
use mayhem_proto::stable_json_bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const MAX_DELTA: usize = 32;
pub const MAX_RELEASE_BYTES: usize = 32 * 1024;
pub const DOCUMENT_OVERHEAD: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub field_id: String,
    pub schema_revision: u32,
}
impl Reference {
    pub fn validate(&self) -> Result<()> {
        check(
            identifier(&self.field_id) && revision(self.schema_revision),
            "invalid registry reference",
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReference {
    pub field_id: String,
    pub schema_revision: u32,
    pub version: u32,
    pub definition_hash: String,
}
impl DocumentReference {
    fn validate(&self) -> Result<()> {
        check(
            identifier(&self.field_id)
                && revision(self.schema_revision)
                && revision(self.version)
                && self.version >= self.schema_revision
                && hash(&self.definition_hash),
            "invalid published document reference",
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(deserialize_with = "required_nullable")]
    pub parent_release_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub parent_release_hash: Option<String>,
    pub changes: Vec<DocumentReference>,
}
impl Manifest {
    pub fn digest(&self) -> Result<String> {
        check(
            self.schema_version == 1
                && !self.changes.is_empty()
                && self.changes.len() <= MAX_DELTA
                && self
                    .changes
                    .windows(2)
                    .all(|p| p[0].field_id < p[1].field_id),
            "invalid registry release delta",
        )?;
        check(
            match (&self.parent_release_id, &self.parent_release_hash) {
                (None, None) => true,
                (Some(id), Some(digest)) => release_id(id) && hash(digest),
                _ => false,
            },
            "invalid registry release parent",
        )?;
        for change in &self.changes {
            change.validate()?;
        }
        digest(
            "mayhem/proxy/registry-release/v1",
            &serde_json::to_value(self).map_err(|_| Error::Invalid("invalid release JSON"))?,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub object: String,
    pub release_id: String,
    /// PostgreSQL signed BIGINT encoded as a canonical positive decimal string.
    pub revision: String,
    pub release_hash: String,
    pub manifest: Manifest,
    pub published_at: String,
    pub publication_state: String,
}
impl Release {
    pub fn validate(&self) -> Result<()> {
        check(
            self.object == "proxy.registry_release"
                && self.publication_state == "published"
                && release_id(&self.release_id)
                && hash(&self.release_hash)
                && timestamp(&self.published_at),
            "invalid registry release identity",
        )?;
        let revision = self.sequence()?;
        check(
            (revision == 1) == self.manifest.parent_release_id.is_none()
                && (revision != 1
                    || self
                        .manifest
                        .changes
                        .iter()
                        .all(|change| change.schema_revision == 1))
                && self.manifest.parent_release_id.as_deref() != Some(&self.release_id)
                && self.manifest.digest()? == self.release_hash,
            "registry release digest or parent differs",
        )?;
        check(
            serde_json::to_vec(self)
                .map_err(|_| Error::Invalid("invalid release JSON"))?
                .len()
                <= MAX_RELEASE_BYTES,
            "registry release exceeds byte bound",
        )
    }
    pub fn sequence(&self) -> Result<u64> {
        check(
            !self.revision.is_empty()
                && self.revision.len() <= 19
                && !self.revision.starts_with('0')
                && self.revision.bytes().all(|c| c.is_ascii_digit()),
            "invalid registry release sequence",
        )?;
        let value = self
            .revision
            .parse::<u64>()
            .map_err(|_| Error::Invalid("invalid registry release sequence"))?;
        check(
            value > 0 && value <= i64::MAX as u64,
            "invalid registry release sequence",
        )?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub field_id: String,
    pub schema_revision: u32,
    pub version: u32,
    pub definition_hash: String,
    pub definition: Definition,
}
impl Document {
    pub fn reference(&self) -> Reference {
        Reference {
            field_id: self.field_id.clone(),
            schema_revision: self.schema_revision,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.reference().validate()?;
        check(
            revision(self.version)
                && self.version >= self.schema_revision
                && hash(&self.definition_hash)
                && self.definition.field_id == self.field_id
                && self.definition.schema_revision == self.schema_revision,
            "registry document identity differs",
        )?;
        let digest = self
            .definition
            .digest()
            .map_err(|_| Error::Invalid("invalid registry definition"))?;
        check(
            digest == self.definition_hash,
            "registry definition digest differs",
        )
    }
    pub(crate) fn bytes(&self) -> Result<usize> {
        Ok(serde_json::to_vec(self)
            .map_err(|_| Error::Invalid("invalid document JSON"))?
            .len())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lookup {
    pub object: String,
    pub release_id: String,
    pub release_hash: String,
    pub definitions: Vec<Document>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Failure {
    pub error: FailureDetail,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FailureDetail {
    #[serde(rename = "statusCode")]
    pub status: u16,
    pub message: String,
    pub code: String,
}
impl Lookup {
    pub(crate) fn validate(&self, release: &Release, refs: &[Reference]) -> Result<()> {
        check(
            self.object == "proxy.registry_lookup"
                && self.release_id == release.release_id
                && self.release_hash == release.release_hash
                && self.definitions.len() == refs.len(),
            "registry lookup release or cardinality differs",
        )?;
        for (document, reference) in self.definitions.iter().zip(refs) {
            document.validate()?;
            check(
                document.reference() == *reference,
                "registry lookup order or reference differs",
            )?;
            // For delta fields this release also directly commits the selected
            // latest document. Older exact semantic revisions remain valid.
            let change = release
                .manifest
                .changes
                .iter()
                .find(|c| c.field_id == document.field_id);
            check(
                release.sequence()? > 1 || change.is_some(),
                "first release cannot inherit an unpublished field",
            )?;
            if let Some(change) = change {
                check(
                    document.schema_revision <= change.schema_revision
                        && document.version <= change.version,
                    "registry lookup returned a future document",
                )?;
                if document.schema_revision == change.schema_revision {
                    check(
                        document.version == change.version
                            && document.definition_hash == change.definition_hash,
                        "registry lookup does not match its release delta",
                    )?;
                } else {
                    check(
                        document.version < change.version,
                        "historical semantic revision reused a newer document version",
                    )?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_references(refs: &[Reference]) -> Result<()> {
    check(
        !refs.is_empty() && refs.len() <= crate::registry::MAX_FIELDS_PER_REQUEST,
        "registry lookup requires 1..96 references",
    )?;
    let mut seen = BTreeSet::new();
    for reference in refs {
        reference.validate()?;
        check(
            seen.insert(reference),
            "duplicate registry lookup reference",
        )?;
    }
    Ok(())
}
pub(crate) fn etag(release: &Release, references: Option<&[Reference]>) -> Result<String> {
    let (kind, selector) = if let Some(refs) = references {
        let hashes: Result<Vec<_>> = refs
            .iter()
            .map(|r| digest("mayhem/proxy/registry-reference/v1", &json!(r)))
            .collect();
        ("lookup", json!(hashes?))
    } else {
        ("release", Value::Null)
    };
    Ok(format!(
        "\"{}\"",
        digest(
            "mayhem/proxy/registry-representation/v1",
            &json!({
                "release_id": release.release_id, "release_hash": release.release_hash, "kind": kind, "selector": selector
            })
        )?
    ))
}
pub(super) fn digest(domain: &str, value: &Value) -> Result<String> {
    let canonical =
        stable_json_bytes(value).map_err(|_| Error::Invalid("invalid registry canonical JSON"))?;
    check(
        canonical.len() <= 16 * 1024,
        "registry digest envelope exceeds byte bound",
    )?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(&canonical);
    Ok(hasher.finalize().to_hex().to_string())
}
fn revision(value: u32) -> bool {
    value > 0 && value <= i32::MAX as u32
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn release_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
        && matches!(value.as_bytes()[14], b'1'..=b'8')
        && matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b')
}
pub(super) fn timestamp(value: &str) -> bool {
    // SITE emits Date.toISOString: bounded UTC calendar time with milliseconds.
    if value.len() != 24
        || !value.bytes().enumerate().all(|(i, c)| match i {
            4 | 7 => c == b'-',
            10 => c == b'T',
            13 | 16 => c == b':',
            19 => c == b'.',
            23 => c == b'Z',
            _ => c.is_ascii_digit(),
        })
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<usize>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        0,
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year > 0
        && (1..=12).contains(&month)
        && day > 0
        && day <= days[month]
        && value[11..13].parse::<u32>().unwrap_or(99) < 24
        && value[14..16].parse::<u32>().unwrap_or(99) < 60
        && value[17..19].parse::<u32>().unwrap_or(99) < 60
}
