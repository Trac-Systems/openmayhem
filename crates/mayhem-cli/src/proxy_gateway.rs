//! Optional `mayhem use` proxy control. Native gateway startup remains the default;
//! this owner explicitly joins the extra controls on process stop or bind failure.
use anyhow::{anyhow, ensure, Context, Result};
use mayhem_bridge::PeerRpcClient;
use mayhem_gateway::openai::{
    proxy_control::{Prepared, ProxyControl, ProxyLifecycle},
    serve_with_shutdown, GatewayState,
};
use mayhem_proxy::discovery::Identity;
use serde_json::Value;
use std::{future::Future, io, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::watch;

pub async fn prepare(
    path: PathBuf,
    home: PathBuf,
    rpc: &PeerRpcClient,
) -> Result<(Arc<ProxyControl>, ProxyLifecycle)> {
    // The same trusted RPC already selected for native canonical startup supplies
    // this identity. No network identity is learned from the proxy config itself.
    let status = rpc
        .status()
        .await
        .map_err(|_| anyhow!("reading gateway peer identity failed"))?;
    let health = rpc
        .health()
        .await
        .map_err(|_| anyhow!("reading gateway contract identity failed"))?;
    let admin = super::read_state_value(rpc, "admin")
        .await?
        .and_then(|v| v.as_str().map(str::to_owned))
        .context("canonical gateway admin identity is missing")?;
    tokio::task::spawn_blocking(move || {
        let config = super::read_config_toml_value(&super::config_path_for_home(&home))?;
        let expected = expected_identity(&status, &health, &admin, &config)?;
        Prepared::load(&path, &expected)?.open().map_err(Into::into)
    })
    .await
    .context("preparing optional proxy gateway control")?
}

fn expected_identity(
    status: &Value,
    health: &Value,
    canonical_admin: &str,
    config: &toml::Value,
) -> Result<Identity> {
    let peer = &status["peer"];
    let msb = &status["msb"];
    ensure!(
        super::is_hex_len(canonical_admin, 64),
        "canonical gateway admin identity is invalid"
    );
    let admin = peer["admin"]
        .as_str()
        .context("gateway peer admin identity is missing")?;
    ensure!(
        admin.eq_ignore_ascii_case(canonical_admin),
        "gateway peer and canonical admin identity differ"
    );
    let network = Identity {
        network_id: msb["networkId"]
            .as_u64()
            .context("gateway MSB network identity is missing")?
            .to_string(),
        msb_bootstrap: msb["bootstrapHex"]
            .as_str()
            .context("gateway MSB bootstrap is missing")?
            .to_ascii_lowercase(),
        subnet_bootstrap: peer["subnetBootstrapHex"]
            .as_str()
            .context("gateway subnet bootstrap is missing")?
            .to_ascii_lowercase(),
        contract_version: u32::try_from(
            health["contract_version"]
                .as_u64()
                .context("gateway verified contract version is missing")?,
        )
        .context("gateway contract version is invalid")?,
    };
    network.validate()?;
    ensure!(
        network.contract_version == mayhem_proto::CONTRACT_VERSION,
        "gateway contract version is unsupported"
    );
    for (key, current) in [
        ("network.msb_bootstrap", network.msb_bootstrap.as_str()),
        (
            "network.subnet_bootstrap",
            network.subnet_bootstrap.as_str(),
        ),
        ("network.admin_peer_pubkey", canonical_admin),
    ] {
        if let Some(pinned) = super::toml_get_path(config, key) {
            ensure!(
                pinned
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(current)),
                "gateway peer identity differs from configured network pins"
            );
        }
    }
    if super::toml_get_path(config, "network.name").and_then(toml::Value::as_str) == Some("mainnet")
    {
        let manifest = super::canonical_mainnet_manifest()?;
        ensure!(
            network.network_id == manifest.network.msb.network_id.to_string()
                && network
                    .msb_bootstrap
                    .eq_ignore_ascii_case(&manifest.network.msb.bootstrap)
                && network
                    .subnet_bootstrap
                    .eq_ignore_ascii_case(&manifest.network.subnet.bootstrap)
                && canonical_admin.eq_ignore_ascii_case(&manifest.contract.admin_peer_pubkey),
            "gateway peer identity differs from canonical mainnet pins"
        );
    }
    Ok(network)
}

pub async fn serve(bind: SocketAddr, state: GatewayState, lifecycle: ProxyLifecycle) -> Result<()> {
    let (shutdown, stopped) = watch::channel(false);
    let gateway = serve_with_shutdown(bind, state, wait_for_stop(stopped.clone()));
    let proxy = tokio::spawn(lifecycle.run(stopped));
    supervise(
        gateway,
        async move {
            proxy
                .await
                .context("proxy gateway task failed")?
                .map_err(Into::into)
        },
        stop_signal(),
        shutdown,
    )
    .await
}

async fn supervise<G, P, S>(
    gateway: G,
    proxy: P,
    signal: S,
    shutdown: watch::Sender<bool>,
) -> Result<()>
where
    G: Future<Output = io::Result<()>>,
    P: Future<Output = Result<()>>,
    S: Future<Output = Result<()>>,
{
    tokio::pin!(gateway, proxy, signal);
    let mut proxy_done = false;
    let mut gateway_done = false;
    let result = loop {
        tokio::select! {
            result = &mut gateway => { gateway_done = true; break result.map_err(Into::into); },
            result = &mut signal => break result,
            _ = &mut proxy, if !proxy_done => {
                proxy_done = true;
                // One sanitized notice. The handle retains the detailed bounded
                // health; failure does not terminate native request serving.
                eprintln!("Proxy discovery control stopped.");
            },
        }
    };
    shutdown.send_replace(true);
    while !proxy_done {
        tokio::select! {
            _ = &mut proxy => proxy_done = true,
            _ = &mut gateway, if !gateway_done => gateway_done = true,
        }
    }
    // Native streams never extend proxy cleanup indefinitely. Listener shutdown
    // is signalled and polled during that cleanup; returning retains the existing
    // process-stop behavior for streams still open, without a generation timeout.
    result
}

async fn wait_for_stop(mut stop: watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}

async fn stop_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { r = tokio::signal::ctrl_c() => r?, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
