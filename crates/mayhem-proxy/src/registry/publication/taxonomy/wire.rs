use super::*;
use crate::{
    discovery::{Identity, Proof},
    registry::required_nullable,
};
use serde::{Deserialize, Serialize};
const SAFE: u64 = 9_007_199_254_740_991;
pub(super) fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
fn revision(n: u32) -> bool {
    n > 0 && n <= i32::MAX as u32
}
fn claim(s: &str, empty: bool) -> bool {
    (empty || !s.is_empty()) && s.len() <= 512 && !s.chars().any(char::is_control)
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub release_id: String,
    pub release_hash: String,
    pub entry_id: String,
    pub schema_revision: u32,
}
impl Reference {
    pub fn validate(&self) -> Result<()> {
        check(
            super::super::wire::release_id(&self.release_id)
                && crate::discovery::hex(&self.release_hash)
                && id(&self.entry_id)
                && revision(self.schema_revision),
            "invalid taxonomy reference",
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReference {
    pub entry_id: String,
    pub schema_revision: u32,
    pub version: u32,
    pub document_hash: String,
}
impl DocumentReference {
    fn validate(&self) -> Result<()> {
        check(
            id(&self.entry_id)
                && revision(self.schema_revision)
                && revision(self.version)
                && self.version >= self.schema_revision
                && crate::discovery::hex(&self.document_hash),
            "invalid taxonomy document reference",
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub family_id: String,
    pub enabled: bool,
    pub label: String,
}
impl Family {
    fn valid(&self) -> bool {
        id(&self.family_id)
            && !self.label.is_empty()
            && self.label.len() <= 128
            && self.label.bytes().all(|b| (32..=126).contains(&b))
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
    #[serde(deserialize_with = "required_nullable")]
    pub network: Option<Identity>,
    pub canonical_families: Vec<Family>,
    pub changes: Vec<DocumentReference>,
}
impl Manifest {
    fn validate(&self) -> Result<()> {
        check(
            self.schema_version == 1
                && (1..=32).contains(&self.changes.len())
                && self.canonical_families.len() <= 32
                && serde_json::to_vec(self)
                    .map_err(|_| Error::Invalid("invalid taxonomy manifest"))?
                    .len()
                    <= 16 * 1024,
            "invalid taxonomy delta bound",
        )?;
        check(
            match (&self.parent_release_id, &self.parent_release_hash) {
                (None, None) => true,
                (Some(id), Some(hash)) => {
                    super::super::wire::release_id(id) && crate::discovery::hex(hash)
                }
                _ => false,
            },
            "invalid taxonomy parent",
        )?;
        check(
            self.changes
                .windows(2)
                .all(|v| v[0].entry_id < v[1].entry_id)
                && self
                    .canonical_families
                    .windows(2)
                    .all(|v| v[0].family_id < v[1].family_id)
                && self
                    .canonical_families
                    .iter()
                    .all(|v| v.enabled && v.valid())
                && (self.canonical_families.is_empty() || self.network.is_some()),
            "invalid taxonomy manifest identity",
        )?;
        if let Some(n) = &self.network {
            n.validate()
                .map_err(|_| Error::Invalid("invalid taxonomy network"))?;
        }
        for c in &self.changes {
            c.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub schema_version: u32,
    pub network: Identity,
    pub snapshot: String,
    pub proof: Proof,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub families: Vec<Family>,
}
impl Observation {
    fn validate(&self) -> Result<()> {
        self.network
            .validate()
            .map_err(|_| Error::Invalid("invalid taxonomy observation network"))?;
        check(
            self.schema_version == 1
                && !self.snapshot.is_empty()
                && self.snapshot.len() <= 2048
                && self.observed_at_ms < self.expires_at_ms
                && self.expires_at_ms <= SAFE
                && self.proof.fork <= SAFE
                && self.proof.signed_length > 0
                && self.proof.signed_length <= SAFE
                && crate::discovery::hex(&self.proof.view_key)
                && crate::discovery::hex(&self.proof.tree_hash)
                && (1..=96).contains(&self.families.len())
                && self.families.iter().all(Family::valid),
            "invalid taxonomy canonical observation",
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub object: String,
    pub release_id: String,
    pub revision: String,
    pub release_hash: String,
    pub manifest: Manifest,
    pub published_at: String,
    pub publication_state: String,
    #[serde(deserialize_with = "required_nullable")]
    pub canonical_observation: Option<Observation>,
}
impl Release {
    pub fn validate(&self) -> Result<()> {
        self.manifest.validate()?;
        let n = self
            .revision
            .parse::<i64>()
            .map_err(|_| Error::Invalid("invalid taxonomy release revision"))?;
        check(
            self.object == "proxy.taxonomy_release"
                && self.publication_state == "published"
                && super::super::wire::release_id(&self.release_id)
                && n > 0
                && n.to_string() == self.revision
                && (n == 1) == self.manifest.parent_release_id.is_none()
                && self.manifest.parent_release_id.as_deref() != Some(&self.release_id)
                && super::super::wire::timestamp(&self.published_at)
                && self.release_hash
                    == digest(
                        "mayhem/proxy/taxonomy-release/v1",
                        &serde_json::to_value(&self.manifest)
                            .map_err(|_| Error::Invalid("invalid taxonomy manifest"))?,
                    )?,
            "invalid taxonomy release",
        )?;
        if let Some(o) = &self.canonical_observation {
            o.validate()?;
            check(
                self.manifest.network.as_ref() == Some(&o.network)
                    && o.families == self.manifest.canonical_families,
                "taxonomy observation differs from manifest",
            )?;
        }
        check(
            self.manifest.canonical_families.is_empty() || self.canonical_observation.is_some(),
            "taxonomy family publication lacks observation",
        )
    }
    pub(super) fn document(&self, d: &DocumentReference) -> Result<()> {
        d.validate()?;
        if let Some(change) = self
            .manifest
            .changes
            .iter()
            .find(|c| c.entry_id == d.entry_id)
        {
            check(
                if d.schema_revision == change.schema_revision {
                    d == change
                } else {
                    d.schema_revision < change.schema_revision && d.version < change.version
                },
                "taxonomy document differs from release delta",
            )?;
        }
        Ok(())
    }
    pub(super) fn summary(&self) -> ReleaseSummary {
        ReleaseSummary {
            release_id: self.release_id.clone(),
            release_hash: self.release_hash.clone(),
            revision: self.revision.clone(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSummary {
    pub release_id: String,
    pub release_hash: String,
    pub revision: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub family_id: String,
    pub model_id: String,
    pub revision: String,
    pub quantization: String,
}
impl Model {
    pub fn validate(&self) -> Result<()> {
        check(
            id(&self.family_id)
                && claim(&self.model_id, false)
                && claim(&self.revision, true)
                && claim(&self.quantization, true),
            "invalid canonical taxonomy model tuple",
        )
    }
    pub fn from_offer(p: &crate::directory::PublishedOffer) -> Self {
        Self {
            family_id: p.market.model.family_id.clone(),
            model_id: p.market.model.model_id.clone(),
            revision: p.market.model.revision.clone(),
            quantization: p.market.model.quantization.clone(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    Family {
        source: DocumentReference,
        family_id: String,
    },
    Model {
        source: DocumentReference,
        family_id: String,
        model_id: String,
        revision: String,
        quantization: String,
    },
}
impl Scope {
    pub fn source(&self) -> &DocumentReference {
        match self {
            Self::Family { source, .. } | Self::Model { source, .. } => source,
        }
    }
    pub fn family_id(&self) -> &str {
        match self {
            Self::Family { family_id, .. } | Self::Model { family_id, .. } => family_id,
        }
    }
    pub fn matches(&self, m: &Model) -> bool {
        match self {
            Self::Family { family_id, .. } => family_id == &m.family_id,
            Self::Model {
                family_id,
                model_id,
                revision,
                quantization,
                ..
            } => {
                family_id == &m.family_id
                    && model_id == &m.model_id
                    && revision == &m.revision
                    && quantization == &m.quantization
            }
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.source().validate()?;
        check(id(self.family_id()), "invalid taxonomy family")?;
        if let Self::Model {
            family_id,
            model_id,
            revision,
            quantization,
            ..
        } = self
        {
            Model {
                family_id: family_id.clone(),
                model_id: model_id.clone(),
                revision: revision.clone(),
                quantization: quantization.clone(),
            }
            .validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub object: String,
    pub schema_version: u32,
    pub release: ReleaseSummary,
    pub category: DocumentReference,
    pub scopes: Vec<Scope>,
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<String>,
    pub scanned_entries: u64,
    pub exhausted: bool,
    pub capacity_reserved: bool,
    pub authorizes_execution: bool,
}
impl Page {
    pub(super) fn validate(
        &self,
        pin: &Pinned,
        reference: &Reference,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<()> {
        check(
            self.object == "proxy.taxonomy_membership_page"
                && self.schema_version == 1
                && self.release == pin.release.summary()
                && self.category.entry_id == reference.entry_id
                && self.category.schema_revision == reference.schema_revision
                && self.scopes.len() <= limit
                && self.scanned_entries <= 256
                && self.scopes.len() as u64 <= self.scanned_entries
                && self.exhausted == self.next_cursor.is_none()
                && !self.capacity_reserved
                && !self.authorizes_execution,
            "invalid taxonomy membership page",
        )?;
        pin.release.document(&self.category)?;
        if let Some(c) = &self.next_cursor {
            check(
                !c.is_empty() && c.len() <= 1024 && Some(c.as_str()) != cursor,
                "invalid taxonomy cursor",
            )?;
        }
        let mut sources = std::collections::BTreeSet::new();
        for scope in &self.scopes {
            scope.validate()?;
            pin.release.document(scope.source())?;
            check(
                sources.insert(scope.source().entry_id.clone()),
                "duplicate taxonomy scope",
            )?;
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Matches {
    pub object: String,
    pub schema_version: u32,
    pub release: ReleaseSummary,
    pub category: DocumentReference,
    pub matches: Vec<Match>,
    pub capacity_reserved: bool,
    pub authorizes_execution: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Match {
    pub model: Model,
    #[serde(deserialize_with = "required_nullable")]
    pub source: Option<DocumentReference>,
    #[serde(deserialize_with = "required_nullable")]
    pub scope: Option<MatchScope>,
}
impl Matches {
    pub(super) fn validate(&self, pin: &Pinned, r: &Reference, models: &[Model]) -> Result<()> {
        check(
            self.object == "proxy.taxonomy_membership_match"
                && self.schema_version == 1
                && self.release == pin.release.summary()
                && self.category.entry_id == r.entry_id
                && self.category.schema_revision == r.schema_revision
                && self.matches.len() == models.len()
                && !self.capacity_reserved
                && !self.authorizes_execution,
            "invalid taxonomy exact match",
        )?;
        pin.release.document(&self.category)?;
        for (m, expected) in self.matches.iter().zip(models) {
            check(&m.model == expected, "taxonomy match reordered model")?;
            match (&m.source, &m.scope) {
                (None, None) => (),
                (Some(source), Some(scope)) => {
                    pin.release.document(source)?;
                    check(scope.matches(&m.model), "taxonomy match scope differs")?;
                }
                _ => return Err(Error::Invalid("partial taxonomy match")),
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum MatchScope {
    Family {
        family_id: String,
    },
    Model {
        family_id: String,
        model_id: String,
        revision: String,
        quantization: String,
    },
}
impl MatchScope {
    fn matches(&self, m: &Model) -> bool {
        match self {
            Self::Family { family_id } => id(family_id) && family_id == &m.family_id,
            Self::Model {
                family_id,
                model_id,
                revision,
                quantization,
            } => {
                family_id == &m.family_id
                    && model_id == &m.model_id
                    && revision == &m.revision
                    && quantization == &m.quantization
            }
        }
    }
}
