//! Temporary discovery credentials remain private and are removed before use.
use super::*;
use mayhem_windows_sandbox::{LeafName, NtfsDirectory, PublishMode};

pub fn models_connection(
    parent: &Path,
    base_url: String,
    network: NetworkPolicy,
    credential: Credential,
) -> Result<crate::connector::http::HttpConnection> {
    let mut config = ConnectionConfig {
        schema_version: 1,
        id: "setup_preview".into(),
        revision: 1,
        base_url,
        network,
        paths: BTreeMap::from([(Operation::Models, "models".into())]),
        authentication: Authentication::None,
        headers: BTreeMap::new(),
        error_profile: ErrorProfile::OpenAi,
        limits: crate::connector::config::Limits {
            max_in_flight: 1,
            max_response_bytes: 64 * 1024,
            connect_timeout_ms: 5_000,
            read_idle_timeout_ms: Some(5_000),
            ..Default::default()
        },
    };
    config.validate().map_err(|_| Error::Invalid)?;
    let value = match credential {
        Credential::None => None,
        Credential::BearerFile(path) => {
            require(path.is_absolute())?;
            config.authentication = Authentication::Bearer { secret: SecretSource::File { path } };
            None
        }
        Credential::BearerValue(value) => Some(value),
    };
    let Some(value) = value else {
        return crate::connector::http::HttpConnection::new(config).map_err(|_| Error::Protection);
    };
    secret_valid(&value)?;
    let mut guard = NtfsDirectory::open_existing(parent)
        .and_then(NtfsDirectory::into_lock)
        .map_err(store::windows_error)?;
    let parent = fs::canonicalize(parent).map_err(|_| Error::Protection)?;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
    let stem = format!(".proxy-models-{}", blake3::hash(&nonce).to_hex());
    let name = LeafName::new(&stem).map_err(store::windows_error)?;
    let temporary = LeafName::new(&format!("{stem}.next")).map_err(store::windows_error)?;
    // Absent sentinel is never created. Cleanup validates that the scratch file
    // is the distinct unpublished credential, not a retained setup record.
    let absent = LeafName::new(&format!("{stem}.absent")).map_err(store::windows_error)?;
    {
        let mut pending = guard.prepare(temporary, &value, 8192).map_err(store::windows_error)?;
        pending.publish(name.clone(), PublishMode::CreateNew).map_err(store::windows_error)?;
    }
    drop(value);
    config.authentication = Authentication::Bearer {
        secret: SecretSource::File { path: parent.join(name.as_str()) },
    };
    // The existing connector loads a sensitive header once. No network is
    // dispatched here. Cleanup must succeed before a usable client is returned,
    // including when header construction fails. An ambiguous mutation is never
    // retried or followed by broad directory cleanup.
    let result = crate::connector::http::HttpConnection::new(config).map_err(|_| Error::Protection);
    guard.discard_uncommitted(&absent, &name).map_err(store::windows_error)?;
    result
}
