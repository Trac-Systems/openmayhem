//! Administrative category membership from the SAME operator-configured trusted
//! origin as field definitions. Membership never proves a provider capability.
mod wire;
use super::wire::digest;
use super::{check, response_bytes, transport, Cached, Error, Key, Reader, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::time::timeout;
pub use wire::{DocumentReference, Model, Page, Reference, Release, Scope};

const RELEASE_BYTES: usize = 32 * 1024;
const PAGE_BYTES: usize = 256 * 1024;
#[derive(Clone, Debug)]
pub struct Pinned {
    origin: String,
    release: Arc<Release>,
}
impl Pinned {
    pub fn metadata(&self) -> &Release {
        &self.release
    }
    pub fn check_network(&self, n: &crate::discovery::Identity) -> Result<()> {
        check(
            self.release
                .manifest
                .network
                .as_ref()
                .is_none_or(|r| r == n),
            "taxonomy release network differs",
        )
    }
}
/// Constructed only by validated exact lookup, not by a caller's JSON. Its scope
/// attests to administrative membership only, never admission or availability.
#[derive(Clone, Debug)]
pub struct Membership {
    reference: Reference,
    model: Model,
    matched: bool,
}
impl Membership {
    pub fn contains(&self, r: &Reference, m: &Model) -> bool {
        &self.reference == r && &self.model == m && self.matched
    }
}
fn etag(release: &Release, kind: &str, selector: Value) -> Result<String> {
    Ok(format!(
        "\"{}\"",
        digest(
            "mayhem/proxy/taxonomy-representation/v1",
            &json!({"release_id":release.release_id,"release_hash":release.release_hash,"kind":kind,"selector":selector})
        )?
    ))
}
impl Reader {
    pub async fn pin_taxonomy(&self, reference: &Reference) -> Result<Pinned> {
        reference.validate()?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let release = match self
                .lock()?
                .entries
                .get(&Key::TaxonomyRelease(reference.release_id.clone()))
            {
                Some((Cached::TaxonomyRelease(r), _)) => Some(r.clone()),
                _ => None,
            };
            let release = if let Some(r) = release {
                r
            } else {
                let (r, tag) = self
                    .taxonomy_read::<Release>(&reference.release_id, None, None, RELEASE_BYTES)
                    .await?;
                r.validate()?;
                check(
                    r.release_id == reference.release_id
                        && tag == etag(&r, "release", Value::Null)?,
                    "taxonomy release identity or ETag differs",
                )?;
                let bytes = serde_json::to_vec(&r)
                    .map_err(|_| Error::Invalid("invalid taxonomy release"))?
                    .len();
                let r = Arc::new(r);
                self.lock()?.insert(
                    Key::TaxonomyRelease(r.release_id.clone()),
                    Cached::TaxonomyRelease(r.clone()),
                    bytes,
                    &self.limits,
                )?;
                r
            };
            check(
                release.release_hash == reference.release_hash,
                "taxonomy release hash differs",
            )?;
            Ok(Pinned {
                origin: self.origin.as_str().into(),
                release,
            })
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    fn taxonomy_pin(&self, pin: &Pinned, r: &Reference) -> Result<()> {
        r.validate()?;
        check(
            pin.origin == self.origin.as_str()
                && pin.release.release_id == r.release_id
                && pin.release.release_hash == r.release_hash,
            "taxonomy pin differs",
        )
    }
    pub async fn taxonomy_members(
        &self,
        pin: &Pinned,
        r: &Reference,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page> {
        self.taxonomy_pin(pin, r)?;
        check(
            (1..=100).contains(&limit) && cursor.is_none_or(|c| !c.is_empty() && c.len() <= 1024),
            "invalid taxonomy page query",
        )?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout,async{
   let mut query=vec![("entry_id",r.entry_id.clone()),("schema_revision",r.schema_revision.to_string()),("limit",limit.to_string())];if let Some(c)=cursor{query.push(("cursor",c.into()));}
   let (page,tag)=self.taxonomy_read::<Page>(&format!("{}/members",r.release_id),Some(&query),None,PAGE_BYTES).await?;
   page.validate(pin,r,limit,cursor)?;
   check(tag==etag(&pin.release,"members",json!({"entry_id":r.entry_id,"schema_revision":r.schema_revision,"limit":limit,"cursor":cursor}))?,"taxonomy members ETag differs")?;
   Ok(page)
  }).await.map_err(|_|Error::Deadline)?
    }
    pub async fn taxonomy_match(
        &self,
        pin: &Pinned,
        r: &Reference,
        models: &[Model],
    ) -> Result<Vec<Membership>> {
        self.taxonomy_pin(pin, r)?;
        check(
            (1..=32).contains(&models.len()),
            "taxonomy exact lookup requires 1..32 models",
        )?;
        let mut unique = std::collections::BTreeSet::new();
        let mut hashes = Vec::new();
        for m in models {
            m.validate()?;
            let hash = digest("mayhem/proxy/taxonomy-match-model/v1", &json!(m))?;
            check(unique.insert(hash.clone()), "duplicate taxonomy model")?;
            hashes.push(hash);
        }
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout,async{
   let body=json!({"entry_id":r.entry_id,"schema_revision":r.schema_revision,"models":models});
   let (answer,tag)=self.taxonomy_read::<wire::Matches>(&format!("{}/match",r.release_id),None,Some(&body),PAGE_BYTES).await?;
   answer.validate(pin,r,models)?;
   check(tag==etag(&pin.release,"match",json!({"entry_id":r.entry_id,"schema_revision":r.schema_revision,"models":hashes}))?,"taxonomy match ETag differs")?;
   Ok(answer.matches.into_iter().map(|m|Membership{reference:r.clone(),model:m.model,matched:m.scope.is_some()}).collect())
  }).await.map_err(|_|Error::Deadline)?
    }
    async fn taxonomy_read<T: DeserializeOwned>(
        &self,
        path: &str,
        query: Option<&Vec<(&str, String)>>,
        body: Option<&Value>,
        max: usize,
    ) -> Result<(T, String)> {
        let mut url = self
            .origin
            .0
            .join(&format!("v1/proxy/taxonomy/releases/{path}"))
            .map_err(|_| Error::Invalid("invalid fixed taxonomy route"))?;
        if let Some(query) = query {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v)));
        }
        let req = if let Some(b) = body {
            self.http.post(url).json(b)
        } else {
            self.http.get(url)
        };
        let response = req
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Http(response.status().as_u16()));
        }
        check(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"))
                })
                && response
                    .headers()
                    .get(reqwest::header::CONTENT_ENCODING)
                    .is_none_or(|v| v == "identity"),
            "invalid taxonomy content type or encoding",
        )?;
        let tag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() == 66)
            .ok_or(Error::Invalid("missing taxonomy ETag"))?
            .to_owned();
        let bytes = response_bytes(response, max).await?;
        Ok((
            serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("invalid taxonomy JSON"))?,
            tag,
        ))
    }
}
