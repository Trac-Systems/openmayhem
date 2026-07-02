#![forbid(unsafe_code)]

use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mayhem_bridge::{PeerRpcClient, DEFAULT_RPC_URL};
use mayhem_hwprobe::{human_report, probe, FixtureProfile, ProbeOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::sleep;

#[derive(Debug, Parser)]
#[command(name = "mayhem")]
#[command(about = "Mayhem network CLI")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Choose a role, create or import a wallet, and sign current router rules.
    Setup(SetupArgs),
    /// Probe local hardware and print enclave backend feasibility.
    Doctor(DoctorArgs),
    /// Inspect, hash, and re-consent to router rules.
    Rules {
        #[command(subcommand)]
        command: RulesCommands,
    },
}

#[derive(Debug, Subcommand)]
enum RulesCommands {
    /// Print the BLAKE3 hash of RULES.md.
    Hash(RulesHashArgs),
    /// Review current rules and sign fresh consent when needed.
    Review(RulesReviewArgs),
}

#[derive(Debug, Parser)]
struct SetupArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Node role for this install.
    #[arg(long, value_enum)]
    role: Option<Role>,

    /// Wallet setup mode.
    #[arg(long, value_enum, default_value_t = WalletMode::Auto)]
    wallet: WalletMode,

    /// BIP-39 mnemonic to import, or to use for deterministic test creation.
    #[arg(long)]
    mnemonic: Option<String>,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Intercom peer store name under <home>/stores.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Peer JSON-RPC base URL, including /v1.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Skip router-rules consent.
    #[arg(long)]
    no_consent: bool,

    /// Dry-run consent through the contract simulator; does not persist state.
    #[arg(long)]
    sim: bool,

    /// Rules version to consent to. If omitted, read rules/current from RPC.
    #[arg(long)]
    rules_ver: Option<u64>,

    /// Rules hash to consent to. Must be paired with --rules-ver.
    #[arg(long)]
    rules_hash: Option<String>,

    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Accept the displayed rules without an interactive prompt.
    #[arg(long)]
    yes: bool,

    /// Overwrite an existing keypair when using --wallet create/import.
    #[arg(long)]
    force: bool,

    /// Print a machine-readable setup report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct RulesHashArgs {
    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Print a machine-readable hash report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct RulesReviewArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Intercom peer store name under <home>/stores when config.toml is absent.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or the bridge default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Accept the displayed rules without an interactive prompt.
    #[arg(long)]
    yes: bool,

    /// Dry-run consent through the contract simulator; does not persist state.
    #[arg(long)]
    sim: bool,

    /// Print a machine-readable review report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct DoctorArgs {
    /// Print the hardware report as JSON.
    #[arg(long)]
    json: bool,

    /// Run against a deterministic reference profile: apple-silicon, linux-nvidia, or cpu-only.
    #[arg(long)]
    fixture: Option<String>,

    /// Path used for disk free-space and write-throughput probes.
    #[arg(long, value_name = "PATH")]
    disk_path: Option<PathBuf>,

    /// Skip the disk write-throughput benchmark.
    #[arg(long)]
    skip_disk_bench: bool,

    /// Size of the temporary disk benchmark write.
    #[arg(long, default_value_t = 16)]
    disk_bench_mib: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Role {
    Provider,
    User,
    Both,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::User => "user",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WalletMode {
    /// Reuse an existing wallet, otherwise create one.
    Auto,
    /// Create a fresh wallet. Fails if keypair.json exists unless --force is used.
    Create,
    /// Import from --mnemonic. Fails if keypair.json exists unless --force is used.
    Import,
    /// Reuse an existing keypair.json.
    Reuse,
}

#[derive(Debug, Deserialize, Serialize)]
struct WalletInfo {
    created: bool,
    keypair_path: String,
    public_key: String,
    address: Option<String>,
    derivation_path: Option<String>,
    mnemonic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SignOutput {
    signature: String,
}

#[derive(Debug, Clone, Serialize)]
struct RulesRef {
    ver: u64,
    hash: String,
}

#[derive(Debug, Serialize)]
struct ConsentReport {
    skipped: bool,
    simulated: bool,
    rules: Option<RulesRef>,
    tx: Option<String>,
    command_hash: Option<String>,
    result: Option<Value>,
    state: Option<Value>,
}

#[derive(Debug, Clone)]
struct RulesDoc {
    path: PathBuf,
    text: String,
    hash: String,
    bytes: usize,
}

#[derive(Debug, Deserialize)]
struct MayhemConfig {
    identity: Option<ConfigIdentity>,
    network: Option<ConfigNetwork>,
}

#[derive(Debug, Deserialize)]
struct ConfigIdentity {
    keypair_path: Option<String>,
    store_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigNetwork {
    rpc_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup(args) => setup(args).await,
        Commands::Doctor(args) => doctor(args),
        Commands::Rules { command } => match command {
            RulesCommands::Hash(args) => rules_hash(args),
            RulesCommands::Review(args) => rules_review(args).await,
        },
    }
}

async fn setup(args: SetupArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let role = select_role(args.role)?;
    let rpc_url = args
        .rpc_url
        .clone()
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned());
    let store_path = home.join("stores").join(&args.peer_store_name);
    let keypair_path = store_path.join("db").join("keypair.json");
    let password = args.wallet_password.clone().unwrap_or_default();

    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
    fs::create_dir_all(keypair_path.parent().expect("keypair has parent"))
        .with_context(|| format!("creating {}", keypair_path.display()))?;

    let wallet = materialize_wallet(&args, &keypair_path, &password).await?;
    let config_path = write_config(
        &home,
        &store_path,
        &wallet,
        role,
        &args.peer_store_name,
        &rpc_url,
    )?;

    let consent = if args.no_consent {
        ConsentReport {
            skipped: true,
            simulated: args.sim,
            rules: None,
            tx: None,
            command_hash: None,
            result: None,
            state: None,
        }
    } else {
        let rpc = PeerRpcClient::new(&rpc_url)?;
        let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
        let rules = resolve_rules(
            args.rules_ver,
            args.rules_hash.as_deref(),
            &rpc,
            Some(&rules_doc),
        )
        .await?;
        if !args.print_json {
            print_rules_review(&home, &rules_doc, &rules, None, None)?;
        }
        confirm_rules_acceptance(args.yes)?;
        let consent = submit_consent(
            &rpc,
            &keypair_path,
            &password,
            &wallet,
            rules.clone(),
            args.sim,
        )
        .await?;
        if !args.sim {
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        consent
    };

    let report = json!({
        "home": home,
        "role": role.as_str(),
        "config_path": config_path,
        "wallet": wallet,
        "network": {
            "rpc_url": rpc_url,
        },
        "consent": consent,
    });

    if args.print_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report)?;
    }

    Ok(())
}

