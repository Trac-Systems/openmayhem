use super::*;
use crate::{
    attempts,
    registry::publication::{Limits, Reader, Reference, TrustedOrigin},
    signing::Authority,
};

/// Host configuration only. Never accepted from a dashboard action/upstream response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationRegistry {
    pub origin: String,
    #[serde(default)]
    pub local_loopback_http: bool,
}
impl DeclarationRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self =
            serde_json::from_slice(&private_file(path, 4096).map_err(|_| Error::Protection)?)
                .map_err(|_| Error::Invalid)?;
        value.reader()?;
        Ok(value)
    }
    pub(super) fn reader(&self) -> Result<Reader> {
        let origin = if self.local_loopback_http {
            TrustedOrigin::local_loopback_http(&self.origin)
        } else {
            TrustedOrigin::https(&self.origin)
        }
        .map_err(|_| Error::Invalid)?;
        Reader::new(origin, Limits::default()).map_err(|_| Error::Invalid)
    }
}
pub(in crate::setup) fn now() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Invalid)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::Invalid)
}
impl Flow {
    pub(super) async fn declaration_fields(
        &self,
        release_id: Option<String>,
        cursor: Option<String>,
    ) -> Result<Value> {
        require(cursor.is_none() || release_id.is_some())?;
        let reader = self.registry.as_ref().ok_or(Error::Invalid)?;
        let pin = match release_id {
            Some(id) => reader.pin_release(&id).await,
            None => reader.current().await,
        }
        .map_err(|_| Error::Invalid)?;
        let mut page = reader
            .fields_page(&pin, cursor.as_deref())
            .await
            .map_err(|_| Error::Invalid)?;
        let endpoint = self.view()?.endpoint;
        page.data.retain(|d| {
            matches!(d.definition.usage, crate::registry::Usage::FilterOnly)
                && d.definition.endpoints.contains(&endpoint)
        });
        Ok(json!(page))
    }
    pub(super) async fn declaration_plan(
        &self,
        revision: u64,
        declaration_revision: u64,
        release_id: &str,
        release_hash: &Digest,
        choices: Vec<DeclarationChoice>,
        expires_at_ms: u64,
    ) -> Result<Value> {
        require(
            !choices.is_empty()
                && choices.len() <= 32
                && choices.windows(2).all(|c| c[0].field_id < c[1].field_id),
        )?;
        let reader = self.registry.as_ref().ok_or(Error::Invalid)?;
        let pin = reader
            .pin_release(release_id)
            .await
            .map_err(|_| Error::Invalid)?;
        require(pin.metadata().release_hash == release_hash.as_str())?;
        let references = choices
            .iter()
            .map(|c| Reference {
                field_id: c.field_id.clone(),
                schema_revision: c.schema_revision,
            })
            .collect::<Vec<_>>();
        let definitions = reader
            .lookup_exact(&pin, &references)
            .await
            .map_err(|_| Error::Invalid)?;
        Ok(json!(self.store()?.plan_data_handling(
            revision,
            declaration_revision,
            &definitions,
            choices,
            now()?,
            expires_at_ms
        )?))
    }
    pub(super) fn confirm_declaration(
        &self,
        revision: u64,
        digest: &Digest,
        key: &SigningKey,
    ) -> Result<Value> {
        let network = &self.config.profile.network;
        let authority = Authority::from_unlocked_wallet(
            key.clone(),
            attempts::Identity {
                network_id: network.network_id.clone(),
                msb_bootstrap: Digest::new(&network.msb_bootstrap).map_err(|_| Error::Invalid)?,
                subnet_bootstrap: Digest::new(&network.subnet_bootstrap)
                    .map_err(|_| Error::Invalid)?,
                controller_pubkey: self.provider().clone(),
            },
        )
        .map_err(|_| Error::Invalid)?;
        Ok(json!(self.store()?.confirm_data_handling(
            revision,
            digest,
            &authority,
            now()?
        )?))
    }
}
