//! Authenticated immutable metadata evidence, never an inference authorization.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionReference {
    pub entry_id: String,
    pub schema_revision: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filters {
    pub release_id: String,
    pub release_hash: String,
    pub variants: Vec<SelectionReference>,
    pub tags: Vec<SelectionReference>,
}
impl Filters {
    pub fn validate(&self) -> Result<()> {
        check(
            super::super::wire::release_id(&self.release_id)
                && crate::discovery::hex(&self.release_hash)
                && (!self.variants.is_empty() || !self.tags.is_empty()),
            "invalid taxonomy filter release",
        )?;
        for refs in [&self.variants, &self.tags] {
            check(
                refs.len() <= 64
                    && refs.windows(2).all(|v| v[0].entry_id < v[1].entry_id)
                    && refs.iter().all(|r| {
                        wire::id(&r.entry_id)
                            && r.schema_revision > 0
                            && r.schema_revision <= i32::MAX as u32
                    }),
                "invalid taxonomy selection references",
            )?;
        }
        Ok(())
    }
}
/// Cannot be deserialized or constructed from caller JSON. Issued only by the
/// trusted origin reader after exact release, filters, tuple and shape checks.
#[derive(Clone, Debug)]
pub struct Selection {
    filters: Filters,
    matches: Vec<(Model, bool)>,
}
impl Selection {
    pub fn allows(&self, filters: &Filters, model: &Model) -> bool {
        &self.filters == filters && self.matches.iter().any(|(m, yes)| m == model && *yes)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    object: String,
    schema_version: u32,
    release: wire::ReleaseSummary,
    variants: Vec<DocumentReference>,
    tags: Vec<DocumentReference>,
    matches: Vec<Match>,
    capacity_reserved: bool,
    authorizes_execution: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    model: Model,
    #[serde(deserialize_with = "crate::registry::required_nullable")]
    variant: Option<DocumentReference>,
    tags: Vec<Tag>,
    matches: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tag {
    #[serde(deserialize_with = "crate::registry::required_nullable")]
    source: Option<DocumentReference>,
    #[serde(deserialize_with = "crate::registry::required_nullable")]
    scope: Option<ScopeKind>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopeKind {
    Model,
    Family,
}
impl Reply {
    fn validate(&self, pin: &Pinned, filters: &Filters, models: &[Model]) -> Result<()> {
        check(
            self.object == "proxy.taxonomy_selection"
                && self.schema_version == 1
                && self.release == pin.release.summary()
                && !self.capacity_reserved
                && !self.authorizes_execution
                && self.matches.len() == models.len(),
            "invalid taxonomy selection envelope",
        )?;
        for (received, required) in [
            (&self.variants, &filters.variants),
            (&self.tags, &filters.tags),
        ] {
            check(
                received.len() == required.len(),
                "taxonomy selection reference count differs",
            )?;
            for (doc, r) in received.iter().zip(required) {
                pin.release.document(doc)?;
                check(
                    doc.entry_id == r.entry_id && doc.schema_revision == r.schema_revision,
                    "taxonomy selection reference differs",
                )?;
            }
        }
        for (answer, model) in self.matches.iter().zip(models) {
            check(
                &answer.model == model && answer.tags.len() == self.tags.len(),
                "taxonomy selection tuple or tag count differs",
            )?;
            if let Some(variant) = &answer.variant {
                check(
                    self.variants.contains(variant),
                    "taxonomy selection variant differs",
                )?;
            }
            for tag in &answer.tags {
                check(
                    tag.source.is_some() == tag.scope.is_some(),
                    "partial taxonomy tag evidence",
                )?;
                if let Some(source) = &tag.source {
                    pin.release.document(source)?;
                }
            }
            check(
                answer.matches
                    == ((self.variants.is_empty() || answer.variant.is_some())
                        && answer.tags.iter().all(|t| t.source.is_some())),
                "taxonomy selection decision differs",
            )?;
        }
        Ok(())
    }
}
impl Reader {
    /// Pin using an actual explicit semantic reference, without inventing a
    /// category or looking up a display alias. Selection validates its kind.
    pub async fn pin_taxonomy_filters(&self, filters: &Filters) -> Result<Pinned> {
        filters.validate()?;
        let first = filters
            .variants
            .first()
            .or_else(|| filters.tags.first())
            .ok_or(Error::Invalid("empty taxonomy filters"))?;
        self.pin_taxonomy(&Reference {
            release_id: filters.release_id.clone(),
            release_hash: filters.release_hash.clone(),
            entry_id: first.entry_id.clone(),
            schema_revision: first.schema_revision,
        })
        .await
    }
    pub async fn taxonomy_selection(
        &self,
        pin: &Pinned,
        filters: &Filters,
        models: &[Model],
    ) -> Result<Selection> {
        filters.validate()?;
        check(
            pin.origin == self.origin.as_str()
                && pin.release.release_id == filters.release_id
                && pin.release.release_hash == filters.release_hash
                && (1..=32).contains(&models.len()),
            "taxonomy selection origin or release differs",
        )?;
        let mut hashes = Vec::new();
        let mut unique = std::collections::BTreeSet::new();
        for model in models {
            model.validate()?;
            let hash = digest("mayhem/proxy/taxonomy-match-model/v1", &json!(model))?;
            check(
                unique.insert(hash.clone()),
                "duplicate taxonomy selection model",
            )?;
            hashes.push(hash);
        }
        let body = json!({"release_hash": filters.release_hash,"variants": filters.variants,"tags":filters.tags,"models":models});
        check(
            serde_json::to_vec(&body)
                .map_err(|_| Error::Invalid("invalid taxonomy selection body"))?
                .len()
                <= 65536,
            "taxonomy selection body exceeds limit",
        )?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let (answer, tag) = self
                .taxonomy_read::<Reply>(
                    &format!("{}/selection", filters.release_id),
                    None,
                    Some(&body),
                    1024 * 1024,
                )
                .await?;
            answer.validate(pin, filters, models)?;
            check(
                tag == etag(
                    &pin.release,
                    "selection",
                    json!({"variants":filters.variants,"tags":filters.tags,"models":hashes}),
                )?,
                "taxonomy selection ETag differs",
            )?;
            Ok(Selection {
                filters: filters.clone(),
                matches: answer
                    .matches
                    .into_iter()
                    .map(|m| (m.model, m.matches))
                    .collect(),
            })
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
}
