//! A single bounded page, pinned to an immutable publication. Never walk history.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldsPage {
    pub object: String,
    pub release_id: String,
    pub release_hash: String,
    pub data: Vec<Document>,
    pub next_cursor: Option<String>,
}
impl Reader {
    pub async fn fields_page(
        &self,
        pin: &PinnedRelease,
        cursor: Option<&str>,
    ) -> Result<FieldsPage> {
        self.check_pin(pin)?;
        check(
            cursor.is_none_or(|s| {
                !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
            }),
            "invalid registry cursor",
        )?;
        let _permit = self.slots.try_acquire().map_err(|_| Error::Busy)?;
        timeout(self.limits.operation_timeout, async {
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("limit", "32");
                if let Some(cursor) = cursor { query.append_pair("cursor", cursor); }
                format!("{}/fields?{}", pin.metadata().release_id, query.finish())
            };
            let (page, etag) = self.read::<FieldsPage>(&path, None, 32 * (MAX_DEFINITION_BYTES + wire::DOCUMENT_OVERHEAD) + 4096).await?;
            let expected = format!("\"{}\"", wire::digest("mayhem/proxy/registry-representation/v1", &json!({
                "release_id":pin.metadata().release_id,"release_hash":pin.metadata().release_hash,
                "kind":"fields","selector":{"limit":32,"cursor":cursor}
            }))?);
            check(page.object == "list" && page.release_id == pin.metadata().release_id
                && page.release_hash == pin.metadata().release_hash && etag == expected
                && page.data.len() <= 32
                && page.next_cursor.as_deref().is_none_or(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control) && Some(s) != cursor), "registry page differs")?;
            let mut seen = BTreeSet::new();
            for doc in &page.data {
                doc.validate()?;
                check(seen.insert(&doc.field_id), "duplicate registry field")?;
            }
            let mut cache = self.lock()?;
            for doc in &page.data {
                let key = Key::Document(pin.metadata().release_id.clone(),pin.metadata().release_hash.clone(),doc.reference());
                if let Some((Cached::Document(known),_)) = cache.entries.get(&key) {
                    if known.as_ref() != doc { return Err(Error::Equivocation); }
                }
            }
            for doc in &page.data {
                let key = Key::Document(pin.metadata().release_id.clone(),pin.metadata().release_hash.clone(),doc.reference());
                cache.insert(key,Cached::Document(Arc::new(doc.clone())),doc.bytes()?,&self.limits)?;
            }
            Ok(page)
        }).await.map_err(|_| Error::Deadline)?
    }
}