fn rules_hash(args: RulesHashArgs) -> Result<()> {
    let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
    if args.print_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "rules_path": rules_doc.path,
                "hash": rules_doc.hash,
                "bytes": rules_doc.bytes,
            }))?
        );
    } else {
        println!("{}", rules_doc.hash);
    }
    Ok(())
}

async fn rules_review(args: RulesReviewArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let rpc_url = args
        .rpc_url
        .clone()
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.rpc_url.clone())
        })
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned());
    let store_name = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.store_name.clone())
        .unwrap_or_else(|| args.peer_store_name.clone());
    let keypair_path = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.keypair_path.as_ref())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join("stores")
                .join(&store_name)
                .join("db")
                .join("keypair.json")
        });
    let password = args.wallet_password.clone().unwrap_or_default();
    let wallet = inspect_wallet(&keypair_path, &password).await?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
    let rules = resolve_rules(None, None, &rpc, Some(&rules_doc)).await?;
    let prior_consent = read_consent_state(&rpc, &wallet.public_key).await?;
    let already_consented = consent_matches(prior_consent.as_ref(), &rules);
    let previous_rules = read_current_accepted_rules(&home)?;

    if !args.print_json {
        print_rules_review(
            &home,
            &rules_doc,
            &rules,
            previous_rules.as_deref(),
            prior_consent.as_ref(),
        )?;
    }

    let consent = if already_consented && !args.sim {
        if previous_rules.as_deref() != Some(rules_doc.text.as_str()) {
            confirm_rules_acceptance(args.yes)?;
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        ConsentReport {
            skipped: true,
            simulated: false,
            rules: Some(rules.clone()),
            tx: None,
            command_hash: None,
            result: None,
            state: prior_consent.clone(),
        }
    } else {
        confirm_rules_acceptance(args.yes)?;
        let consent = submit_consent(
            &rpc,
            &keypair_path,
            &password,
            &wallet,
            rules.clone(),
            args.sim,
        )
        .await?;
        if !args.sim {
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        consent
    };
    let consent_skipped = consent.skipped;
    let consent_simulated = consent.simulated;
    let rules_ver = rules.ver;

    let report = json!({
        "home": home,
        "wallet": {
            "public_key": wallet.public_key,
            "address": wallet.address,
            "keypair_path": wallet.keypair_path,
        },
        "network": {
            "rpc_url": rpc_url,
        },
        "rules": {
            "ver": rules.ver,
            "hash": rules.hash,
            "local_hash": rules_doc.hash,
            "path": rules_doc.path,
            "bytes": rules_doc.bytes,
        },
        "prior_consent": prior_consent,
        "already_consented": already_consented,
        "consent": consent,
    });

    if args.print_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if consent_skipped {
        println!("Consent: already current for rules v{rules_ver}");
    } else if consent_simulated {
        println!("Consent: simulated for rules v{rules_ver}");
    } else {
        println!("Consent: submitted and observed for rules v{rules_ver}");
    }

    Ok(())
}

fn doctor(args: DoctorArgs) -> Result<()> {
    let fixture = args
        .fixture
        .as_deref()
        .map(|value| {
            FixtureProfile::parse(value).with_context(|| {
                format!(
                    "unknown fixture {value}; expected apple-silicon, linux-nvidia, or cpu-only"
                )
            })
        })
        .transpose()?;

    let mut options = ProbeOptions::default();
    if let Some(path) = args.disk_path {
        options.disk_path = absolutize(path)?;
    }
    options.run_disk_bench = !args.skip_disk_bench;
    options.disk_bench_mib = args.disk_bench_mib;
    options.fixture = fixture;

    let report = probe(options);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", human_report(&report));
    }
    Ok(())
}

