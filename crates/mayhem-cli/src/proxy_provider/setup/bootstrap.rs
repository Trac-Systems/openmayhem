//! Guided first setup. Host authority comes from existing configured Core;
//! questions collect operator choices, never upstream assertions or secret argv.
use super::*;
mod guided;
use anyhow::{ensure, Context};
use mayhem_proto::proxy::{
    finance::{ProxyHoldExpiry, ProxyReceiptOutcome, ProxySettlementPolicy},
    *,
};
use mayhem_proxy::setup::guided::{usd_to_au, usd_to_microusd, Canonical};
use mayhem_proxy::{
    attempts::Digest,
    connector::config::{private_file, NetworkPolicy},
    setup::{
        bootstrap::{self, Choices, Credential, Host},
        OfferInput, ProfileMarket,
    },
};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};

#[derive(Debug, Args)]
pub struct InitArgs {
    #[command(flatten)]
    wallet: WalletLocatorArgs,
    /// Existing protected upstream bearer-key file. Never accepts the key itself.
    #[arg(long, value_name = "PATH")]
    api_key_file: Option<PathBuf>,
    /// Approved local tokenizer data, required for LLM profiles. No remote download.
    #[arg(long, value_name = "PATH")]
    tokenizer_file: Option<PathBuf>,
    /// Existing wallet password reference retained for unattended restarts.
    #[arg(long, value_name = "PATH")]
    restart_password_file: Option<PathBuf>,
    /// Explicit trusted admission API origin; omission keeps enrollment unavailable.
    #[arg(long)]
    admission_origin: Option<String>,
    /// Existing trusted peer. Defaults to the saved Core configuration.
    #[arg(long)]
    rpc_url: Option<String>,
}
fn ask(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<String> {
    write!(
        output,
        "{label} [{}]: ",
        if default.is_empty() {
            "required"
        } else {
            default
        }
    )?;
    output.flush()?;
    let mut bytes = Vec::new();
    let n = std::io::Read::take(&mut *input, 4097).read_until(b'\n', &mut bytes)?;
    ensure!(
        n > 0 && n <= 4096,
        "input ended or exceeded its bound; setup was not saved"
    );
    let value = std::str::from_utf8(&bytes)?.trim();
    let value = if value.is_empty() { default } else { value };
    ensure!(
        !value.is_empty() && !value.chars().any(char::is_control),
        "an explicit valid choice is required"
    );
    Ok(value.into())
}
fn number<T: std::str::FromStr>(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<T> {
    ask(input, output, label, default)?
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid numeric choice"))
}
fn yes(input: &mut impl BufRead, output: &mut impl Write, label: &str) -> Result<bool> {
    match ask(input, output, label, "")?.as_str() {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => anyhow::bail!("choose yes or no"),
    }
}
fn path(value: &Path) -> Result<PathBuf> {
    Ok(crate::absolutize(value.to_owned())?)
}

async fn choices(
    input: &mut impl BufRead,
    output: &mut impl Write,
    args: &InitArgs,
    host: &Host,
    home: &Path,
) -> Result<Choices> {
    let mut base_url = ask(
        input,
        output,
        "Upstream API base URL (directory, e.g. https://host/v1/)",
        "",
    )?;
    if !base_url.ends_with('/') {
        base_url.push('/');
    }
    let endpoint = match ask(
        input,
        output,
        "Protocol endpoint: chat, completions, responses, decisions",
        "chat",
    )?
    .as_str()
    {
        "chat" => ProxyEndpoint::Chat,
        "completions" => ProxyEndpoint::Completions,
        "responses" => ProxyEndpoint::Responses,
        "decisions" => ProxyEndpoint::Decisions,
        _ => {
            anyhow::bail!("unsupported endpoint; use a reviewed custom profile for other protocols")
        }
    };
    let network_policy = if yes(
        input,
        output,
        "Public HTTPS only? yes/no (no requires exact explicitly permitted IP networks)",
    )? {
        NetworkPolicy::PublicHttps
    } else {
        let networks = ask(
            input,
            output,
            "Explicit allowed CIDRs, comma-separated; never a network scan",
            "",
        )?
        .split(',')
        .map(|s| s.trim().parse())
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let allow_http = yes(
            input,
            output,
            "Permit plaintext HTTP for those exact destinations? yes/no",
        )?;
        NetworkPolicy::Pinned {
            networks,
            allow_http,
        }
    };
    let credential = if let Some(file) = &args.api_key_file {
        Credential::BearerFile(path(file)?)
    } else {
        let value = ask(
            input,
            output,
            "Protected bearer-key file path, or none for no authentication (never paste a key)",
            "",
        )?;
        if value == "none" {
            Credential::None
        } else {
            Credential::BearerFile(path(Path::new(&value))?)
        }
    };
    let canonical = Canonical::new(host)?;
    let upstream_model =
        guided::model(input, output, home, &base_url, &network_policy, &credential).await?;
    let market = guided::market(input, output, &canonical, endpoint, &upstream_model).await?;
    let served_context: u32 = number(input, output, "Declared served context", "")?;
    let concurrency = number(
        input,
        output,
        "Maximum shared upstream concurrency for OpenMayhem",
        "",
    )?;
    let mut accepted_rails = ask(
        input,
        output,
        "Accepted rails, comma-separated: fiat,tnk,tap",
        "",
    )?
    .split(',')
    .map(|v| match v.trim() {
        "fiat" => Ok(ProxyRail::Fiat),
        "tnk" => Ok(ProxyRail::Tnk),
        "tap" => Ok(ProxyRail::Tap),
        _ => Err(anyhow::anyhow!("unknown rail")),
    })
    .collect::<Result<Vec<_>>>()?;
    accepted_rails.sort();
    ensure!(
        accepted_rails.windows(2).all(|v| v[0] != v[1]),
        "duplicate rail"
    );
    writeln!(output, "LLM input/output units are normalized billing units, not upstream tokenizer tokens. Prices are USD-denominated, converted exactly at 1 USD = 10^18 AU; no token FX or fee is inferred.")?;
    let units = mayhem_proxy::metering::Policy::for_endpoint(endpoint)
        .contract()
        .units;
    let mut rates = Vec::new();
    for unit in units {
        let granularity = number(
            input,
            output,
            &format!("{unit}: billing units per price"),
            "",
        )?;
        let per_unit_au = usd_to_au(&ask(
            input,
            output,
            &format!("{unit}: USD for that many units (up to 18 decimal places)"),
            "",
        )?)?;
        rates.push(ProxyRate {
            unit,
            granularity,
            per_unit_au,
        });
    }
    let per_request_au = usd_to_au(&ask(
        input,
        output,
        "Additional USD per request (enter 0 if none)",
        "",
    )?)?;
    let min_session_au = usd_to_au(&ask(
        input,
        output,
        "Minimum session USD (enter 0 if none)",
        "",
    )?)?;
    let ctx_bracket = format!("ctx{served_context}");
    let outcome_class = if endpoint == ProxyEndpoint::Decisions {
        let value = ask(
            input,
            output,
            "Exact canonical decision outcome-class hash, or unclassified",
            "unclassified",
        )?;
        if value == "unclassified" {
            String::new()
        } else {
            value
        }
    } else {
        String::new()
    };
    let sequence = canonical.next_sequence().await?;
    writeln!(
        output,
        "Canonical next operation sequence: {sequence} (rechecked before save and publication)."
    )?;
    let mut payable_outcomes = vec![ProxyReceiptOutcome::Complete];
    for (label, outcome) in [
        (
            "Allow charges for verified partial outcomes? yes/no",
            ProxyReceiptOutcome::Partial,
        ),
        (
            "Allow charges for verified refused outcomes? yes/no",
            ProxyReceiptOutcome::Refused,
        ),
        (
            "Allow charges for verified cancelled outcomes? yes/no",
            ProxyReceiptOutcome::Cancelled,
        ),
    ] {
        if yes(input, output, label)? {
            payable_outcomes.push(outcome);
        }
    }
    payable_outcomes.sort();
    let allow_checkpoints = yes(
        input,
        output,
        "Allow separately authorized checkpoints? yes/no",
    )?;
    let hold_expiry = match ask(
        input,
        output,
        "Hold expiry policy: none or release_unfinalized_and_block_retry",
        "",
    )?
    .as_str()
    {
        "none" => None,
        "release_unfinalized_and_block_retry" => {
            Some(ProxyHoldExpiry::ReleaseUnfinalizedAndBlockRetry)
        }
        _ => anyhow::bail!("unsupported explicit hold-expiry policy"),
    };
    let max_attempts = number(input, output, "Cumulative probe attempt allowance", "")?;
    let max_cost_microusd = usd_to_microusd(&ask(
        input,
        output,
        "Cumulative probe allowance in USD (up to 6 decimal places; 0 only for no-charge backend)",
        "",
    )?)?;
    let per_attempt_cost_microusd = usd_to_microusd(&ask(
        input,
        output,
        "Approved maximum estimated USD per probe",
        "",
    )?)?;
    let probe_output_limit = number(
        input,
        output,
        "Probe maximum output tokens/decoder bound (1–1024)",
        "32",
    )?;
    let probe_timeout_ms = number(
        input,
        output,
        "Probe deadline in milliseconds (1–30000)",
        "10000",
    )?;
    let allow_recovery_probes = yes(
        input,
        output,
        "Allow targeted recovery probes within this same cumulative allowance? yes/no",
    )?;
    let tokenizer = if endpoint != ProxyEndpoint::Decisions {
        writeln!(output, "LLM readiness requires approved local tokenizer data. This measures speed, never billing or remote model identity; no tokenizer is guessed/downloaded.")?;
        let file = match &args.tokenizer_file {
            Some(p) => path(p)?,
            None => path(Path::new(&ask(
                input,
                output,
                "Protected approved tokenizer.json path",
                "",
            )?))?,
        };
        let bytes = private_file(&file, 64 * 1024 * 1024).map_err(|_| {
            anyhow::anyhow!("approved tokenizer data is unavailable or unprotected")
        })?;
        Some(mayhem_proxy::managed::Tokenizer {
            file,
            digest: Digest::new(blake3::hash(&bytes).to_hex().to_string())?,
            limits: mayhem_proxy::health::native::Limits {
                artifact_bytes: 64 * 1024 * 1024,
                output_bytes: 4 * 1024 * 1024,
                channels: 64,
                workers: 2,
                minimum_tokens: 8,
            },
        })
    } else {
        ensure!(
            args.tokenizer_file.is_none(),
            "decisions does not use a tokenizer"
        );
        None
    };
    let days: u64 = number(
        input,
        output,
        "Local completed-request journal retention in days (not an upstream/privacy promise)",
        "",
    )?;
    let closed_retention_ms = days.checked_mul(86_400_000).context("retention overflow")?;
    Ok(Choices {
        base_url,
        network_policy,
        credential,
        endpoint,
        upstream_model,
        market,
        served_context,
        concurrency,
        accepted_rails: accepted_rails.clone(),
        offers: vec![OfferInput {
            revision: 1,
            ctx_bracket,
            outcome_class,
            rates,
            per_request_au,
            min_session_au,
            accepted_rails,
        }],
        sequence,
        settlement_policy: ProxySettlementPolicy {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            payable_outcomes,
            allow_checkpoints,
            hold_expiry,
        },
        probe_budget: mayhem_proxy::capacity::probes::Budget {
            max_attempts,
            max_cost_microusd,
            per_attempt_cost_microusd,
        },
        probe_output_limit,
        probe_timeout_ms,
        allow_recovery_probes,
        tokenizer,
        closed_retention_ms,
    })
}

pub async fn run(mut args: InitArgs) -> Result<()> {
    let home = crate::absolutize(
        args.wallet
            .home
            .clone()
            .map(Ok)
            .unwrap_or_else(crate::default_home)?,
    )?;
    let destination = home.join("proxy-setup");
    ensure!(
        std::fs::symlink_metadata(&destination).is_err(),
        "a setup already exists; resume its wizard.json, do not reset the original Run or budget"
    );
    let config = crate::read_config_toml_value(&crate::config_path_for_home(&home))?;
    let rpc_url = crate::resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
    crate::require_secure_fund_rpc_url(&rpc_url)?;
    let bridge_url = crate::toml_get_path(&config, "network.sc_bridge_url")
        .and_then(toml::Value::as_str)
        .context(
            "existing Core bridge is not configured; start the existing local Core before setup",
        )?
        .to_owned();
    let password = args
        .restart_password_file
        .clone()
        .map(|v| path(&v))
        .transpose()?
        .or_else(|| {
            let p = home.join("secrets/wallet-password");
            p.exists().then_some(p)
        });
    if let Some(file) = &password {
        let bytes = private_file(file, 8192).map_err(|_| {
            anyhow::anyhow!("restart password reference is unavailable or unprotected")
        })?;
        args.wallet.wallet_password = Some(std::str::from_utf8(&bytes)?.trim_end().into());
    }
    let rpc = mayhem_bridge::PeerRpcClient::new(&rpc_url)?;
    let (status, health, admin) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let status = rpc.status().await?;
        let health = rpc.health().await?;
        let admin = crate::read_state_value(&rpc, "admin")
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .context("canonical admin identity unavailable")?;
        Ok::<_, anyhow::Error>((status, health, admin))
    })
    .await
    .context("configured Core identity read timed out")??;
    let network = crate::proxy_gateway::expected_identity(&status, &health, &admin, &config)?;
    let keypair = resolve_wallet_keypair_path(&args.wallet)?;
    let key = cached_wallet_signing_key(
        &keypair,
        args.wallet.wallet_password.as_deref().unwrap_or_default(),
    )
    .await?;
    let host = Host {
        network,
        provider_pubkey: Digest::new(crate::hex_encode(key.verifying_key().as_bytes()))?,
        peer_rpc: rpc_url,
        bridge_url,
        bridge_token_file: crate::sc_bridge_token_file_path(&home),
        worker_program: std::env::current_exe()?.with_file_name(crate::executable_sibling_name(
            "mayhem-proxy-worker",
            cfg!(windows),
        )),
        wallet_password_file: password.clone(),
        admission_origin: args.admission_origin.clone(),
    };
    let mut output = io::stdout();
    writeln!(output,"Choose an existing model server and canonical market. Only explicitly requested discovery reads run here; no probe, payment, publication or service change.")?;
    let chosen = choices(&mut io::stdin().lock(), &mut output, &args, &host, &home).await?;
    for r in &chosen.offers[0].rates {
        writeln!(
            output,
            "USD {} = {} AU per {} {} billing units",
            mayhem_proxy::setup::guided::au_to_usd(r.per_unit_au),
            r.per_unit_au,
            r.granularity,
            r.unit
        )?;
    }
    writeln!(
        output,
        "Review exact choices:\n{}",
        serde_json::to_string_pretty(&chosen.review())?
    )?;
    ensure!(
        yes(
            &mut io::stdin().lock(),
            &mut output,
            "Recheck canonical selection and save this private setup? yes/no"
        )?,
        "setup cancelled"
    );
    Canonical::new(&host)?.revalidate(&chosen).await?;
    let bundle = tokio::task::spawn_blocking(move || bootstrap::create(&destination, host, chosen))
        .await??;
    let password_arg = password
        .as_ref()
        .map(|p| {
            format!(
                " --wallet-password-file {}",
                crate::shell_single_quote(&p.to_string_lossy())
            )
        })
        .unwrap_or_default();
    println!("Saved protected setup. Resume with: mayhem provider proxy setup wizard --config {} --home {} --keypair {}{}",
        crate::shell_single_quote(&bundle.config_file.to_string_lossy()), crate::shell_single_quote(&home.to_string_lossy()),
        crate::shell_single_quote(&keypair.to_string_lossy()), password_arg);
    println!("The standard resource profile bounds parser buffers, sessions and health sampling. The wizard will show the exact probe and Run plans before any execution.");
    if yes(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        "Open the shared wizard now? yes/no",
    )? {
        super::wizard::resume(bundle.config_file, args.wallet).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn bootstrap_prompts_bound_input_and_never_accept_implicit_financial_choices() {
        assert!(yes(&mut io::Cursor::new(b"\n"), &mut Vec::new(), "charge?").is_err());
        assert!(ask(
            &mut io::Cursor::new(vec![b'a'; 4097]),
            &mut Vec::new(),
            "url",
            ""
        )
        .is_err());
        assert_eq!(
            ask(
                &mut io::Cursor::new(b"\n"),
                &mut Vec::new(),
                "protocol",
                "chat"
            )
            .unwrap(),
            "chat"
        );
        assert!(ask(
            &mut io::Cursor::new(b"value\x00\n"),
            &mut Vec::new(),
            "field",
            ""
        )
        .is_err());
    }
    #[test]
    fn bootstrap_cli_accepts_only_secret_references_and_preserves_explicit_prices_policy() {
        assert!(crate::Cli::try_parse_from([
            "mayhem",
            "provider",
            "proxy",
            "setup",
            "init",
            "--api-key-file",
            "/private/key",
            "--tokenizer-file",
            "/private/tokenizer"
        ])
        .is_ok());
        assert!(crate::Cli::try_parse_from([
            "mayhem",
            "provider",
            "proxy",
            "setup",
            "init",
            "--api-key",
            "never-on-argv"
        ])
        .is_err());
        assert_eq!(usd_to_au("0.000000000000000123").unwrap(), 123);
        assert_eq!(usd_to_microusd("0.000005").unwrap(), 5);
        assert!(usd_to_microusd("0.0000005").is_err());
    }
}

#[cfg(all(test, unix))]
#[path = "bootstrap/guided_tests.rs"]
mod guided_tests;
