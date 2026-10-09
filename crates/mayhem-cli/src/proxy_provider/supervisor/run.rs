//! Host-only wizard handoff. The browser supplies an acknowledged plan hash,
//! never a wallet locator, supervisor destination, child argv or credential.
use super::*;
use mayhem_proxy::{
    attempts::{Digest, Identity},
    setup::{self, LaunchBinding, LifecycleObservation, RunFuture, RunLifecycle},
};

pub struct Host {
    home: PathBuf,
    keypair: PathBuf,
    password: Option<PathBuf>,
    binary: PathBuf,
    origin: String,
    provider: Digest,
    client: reqwest::Client,
}
impl Host {
    pub fn new(
        home: PathBuf,
        keypair: PathBuf,
        password: Option<PathBuf>,
        provider: Digest,
    ) -> Result<Self> {
        let home = cli::absolutize(home)?;
        let keypair = std::fs::canonicalize(keypair)?;
        let password = password.map(cli::absolutize).transpose()?.or_else(|| {
            let path = home.join("secrets/wallet-password");
            path.exists().then_some(path)
        });
        let origin = cli::mayhemd_control_url(&home)?;
        ensure_loopback(&origin)?;
        Ok(Self {
            home,
            keypair,
            password,
            binary: std::env::current_exe()?.canonicalize()?,
            origin,
            provider,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }
    fn child(&self, name: &str, path: &Path, digest: &Digest) -> Result<Value> {
        let mut child = child_config(
            name,
            &self.binary,
            path,
            &self.home,
            &self.keypair,
            self.password.as_deref(),
        )?;
        child["args"]
            .as_array_mut()
            .context("missing child arguments")?
            .extend([json!("--expected-config-digest"), json!(digest)]);
        // Canonical complete ChildConfig defaults, as persisted by mayhemd.
        child["cwd"] = Value::Null;
        child["startup_probes"] = json!([]);
        Ok(child)
    }
    async fn post(&self, route: &str, body: &Value) -> setup::Result<Value> {
        let token = cli::load_or_create_mayhemd_control_token(&self.home)
            .map_err(|_| setup::Error::Protection)?;
        let mut response = self
            .client
            .post(format!("{}{route}", self.origin.trim_end_matches('/')))
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .map_err(|_| setup::Error::RunUnavailable)?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 65536) {
            return Err(setup::Error::RunUnavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| setup::Error::RunUnavailable)?
        {
            if bytes.len() + chunk.len() > 65536 {
                return Err(setup::Error::RunUnavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| setup::Error::RunUnavailable)
    }
}
fn hash_child(value: &Value) -> Result<Digest> {
    let mut value = value.clone();
    value
        .as_object_mut()
        .context("child object")?
        .remove("persistent");
    let mut bytes = b"mayhem/supervisor/child-config/v1\0".to_vec();
    bytes.extend(mayhem_proto::stable_json_bytes(&value)?);
    Ok(Digest::new(blake3::hash(&bytes).to_hex().to_string())?)
}
impl RunLifecycle for Host {
    fn binding(
        &self,
        identity: &Identity,
        path: &Path,
        digest: &Digest,
    ) -> setup::Result<LaunchBinding> {
        if identity.controller_pubkey != self.provider {
            return Err(setup::Error::RunConflict);
        }
        let name = child_name(identity).map_err(|_| setup::Error::Invalid)?;
        let child = self
            .child(&name, path, digest)
            .map_err(|_| setup::Error::Invalid)?;
        let scope =
            mayhem_proto::stable_json_bytes(&json!({"home":self.home,"origin":self.origin}))
                .map_err(|_| setup::Error::Invalid)?;
        Ok(LaunchBinding {
            schema_version: 1,
            authority_digest: scope_digest(&scope)?,
            child_name: name,
            child_config_hash: hash_child(&child).map_err(|_| setup::Error::Invalid)?,
        })
    }
    fn inspect<'a>(&'a self, binding: &'a LaunchBinding) -> RunFuture<'a, LifecycleObservation> {
        Box::pin(async move {
            let value=self.post("/children/inspect",&json!({"name":binding.child_name,"expected_config_hash":binding.child_config_hash})).await?;
            let observation: LifecycleObservation =
                serde_json::from_value(value).map_err(|_| setup::Error::RunUnavailable)?;
            observation.validate(binding)?;
            Ok(observation)
        })
    }
    fn install<'a>(
        &'a self,
        binding: &'a LaunchBinding,
        path: &'a Path,
        digest: &'a Digest,
    ) -> RunFuture<'a, ()> {
        Box::pin(async move {
            let config = path.to_owned();
            let expected = digest.clone();
            let prepared = tokio::task::spawn_blocking(move || {
                Prepared::load_supervised_pinned(&config, &expected)
            })
            .await
            .map_err(|_| setup::Error::RunUnavailable)?
            .map_err(|_| setup::Error::RunPrerequisite)?;
            if &self.binding(prepared.identity(), path, digest)? != binding {
                return Err(setup::Error::RunConflict);
            }
            // The already configured wallet is reused; only its protected restart
            // reference is retained in child argv. No secrets go through Flow/UI.
            let password = self
                .password
                .as_deref()
                .map(read_password)
                .transpose()
                .map_err(|_| setup::Error::Protection)?;
            ensure_restart_password(
                password.as_deref(),
                std::env::var_os("MAYHEM_WALLET_PASSWORD").is_some(),
            )
            .map_err(|_| setup::Error::RunPrerequisite)?;
            let key = cli::cached_wallet_signing_key(
                &self.keypair,
                password.as_deref().unwrap_or_default(),
            )
            .await
            .map_err(|_| setup::Error::RunPrerequisite)?;
            Authority::from_unlocked_wallet(key, prepared.identity().clone())
                .map_err(|_| setup::Error::RunConflict)?;
            let child = self
                .child(&binding.child_name, path, digest)
                .map_err(|_| setup::Error::RunConflict)?;
            let response = self.post("/children/add", &child).await?;
            if response["ok"] != true
                || response["persistent"] != true
                || response["name"] != binding.child_name
            {
                return Err(setup::Error::RunUnavailable);
            }
            Ok(())
        })
    }
}

fn scope_digest(bytes: &[u8]) -> setup::Result<Digest> {
    let mut hash = blake3::Hasher::new_derive_key("mayhem/proxy/setup-supervisor/v1");
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Digest::new(hash.finalize().to_hex().to_string()).map_err(|_| setup::Error::Invalid)
}