fn default_home() -> Result<PathBuf> {
    if let Ok(home) = env::var("MAYHEM_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }

    let user_home = env::var("HOME").context("HOME is not set; pass --home")?;
    Ok(PathBuf::from(user_home).join(".mayhem"))
}

fn default_rules_path() -> Result<PathBuf> {
    let repo_rules = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../RULES.md");
    if repo_rules.exists() {
        return absolutize(repo_rules);
    }
    absolutize(PathBuf::from("RULES.md"))
}

fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(env::current_dir()?.join(path))
}

fn read_mayhem_config(home: &Path) -> Result<Option<MayhemConfig>> {
    let config_path = home.join("config.toml");
    if !config_path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let config =
        toml::from_str(&text).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok(Some(config))
}

fn read_rules_doc(path: Option<&Path>) -> Result<RulesDoc> {
    let path = match path {
        Some(path) => absolutize(path.to_path_buf())?,
        None => default_rules_path()?,
    };
    let path = fs::canonicalize(&path).unwrap_or(path);
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8(bytes.clone())
        .with_context(|| format!("{} must be valid UTF-8", path.display()))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    Ok(RulesDoc {
        path,
        text,
        hash,
        bytes: bytes.len(),
    })
}

fn rules_state_dir(home: &Path) -> PathBuf {
    home.join("rules")
}

fn read_current_accepted_rules(home: &Path) -> Result<Option<String>> {
    let path = rules_state_dir(home).join("current.md");
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(text))
}

fn persist_rules_acceptance(home: &Path, rules_doc: &RulesDoc, rules: &RulesRef) -> Result<()> {
    let dir = rules_state_dir(home);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let versioned_path = dir.join(format!("accepted-{}-{}.md", rules.ver, rules.hash));
    fs::write(&versioned_path, &rules_doc.text)
        .with_context(|| format!("writing {}", versioned_path.display()))?;
    fs::write(dir.join("current.md"), &rules_doc.text)
        .with_context(|| format!("writing {}", dir.join("current.md").display()))?;
    fs::write(
        dir.join("current.json"),
        serde_json::to_vec_pretty(&json!({
            "ver": rules.ver,
            "hash": rules.hash,
            "path": rules_doc.path,
            "bytes": rules_doc.bytes,
            "accepted_text": versioned_path,
        }))?,
    )
    .with_context(|| format!("writing {}", dir.join("current.json").display()))?;
    Ok(())
}

