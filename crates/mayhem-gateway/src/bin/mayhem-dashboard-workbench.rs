#![forbid(unsafe_code)]

use std::{env, net::SocketAddr};

use mayhem_gateway::openai::{dashboard_workbench_router, validate_loopback_dashboard_bind};
use tokio::net::TcpListener;

const DEFAULT_BIND: &str = "127.0.0.1:11436";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = parse_bind()?;
    validate_loopback_dashboard_bind(bind)?;

    let listener = TcpListener::bind(bind).await?;
    eprintln!("Mayhem dashboard workbench: http://{bind}/");
    eprintln!("Fixture data only; no Mayhem network services are running.");

    axum::serve(listener, dashboard_workbench_router()?)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn parse_bind() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let mut bind =
        env::var("MAYHEM_DASHBOARD_WORKBENCH_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind = args.next().ok_or("--bind requires an address")?,
            "--help" | "-h" => {
                println!("Usage: mayhem-dashboard-workbench [--bind 127.0.0.1:11436]");
                println!();
                println!("Serves fixture-backed User, Provider, and Network dashboards.");
                println!("It never connects to the Mayhem network or starts inference workers.");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    Ok(bind.parse()?)
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        eprintln!("dashboard workbench shutdown signal failed: {err}");
    }
}
