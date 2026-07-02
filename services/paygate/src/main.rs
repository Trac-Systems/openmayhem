#![forbid(unsafe_code)]

use std::{env, net::SocketAddr, path::PathBuf};

use mayhem_paygate::{serve, OracleKeypair, PaygateConfig, PaygateState};

#[derive(Debug, Default)]
struct CliOverrides {
    config_path: Option<PathBuf>,
    bind: Option<SocketAddr>,
    contract_rpc_url: Option<String>,
    oracle_key_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overrides = parse_args()?;
    let mut config = PaygateConfig::from_sources(overrides.config_path.as_deref())?;
    apply_overrides(&mut config, overrides)?;

    let oracle = OracleKeypair::load_or_create(&config.oracle_key_path)?;
    let state = PaygateState::try_new(config.clone(), oracle)?;
    eprintln!(
        "mayhem-paygate listening on http://{} with oracle {}",
        config.bind,
        state.oracle_public_key()
    );
    serve(config.bind, state).await?;
    Ok(())
}

fn parse_args() -> Result<CliOverrides, Box<dyn std::error::Error>> {
    let mut overrides = CliOverrides::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                overrides.config_path = Some(PathBuf::from(value_for(&arg, &mut args)?));
            }
            "--bind" => {
                overrides.bind = Some(value_for(&arg, &mut args)?.parse()?);
            }
            "--contract-rpc-url" => {
                overrides.contract_rpc_url = Some(value_for(&arg, &mut args)?);
            }
            "--oracle-key" => {
                overrides.oracle_key_path = Some(PathBuf::from(value_for(&arg, &mut args)?));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    Ok(overrides)
}

fn value_for(
    arg: &str,
    args: &mut impl Iterator<Item = String>,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("{arg} requires a value").into())
}

fn apply_overrides(
    config: &mut PaygateConfig,
    overrides: CliOverrides,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(bind) = overrides.bind {
        config.bind = bind;
    }
    if let Some(rpc_url) = overrides.contract_rpc_url {
        config.contract_rpc_url = rpc_url;
    }
    if let Some(path) = overrides.oracle_key_path {
        config.oracle_key_path = path;
    }
    config.validate()?;
    Ok(())
}

fn print_help() {
    println!(
        "Usage: mayhem-paygate [--config config.toml] [--bind 127.0.0.1:11436] \
         [--contract-rpc-url http://127.0.0.1:49223/v1] [--oracle-key path]"
    );
}