fn print_rules_review(
    home: &Path,
    rules_doc: &RulesDoc,
    rules: &RulesRef,
    previous_rules: Option<&str>,
    prior_consent: Option<&Value>,
) -> Result<()> {
    println!("Mayhem rules review");
    println!("Home: {}", home.display());
    println!("Rules file: {}", rules_doc.path.display());
    println!("Rules version: {}", rules.ver);
    println!("BLAKE3(RULES.md): {}", rules_doc.hash);
    if let Some(consent) = prior_consent {
        let ver = consent.get("ver").and_then(Value::as_u64).unwrap_or(0);
        let hash = consent.get("hash").and_then(Value::as_str).unwrap_or("");
        println!("Existing consent: v{ver} {hash}");
    } else {
        println!("Existing consent: none");
    }

    match previous_rules {
        Some(previous) if previous == rules_doc.text => {
            println!();
            println!("Local accepted rules text is unchanged.");
        }
        Some(previous) => {
            println!();
            println!("Diff from locally accepted rules:");
            println!("{}", render_line_diff(previous, &rules_doc.text));
        }
        None => {
            println!();
            println!("No locally accepted rules text was found. Current rules text follows.");
            println!();
            println!("{}", rules_doc.text);
        }
    }
    Ok(())
}

fn render_line_diff(old: &str, new: &str) -> String {
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let max_len = old_lines.len().max(new_lines.len());
    let mut out = String::new();
    for i in 0..max_len {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(old), Some(new)) if old == new => {
                let _ = writeln!(out, "  {old}");
            }
            (Some(old), Some(new)) => {
                let _ = writeln!(out, "- {old}");
                let _ = writeln!(out, "+ {new}");
            }
            (Some(old), None) => {
                let _ = writeln!(out, "- {old}");
            }
            (None, Some(new)) => {
                let _ = writeln!(out, "+ {new}");
            }
            (None, None) => {}
        }
    }
    out
}

fn confirm_rules_acceptance(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        bail!("Pass --yes to accept and sign the current RULES.md in non-interactive mode.");
    }

    eprint!("Type 'agree' to sign consent for these rules: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() != "agree" {
        bail!("Rules consent was not signed.");
    }
    Ok(())
}

fn select_role(role: Option<Role>) -> Result<Role> {
    if let Some(role) = role {
        return Ok(role);
    }

    if !io::stdin().is_terminal() {
        bail!("Pass --role provider, --role user, or --role both for non-interactive setup.");
    }

    loop {
        print!("Choose role [provider/user/both]: ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        match answer.trim().to_lowercase().as_str() {
            "provider" | "prov" | "p" => return Ok(Role::Provider),
            "user" | "u" => return Ok(Role::User),
            "both" | "b" => return Ok(Role::Both),
            _ => eprintln!("Please enter provider, user, or both."),
        }
    }
}

async fn materialize_wallet(
    args: &SetupArgs,
    keypair_path: &Path,
    password: &str,
) -> Result<WalletInfo> {
    let exists = keypair_path.exists();
    match args.wallet {
        WalletMode::Auto => {
            if exists {
                inspect_wallet(keypair_path, password).await
            } else {
                create_wallet(keypair_path, password, args.mnemonic.as_deref(), false).await
            }
        }
        WalletMode::Create => {
            if exists && !args.force {
                bail!(
                    "{} already exists; use --wallet reuse or pass --force.",
                    keypair_path.display()
                );
            }
            create_wallet(keypair_path, password, args.mnemonic.as_deref(), args.force).await
        }
        WalletMode::Import => {
            if exists && !args.force {
                bail!(
                    "{} already exists; use --wallet reuse or pass --force.",
                    keypair_path.display()
                );
            }
            let mnemonic = args
                .mnemonic
                .as_deref()
                .context("--wallet import requires --mnemonic")?;
            create_wallet(keypair_path, password, Some(mnemonic), true).await
        }
        WalletMode::Reuse => {
            if !exists {
                bail!(
                    "{} does not exist; use --wallet create/import.",
                    keypair_path.display()
                );
            }
            inspect_wallet(keypair_path, password).await
        }
    }
}

