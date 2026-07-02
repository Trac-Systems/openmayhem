#![forbid(unsafe_code)]

use std::{env, net::SocketAddr};

use mayhem_gateway::openai::{serve, GatewayState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    if !args.dev_embedded_catalog {
        eprintln!(
            "mayhem-gateway is a development smoke binary and does not read the contract ledger."
        );
        eprintln!("Use `mayhem use` for canonical contract-backed models.");
        eprintln!(
            "For isolated API smoke only, pass: mayhem-gateway --dev-embedded-catalog --bind {}",
            args.bind
        );
        std::process::exit(2);
    }
    let state = GatewayState::from_embedded_catalog();
    eprintln!(
        "mayhem-gateway listening on http://{} with development embedded catalog",
        args.bind
    );
    serve(args.bind, state).await?;
    Ok(())
}

struct GatewayArgs {
    bind: SocketAddr,
    dev_embedded_catalog: bool,
}

fn parse_args() -> Result<GatewayArgs, Box<dyn std::error::Error>> {
    let mut bind = env::var("MAYHEM_GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:11435".to_owned());
    let mut dev_embedded_catalog = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                bind = args.next().ok_or("--bind requires an address")?;
            }
            "--dev-embedded-catalog" => {
                dev_embedded_catalog = true;
            }
            "--help" | "-h" => {
                println!("Usage: mayhem-gateway --dev-embedded-catalog [--bind 127.0.0.1:11435]");
                println!();
                println!("Development-only raw gateway. Use `mayhem use` for canonical contract-backed models.");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    Ok(GatewayArgs {
        bind: bind.parse()?,
        dev_embedded_catalog,
    })
}
