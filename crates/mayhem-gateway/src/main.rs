#![forbid(unsafe_code)]

use std::{env, net::SocketAddr};

use mayhem_gateway::openai::{serve, GatewayState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = parse_bind_addr()?;
    let state = GatewayState::from_embedded_catalog();
    eprintln!("mayhem-gateway listening on http://{bind}");
    serve(bind, state).await?;
    Ok(())
}

fn parse_bind_addr() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next() {
        Some(arg) => match arg.as_str() {
            "--bind" => {
                let value = args.next().ok_or("--bind requires an address")?;
                if let Some(extra) = args.next() {
                    return Err(format!("unexpected extra argument: {extra}").into());
                }
                Ok(value.parse()?)
            }
            "--help" | "-h" => {
                println!("Usage: mayhem-gateway [--bind 127.0.0.1:11435]");
                std::process::exit(0);
            }
            _ => Err(format!("unknown argument: {arg}").into()),
        },
        None => {
            let bind =
                env::var("MAYHEM_GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:11435".to_owned());
            Ok(bind.parse()?)
        }
    }
}