async fn create_wallet(
    keypair_path: &Path,
    password: &str,
    mnemonic: Option<&str>,
    force: bool,
) -> Result<WalletInfo> {
    let mut args = vec![
        "create".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    if let Some(mnemonic) = mnemonic {
        args.extend(["--mnemonic".to_owned(), mnemonic.to_owned()]);
    }
    if force {
        args.push("--force".to_owned());
    }
    run_wallet_helper(args).await
}

async fn inspect_wallet(keypair_path: &Path, password: &str) -> Result<WalletInfo> {
    let mut args = vec![
        "inspect".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    run_wallet_helper(args).await
}

async fn sign_message(keypair_path: &Path, password: &str, message: &str) -> Result<String> {
    let mut args = vec![
        "sign".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
        "--message".to_owned(),
        message.to_owned(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    let output: SignOutput = run_wallet_helper(args).await?;
    Ok(output.signature)
}

async fn sign_hex(keypair_path: &Path, password: &str, message_hex: &str) -> Result<String> {
    let mut args = vec![
        "sign".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
        "--message-hex".to_owned(),
        message_hex.to_owned(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    let output: SignOutput = run_wallet_helper(args).await?;
    Ok(output.signature)
}

async fn run_wallet_helper<T>(args: Vec<String>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/wallet-helper.mjs");
    let output = Command::new("node")
        .arg(&helper)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running wallet helper {}", helper.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("wallet helper failed: {}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("parsing wallet helper JSON output")
}

fn write_config(
    home: &Path,
    store_path: &Path,
    wallet: &WalletInfo,
    role: Role,
    store_name: &str,
    rpc_url: &str,
) -> Result<PathBuf> {
    let config_path = home.join("config.toml");
    let address = wallet.address.as_deref().unwrap_or("");
    let derivation_path = wallet.derivation_path.as_deref().unwrap_or("");
    let contents = format!(
        concat!(
            "[identity]\n",
            "public_key = {}\n",
            "address = {}\n",
            "derivation_path = {}\n",
            "keypair_path = {}\n",
            "store_name = {}\n",
            "store_path = {}\n\n",
            "[role]\n",
            "mode = {}\n\n",
            "[network]\n",
            "rpc_url = {}\n"
        ),
        toml_string(&wallet.public_key),
        toml_string(address),
        toml_string(derivation_path),
        toml_string(&wallet.keypair_path),
        toml_string(store_name),
        toml_string(&store_path.display().to_string()),
        toml_string(role.as_str()),
        toml_string(rpc_url),
    );
    fs::write(&config_path, contents)
        .with_context(|| format!("writing {}", config_path.display()))?;
    Ok(config_path)
}

fn toml_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

async fn resolve_rules(
    rules_ver: Option<u64>,
    rules_hash: Option<&str>,
    rpc: &PeerRpcClient,
    rules_doc: Option<&RulesDoc>,
) -> Result<RulesRef> {
    let rules = match (rules_ver, rules_hash) {
        (Some(ver), Some(hash)) => {
            validate_rules_hash(hash)?;
            RulesRef {
                ver,
                hash: hash.to_owned(),
            }
        }
        (None, None) => {
            let state = rpc
                .state(Some("rules/current"), Some(false))
                .await
                .context("reading rules/current from peer RPC")?;
            let value = state
                .get("value")
                .filter(|value| !value.is_null())
                .context("rules/current is not set; ask the admin peer to set rules first")?;
            let ver = value
                .get("ver")
                .and_then(Value::as_u64)
                .context("rules/current.ver is missing or invalid")?;
            let hash = value
                .get("hash")
                .and_then(Value::as_str)
                .context("rules/current.hash is missing or invalid")?
                .to_owned();
            validate_rules_hash(&hash)?;
            RulesRef { ver, hash }
        }
        _ => bail!("--rules-ver and --rules-hash must be provided together."),
    };

    if let Some(rules_doc) = rules_doc {
        if rules.hash != rules_doc.hash {
            bail!(
                "contract rules hash {} does not match BLAKE3({}) {}",
                rules.hash,
                rules_doc.path.display(),
                rules_doc.hash
            );
        }
    }

    Ok(rules)
}

fn validate_rules_hash(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        bail!("rules hash must be a 32-byte hex BLAKE3 digest (64 characters).");
    }
    Ok(())
}

async fn read_consent_state(rpc: &PeerRpcClient, public_key: &str) -> Result<Option<Value>> {
    let key = format!("consent/{public_key}");
    let state = rpc
        .state(Some(&key), Some(false))
        .await
        .with_context(|| format!("reading {key} from peer RPC"))?;
    Ok(state.get("value").cloned().filter(|value| !value.is_null()))
}

fn consent_matches(consent: Option<&Value>, rules: &RulesRef) -> bool {
    consent.is_some_and(|state| {
        state.get("ver").and_then(Value::as_u64) == Some(rules.ver)
            && state.get("hash").and_then(Value::as_str) == Some(rules.hash.as_str())
    })
}

async fn submit_consent(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    rules: RulesRef,
    sim: bool,
) -> Result<ConsentReport> {
    let consent_message = format!("mayhem-consent{}{}", rules.ver, rules.hash);
    let consent_sig = sign_message(keypair_path, password, &consent_message).await?;
    let prepared_command = json!({
        "type": "consent",
        "value": {
            "op": "consent",
            "ver": rules.ver,
            "hash": rules.hash,
            "sig": consent_sig,
        }
    });

    let nonce_response = rpc
        .contract_nonce()
        .await
        .context("requesting contract nonce")?;
    let nonce = nonce_response
        .get("nonce")
        .and_then(Value::as_str)
        .context("RPC nonce response did not include nonce")?;

    let prepared = rpc
        .prepare_tx(json!({
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "nonce": nonce,
        }))
        .await
        .context("preparing consent tx")?;
    let tx = prepared
        .get("tx")
        .and_then(Value::as_str)
        .context("RPC prepare response did not include tx")?
        .to_owned();
    let command_hash = prepared
        .get("command_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let signature = sign_hex(keypair_path, password, &tx).await?;
    let submitted = rpc
        .submit_tx(json!({
            "tx": tx,
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "signature": signature,
            "nonce": nonce,
            "sim": sim,
        }))
        .await
        .context("submitting consent tx")?;
    let result = Some(
        submitted
            .get("result")
            .cloned()
            .unwrap_or_else(|| submitted.clone()),
    );

    let state = if sim {
        None
    } else {
        Some(wait_for_consent_state(rpc, &wallet.public_key, &rules).await?)
    };

    Ok(ConsentReport {
        skipped: false,
        simulated: sim,
        rules: Some(rules),
        tx: Some(tx),
        command_hash,
        result,
        state,
    })
}

async fn wait_for_consent_state(
    rpc: &PeerRpcClient,
    public_key: &str,
    rules: &RulesRef,
) -> Result<Value> {
    let key = format!("consent/{public_key}");
    let mut last = Value::Null;
    for _ in 0..120 {
        let state = rpc
            .state(Some(&key), Some(false))
            .await
            .with_context(|| format!("reading {key} from peer RPC"))?;
        last = state.get("value").cloned().unwrap_or(Value::Null);
        if last.get("ver").and_then(Value::as_u64) == Some(rules.ver)
            && last.get("hash").and_then(Value::as_str) == Some(rules.hash.as_str())
        {
            return Ok(last);
        }
        sleep(Duration::from_millis(500)).await;
    }

    bail!("timed out waiting for persisted consent state; last value: {last}");
}

fn print_human_report(report: &Value) -> Result<()> {
    println!("Mayhem setup complete.");
    println!("Home: {}", report["home"].as_str().unwrap_or(""));
    println!("Role: {}", report["role"].as_str().unwrap_or(""));
    println!("Config: {}", report["config_path"].as_str().unwrap_or(""));
    println!(
        "Wallet: {}",
        if report["wallet"]["created"].as_bool().unwrap_or(false) {
            "created"
        } else {
            "reused"
        }
    );
    println!(
        "Public key: {}",
        report["wallet"]["public_key"].as_str().unwrap_or("")
    );
    if let Some(address) = report["wallet"]["address"].as_str() {
        println!("Address: {address}");
    }
    if let Some(mnemonic) = report["wallet"]["mnemonic"].as_str() {
        println!("Mnemonic (shown once): {mnemonic}");
    }

    let consent = &report["consent"];
    if consent["skipped"].as_bool().unwrap_or(false) {
        println!("Consent: skipped");
    } else if consent["simulated"].as_bool().unwrap_or(false) {
        println!(
            "Consent: simulated for rules v{}",
            consent["rules"]["ver"].as_u64().unwrap_or(0)
        );
    } else {
        println!(
            "Consent: submitted and observed for rules v{}",
            consent["rules"]["ver"].as_u64().unwrap_or(0)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn rules_hash_allows_hex_only() {
        assert!(validate_rules_hash(&"a".repeat(64)).is_ok());
        assert!(validate_rules_hash("not-hex").is_err());
        assert!(validate_rules_hash("aabb001122").is_err());
        assert!(validate_rules_hash("").is_err());
    }
}
