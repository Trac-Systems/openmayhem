//! First-time local dashboard authority from existing host state, never HTTP.
use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use mayhem_gateway::openai::GatewayProxySetupBootstrap;
use mayhem_proxy::{attempts::Digest, connector::config::private_file, setup::bootstrap::Host};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Enable first-time provider setup in the existing loopback dashboard.
    #[arg(long, conflicts_with_all=["proxy_setup_config","dev_embedded_catalog"])]
    pub proxy_setup: bool,
    /// Host-approved protected tokenizer data; required for LLM readiness, never downloaded.
    #[arg(long, value_name = "PATH", requires = "proxy_setup")]
    pub proxy_setup_tokenizer_file: Option<PathBuf>,
    /// Optional protected upstream key reference, exposed only as the opaque ID "configured".
    #[arg(long, value_name = "PATH", requires = "proxy_setup")]
    pub proxy_setup_api_key_file: Option<PathBuf>,
    /// Existing protected password reference for unattended managed restarts.
    #[arg(long, value_name = "PATH", requires = "proxy_setup")]
    pub proxy_setup_restart_password_file: Option<PathBuf>,
    /// Trusted admission API origin. Omission leaves enrollment unavailable.
    #[arg(long, requires = "proxy_setup")]
    pub proxy_setup_admission_origin: Option<String>,
    /// Protected registry trust configuration for optional provider declarations.
    #[arg(long, value_name = "PATH", requires = "proxy_setup")]
    pub proxy_setup_declaration_registry_file: Option<PathBuf>,
}

pub async fn prepare(
    args: &Args,
    home: PathBuf,
    keypair: PathBuf,
    seed: &[u8; 32],
    rpc: &mayhem_bridge::PeerRpcClient,
    rpc_url: String,
    bridge_url: String,
) -> Result<GatewayProxySetupBootstrap> {
    crate::require_secure_fund_rpc_url(&rpc_url)?;
    let (status, health, admin) = tokio::time::timeout(Duration::from_secs(15), async {
        let status = rpc.status().await?;
        let health = rpc.health().await?;
        let admin = crate::read_state_value(rpc, "admin")
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .context("canonical setup admin unavailable")?;
        Ok::<_, anyhow::Error>((status, health, admin))
    })
    .await
    .context("setup identity read timed out")??;
    let cfg = crate::read_config_toml_value(&crate::config_path_for_home(&home))?;
    let network = crate::proxy_gateway::expected_identity(&status, &health, &admin, &cfg)?;
    let provider_pubkey = Digest::new(crate::hex_encode(
        ed25519_dalek::SigningKey::from_bytes(seed)
            .verifying_key()
            .as_bytes(),
    ))?;
    let password = args
        .proxy_setup_restart_password_file
        .clone()
        .map(crate::absolutize)
        .transpose()?
        .or_else(|| {
            let p = home.join("secrets/wallet-password");
            p.exists().then_some(p)
        });
    let mut tokenizers = BTreeMap::new();
    if let Some(file) = &args.proxy_setup_tokenizer_file {
        let file = crate::absolutize(file.clone())?;
        let bytes = private_file(&file, 64 * 1024 * 1024)
            .map_err(|_| anyhow::anyhow!("approved tokenizer unavailable or unprotected"))?;
        tokenizers.insert(
            "approved".into(),
            mayhem_proxy::managed::Tokenizer {
                file,
                digest: Digest::new(blake3::hash(&bytes).to_hex().to_string())?,
                limits: mayhem_proxy::health::native::Limits {
                    artifact_bytes: 64 * 1024 * 1024,
                    output_bytes: 4 * 1024 * 1024,
                    channels: 64,
                    workers: 2,
                    minimum_tokens: 8,
                },
            },
        );
    }
    let mut credentials = BTreeMap::new();
    if let Some(file) = &args.proxy_setup_api_key_file {
        let file = crate::absolutize(file.clone())?;
        private_file(&file, 8192).map_err(|_| {
            anyhow::anyhow!("upstream credential reference unavailable or unprotected")
        })?;
        credentials.insert("configured".into(), file);
    }
    let lifecycle = Arc::new(crate::proxy_provider::supervisor::Host::new(
        home.clone(),
        keypair,
        password.clone(),
        provider_pubkey.clone(),
    )?);
    Ok(GatewayProxySetupBootstrap {
        destination: home.join("proxy-setup"),
        host: Host {
            network,
            provider_pubkey,
            peer_rpc: rpc_url,
            bridge_url,
            bridge_token_file: crate::sc_bridge_token_file_path(&home),
            worker_program: std::env::current_exe()?.with_file_name(
                crate::executable_sibling_name("mayhem-proxy-worker", cfg!(windows)),
            ),
            wallet_password_file: password,
            admission_origin: args.proxy_setup_admission_origin.clone(),
            declaration_registry: args.proxy_setup_declaration_registry_file.as_deref().map(mayhem_proxy::setup::DeclarationRegistry::load).transpose()?,
        },
        tokenizers,
        credentials,
        lifecycle,
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn dashboard_bootstrap_real_use_command_preserves_native_default() {
        let native = crate::Cli::try_parse_from(["mayhem", "use"]).unwrap();
        let crate::Commands::Use(native) = native.command else {
            panic!("use")
        };
        assert!(!native.setup_bootstrap.proxy_setup);
        let parsed = crate::Cli::try_parse_from([
            "mayhem",
            "use",
            "--proxy-setup",
            "--proxy-setup-tokenizer-file",
            "approved.json",
        ])
        .unwrap();
        let crate::Commands::Use(args) = parsed.command else {
            panic!("use")
        };
        assert!(args.setup_bootstrap.proxy_setup);
        assert!(crate::Cli::try_parse_from([
            "mayhem",
            "use",
            "--proxy-setup",
            "--proxy-setup-config",
            "original.json"
        ])
        .is_err());
    }
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: super::Args,
        #[arg(long)]
        proxy_setup_config: Option<String>,
        #[arg(long)]
        dev_embedded_catalog: bool,
    }
    #[test]
    fn dashboard_bootstrap_requires_explicit_activation_and_preserves_existing_mode() {
        assert!(
            Cli::try_parse_from(["fixture", "--proxy-setup-tokenizer-file", "fixture.json"])
                .is_err()
        );
        assert!(Cli::try_parse_from([
            "fixture",
            "--proxy-setup",
            "--proxy-setup-config",
            "fixture.json"
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["fixture", "--proxy-setup", "--dev-embedded-catalog"]).is_err()
        );
        let v = Cli::try_parse_from([
            "fixture",
            "--proxy-setup",
            "--proxy-setup-tokenizer-file",
            "fixture.json",
            "--proxy-setup-admission-origin",
            "http://127.0.0.1:8",
        ])
        .unwrap();
        assert!(v.args.proxy_setup);
        assert!(v.args.proxy_setup_tokenizer_file.is_some());
        assert!(!Cli::try_parse_from(["fixture"]).unwrap().args.proxy_setup);
    }
}
