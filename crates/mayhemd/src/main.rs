#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinSet;
use tokio::time::sleep;

const DEFAULT_BIND: &str = "127.0.0.1:11437";
const DEFAULT_CONFIG_FILE: &str = "config.toml";
const DEFAULT_PID_FILE: &str = "mayhemd.pid";
const DEFAULT_STATE_FILE: &str = "mayhemd-state.json";
const DEFAULT_RESTART_STABLE_AFTER_MS: u64 = 60_000;
const DEFAULT_CRASH_LOOP_THRESHOLD: u64 = 5;
const CONTROL_TOKEN_ENV: &str = "MAYHEMD_CONTROL_TOKEN";
const MIN_CONTROL_TOKEN_BYTES: usize = 32;
const MAX_STATUS_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_STATUS_REQUEST_BODY_BYTES: usize = 64 * 1024;
const STATUS_REQUEST_READ_CHUNK_BYTES: usize = 4 * 1024;

#[derive(Debug, Parser)]
#[command(name = "mayhemd")]
#[command(about = "Mayhem local process supervisor")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// TOML config file. Defaults to <home>/config.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Loopback status endpoint bind address.
    #[arg(long)]
    bind: Option<SocketAddr>,

    /// PID file path. Defaults to <home>/mayhemd.pid.
    #[arg(long, value_name = "PATH")]
    pid_file: Option<PathBuf>,

    /// State snapshot file path. Defaults to <home>/mayhemd-state.json.
    #[arg(long, value_name = "PATH")]
    state_file: Option<PathBuf>,

    /// Print an example supervisor config and exit.
    #[arg(long)]
    print_example_config: bool,

    /// Stop automatically after this many milliseconds. Useful for smoke tests.
    #[arg(long, hide = true)]
    exit_after_ms: Option<u64>,

    /// Print a machine-readable startup report.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileConfig {
    #[serde(default)]
    supervisor: SupervisorConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SupervisorConfig {
    #[serde(default = "default_bind_string")]
    bind: String,
    #[serde(default)]
    children: Vec<ChildConfig>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            bind: default_bind_string(),
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ChildConfig {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    restart: bool,
    #[serde(default = "default_backoff_ms")]
    restart_backoff_ms: u64,
    #[serde(default = "default_restart_stable_after_ms")]
    restart_stable_after_ms: u64,
    #[serde(default = "default_crash_loop_threshold")]
    crash_loop_threshold: u64,
}

#[derive(Clone, Debug, Serialize)]
struct SupervisorState {
    ok: bool,
    pid: u32,
    started_at_ms: u64,
    bind: String,
    config_path: Option<PathBuf>,
    pid_file: PathBuf,
    state_file: PathBuf,
    children: BTreeMap<String, ChildState>,
}

#[derive(Clone, Debug, Serialize)]
struct ChildState {
    name: String,
    command: String,
    args: Vec<String>,
    pid: Option<u32>,
    running: bool,
    restart: bool,
    restart_pending: bool,
    restarts: u64,
    consecutive_failures: u64,
    crash_loop: bool,
    started_at_ms: Option<u64>,
    stopped_at_ms: Option<u64>,
    last_uptime_ms: Option<u64>,
    last_exit: Option<String>,
    last_error: Option<String>,
}

const REDACTED_ARGUMENT: &str = "[REDACTED]";

fn sensitive_argument_flag(value: &str) -> bool {
    let normalized = value
        .trim_start_matches('-')
        .replace('_', "-")
        .to_ascii_lowercase();
    normalized == "password"
        || normalized.ends_with("-password")
        || normalized == "token"
        || normalized.ends_with("-token")
        || normalized == "secret"
        || normalized.ends_with("-secret")
        || normalized == "api-key"
        || normalized.ends_with("-api-key")
        || normalized == "private-key"
        || normalized.ends_with("-private-key")
        || normalized == "credential"
        || normalized.ends_with("-credential")
}

fn url_contains_credentials(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    let authority_start = scheme_end + 3;
    let suffix = &value[authority_start..];
    let authority_end = suffix
        .find(|ch| matches!(ch, '/' | '?' | '#'))
        .unwrap_or(suffix.len());
    if suffix[..authority_end].contains('@') {
        return true;
    }
    let Some((_, query)) = value.split_once('?') else {
        return false;
    };
    query.split('&').any(|field| {
        let name = field.split_once('=').map_or(field, |(name, _)| name);
        sensitive_argument_flag(name)
    })
}

fn redact_child_args(args: &[String]) -> Vec<String> {
    let mut redact_next = false;
    args.iter()
        .map(|arg| {
            if redact_next {
                redact_next = false;
                return REDACTED_ARGUMENT.to_owned();
            }
            if let Some((flag, _)) = arg.split_once('=') {
                if flag.starts_with('-') && sensitive_argument_flag(flag) {
                    return format!("{flag}={REDACTED_ARGUMENT}");
                }
            }
            if arg.starts_with('-') && sensitive_argument_flag(arg) {
                redact_next = true;
                return arg.clone();
            }
            if url_contains_credentials(arg) {
                return "[REDACTED_URL]".to_owned();
            }
            arg.clone()
        })
        .collect()
}

#[derive(Clone)]
struct SupervisorRuntime {
    state: Arc<Mutex<SupervisorState>>,
    state_file: PathBuf,
}

type SupervisorControlReply = oneshot::Sender<std::result::Result<serde_json::Value, String>>;

#[derive(Debug)]
enum SupervisorCommand {
    Add {
        child: ChildConfig,
        reply: SupervisorControlReply,
    },
    Remove {
        name: String,
        reply: SupervisorControlReply,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.print_example_config {
        println!("{}", example_config());
        return Ok(());
    }

    let home = args
        .home
        .clone()
        .map(Ok)
        .unwrap_or_else(default_home)
        .context("resolving Mayhem home")?;
    let home = absolutize(home)?;
    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;

    let config_path = args
        .config
        .clone()
        .map(absolutize)
        .transpose()?
        .or_else(|| {
            let path = home.join(DEFAULT_CONFIG_FILE);
            path.exists().then_some(path)
        });
    let file_config = read_config(config_path.as_deref())?;
    let bind = match args.bind {
        Some(bind) => bind,
        None => parse_bind(&file_config.supervisor.bind)?,
    };
    let control_token = read_control_token()?;
    validate_control_bind(bind, control_token.as_deref())?;
    let pid_file = args
        .pid_file
        .clone()
        .map(absolutize)
        .transpose()?
        .unwrap_or_else(|| home.join(DEFAULT_PID_FILE));
    let state_file = args
        .state_file
        .clone()
        .map(absolutize)
        .transpose()?
        .unwrap_or_else(|| home.join(DEFAULT_STATE_FILE));

    validate_children(&file_config.supervisor.children)?;
    let status_listener = bind_status_listener(bind).await?;
    write_pid_file(&pid_file)?;

    let started_at_ms = unix_epoch_millis()?;
    let state = SupervisorState {
        ok: true,
        pid: std::process::id(),
        started_at_ms,
        bind: bind.to_string(),
        config_path,
        pid_file: pid_file.clone(),
        state_file: state_file.clone(),
        children: initial_child_states(&file_config.supervisor.children),
    };
    let runtime = SupervisorRuntime {
        state: Arc::new(Mutex::new(state)),
        state_file,
    };
    runtime.persist_state().await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": true,
                "pid": std::process::id(),
                "bind": bind.to_string(),
                "pid_file": pid_file,
                "state_file": runtime.state_file,
                "children": file_config.supervisor.children.len(),
            }))?
        );
    } else {
        println!(
            "mayhemd supervising {} child process(es); status http://{}/status",
            file_config.supervisor.children.len(),
            bind
        );
    }

    let result = run_supervisor(
        file_config.supervisor.children,
        status_listener,
        runtime,
        control_token,
        args.exit_after_ms,
    )
    .await;
    let _ = fs::remove_file(&pid_file);
    result
}

async fn run_supervisor(
    children: Vec<ChildConfig>,
    status_listener: TcpListener,
    runtime: SupervisorRuntime,
    control_token: Option<String>,
    exit_after_ms: Option<u64>,
) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (control_tx, mut control_rx) = mpsc::channel(32);
    let mut tasks = JoinSet::new();
    let mut child_shutdowns = BTreeMap::<String, watch::Sender<bool>>::new();

    tasks.spawn(supervise_status_server(
        Arc::new(status_listener),
        runtime.clone(),
        shutdown_rx.clone(),
        control_tx,
        control_token.map(Arc::<str>::from),
    ));
    for child in children {
        spawn_supervised_child(child, &runtime, &mut tasks, &mut child_shutdowns)?;
    }

    let mut exit_sleep = exit_after_ms.map(|ms| Box::pin(sleep(Duration::from_millis(ms))));
    loop {
        tokio::select! {
            signal = wait_for_shutdown_signal() => {
                signal?;
                break;
            }
            _ = async {
                if let Some(sleep) = exit_sleep.as_mut() {
                    sleep.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                break;
            }
            command = control_rx.recv() => {
                let Some(command) = command else {
                    continue;
                };
                handle_supervisor_command(
                    command,
                    &runtime,
                    &mut tasks,
                    &mut child_shutdowns,
                )
                .await;
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(err))) => {
                        return Err(err).context("supervisor component failed");
                    }
                    Some(Err(err)) => {
                        bail!("supervisor component task panicked: {err}");
                    }
                    None => {}
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    for shutdown in child_shutdowns.values() {
        let _ = shutdown.send(true);
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(err) = result {
            eprintln!("mayhemd task failed: {err}");
        }
    }
    Ok(())
}

fn spawn_supervised_child(
    child: ChildConfig,
    runtime: &SupervisorRuntime,
    tasks: &mut JoinSet<Result<()>>,
    child_shutdowns: &mut BTreeMap<String, watch::Sender<bool>>,
) -> Result<()> {
    if child_shutdowns.contains_key(&child.name) {
        bail!("duplicate supervisor child name {}", child.name);
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    child_shutdowns.insert(child.name.clone(), shutdown_tx);
    tasks.spawn(supervise_child_component(
        child,
        runtime.clone(),
        shutdown_rx,
    ));
    Ok(())
}

async fn supervise_child_component(
    child: ChildConfig,
    runtime: SupervisorRuntime,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let component_backoff = Duration::from_millis(child.restart_backoff_ms.max(250));
    loop {
        let task = tokio::spawn(supervise_child(
            child.clone(),
            runtime.clone(),
            shutdown.clone(),
        ));
        let failure = match task.await {
            Ok(Ok(())) if *shutdown.borrow() => return Ok(()),
            Ok(Ok(())) if !child.restart => {
                while !*shutdown.borrow() {
                    if shutdown.changed().await.is_err() {
                        return Ok(());
                    }
                }
                return Ok(());
            }
            Ok(Ok(())) => "child supervisor stopped unexpectedly".to_owned(),
            Ok(Err(err)) => format!("child supervisor failed: {err:#}"),
            Err(err) => format!("child supervisor panicked: {err}"),
        };
        eprintln!(
            "mayhemd component fault for child {}: {}; restarting supervision",
            child.name, failure
        );
        if let Err(err) = runtime
            .mark_child_component_failure(&child.name, failure)
            .await
        {
            eprintln!(
                "mayhemd could not persist component fault for child {}: {err:#}",
                child.name
            );
        }
        tokio::select! {
            _ = sleep(component_backoff) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn handle_supervisor_command(
    command: SupervisorCommand,
    runtime: &SupervisorRuntime,
    tasks: &mut JoinSet<Result<()>>,
    child_shutdowns: &mut BTreeMap<String, watch::Sender<bool>>,
) {
    match command {
        SupervisorCommand::Add { child, reply } => {
            let result = async {
                validate_children(std::slice::from_ref(&child))?;
                if child_shutdowns.contains_key(&child.name) {
                    bail!("supervisor child {} already exists", child.name);
                }
                runtime.add_child_config(&child).await?;
                let name = child.name.clone();
                spawn_supervised_child(child, runtime, tasks, child_shutdowns)?;
                Ok(json!({ "ok": true, "name": name }))
            }
            .await
            .map_err(|err: anyhow::Error| err.to_string());
            let _ = reply.send(result);
        }
        SupervisorCommand::Remove { name, reply } => {
            let result = async {
                let stopping = if let Some(shutdown) = child_shutdowns.remove(&name) {
                    let _ = shutdown.send(true);
                    true
                } else {
                    false
                };
                let removed = runtime.remove_child_config(&name).await?;
                if stopping || removed {
                    Ok(json!({ "ok": true, "name": name, "stopping": stopping }))
                } else {
                    bail!("supervisor child {name} is not running or does not exist");
                }
            }
            .await
            .map_err(|err: anyhow::Error| err.to_string());
            let _ = reply.send(result);
        }
    }
}

async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("installing SIGTERM handler")?;
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("waiting for Ctrl-C shutdown signal")?;
            }
            _ = terminate.recv() => {}
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("waiting for Ctrl-C shutdown signal")
    }
}

async fn supervise_child(
    child: ChildConfig,
    runtime: SupervisorRuntime,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut restarts = 0_u64;
    let mut consecutive_failures = 0_u64;
    let initial_backoff = Duration::from_millis(child.restart_backoff_ms.max(250));
    let stable_after = Duration::from_millis(child.restart_stable_after_ms.max(1));
    let crash_loop_threshold = child.crash_loop_threshold.max(1);
    let mut backoff = initial_backoff;
    loop {
        if *shutdown.borrow() {
            runtime
                .mark_child_stopped(&child.name, "shutdown before start".to_owned(), None)
                .await?;
            return Ok(());
        }

        let mut command = Command::new(&child.command);
        command.args(&child.args);
        command.env_remove(CONTROL_TOKEN_ENV);
        command.stdin(Stdio::null());
        command.stdout(Stdio::inherit());
        command.stderr(Stdio::inherit());
        command.kill_on_drop(true);
        if let Some(cwd) = &child.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &child.env {
            command.env(key, value);
        }

        match command.spawn() {
            Ok(mut process) => {
                let pid = process.id();
                let started = std::time::Instant::now();
                runtime
                    .mark_child_started(&child, pid, restarts, consecutive_failures)
                    .await
                    .with_context(|| format!("updating child {} state", child.name))?;
                let stable_sleep = sleep(stable_after);
                tokio::pin!(stable_sleep);
                let mut stable = false;
                loop {
                    tokio::select! {
                        status = process.wait() => {
                            let uptime = started.elapsed();
                            let exit = match status {
                                Ok(status) => exit_status_string(status),
                                Err(err) => format!("wait failed: {err}"),
                            };
                            if stable || uptime >= stable_after {
                                consecutive_failures = 0;
                                backoff = initial_backoff;
                            }
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            restarts = restarts.saturating_add(1);
                            let crash_loop = consecutive_failures >= crash_loop_threshold;
                            eprintln!(
                                "mayhemd child {} exited ({exit}) after {}ms; restart #{restarts}, consecutive failure #{consecutive_failures}{}",
                                child.name,
                                duration_millis(uptime),
                                if crash_loop { " [CRASH LOOP]" } else { "" },
                            );
                            runtime
                                .mark_child_failed(
                                    &child.name,
                                    exit,
                                    None,
                                    duration_millis(uptime),
                                    restarts,
                                    consecutive_failures,
                                    crash_loop,
                                    child.restart && !*shutdown.borrow(),
                                )
                                .await?;
                            if !child.restart || *shutdown.borrow() {
                                return Ok(());
                            }
                            break;
                        }
                        _ = stable_sleep.as_mut(), if !stable => {
                            stable = true;
                            consecutive_failures = 0;
                            backoff = initial_backoff;
                            if let Err(err) = runtime.mark_child_stable(&child.name).await {
                                eprintln!("mayhemd could not persist stable state for child {}: {err:#}", child.name);
                            }
                        }
                        changed = shutdown.changed() => {
                            changed.context("watching supervisor shutdown")?;
                            if *shutdown.borrow() {
                                let _ = process.kill().await;
                                let _ = process.wait().await;
                                runtime
                                    .mark_child_stopped(&child.name, "killed by supervisor shutdown".to_owned(), None)
                                    .await?;
                                return Ok(());
                            }
                        }
                    }
                }
            }
            Err(err) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                restarts = restarts.saturating_add(1);
                let crash_loop = consecutive_failures >= crash_loop_threshold;
                eprintln!(
                    "mayhemd child {} spawn failed; restart #{restarts}, consecutive failure #{consecutive_failures}{}: {err}",
                    child.name,
                    if crash_loop { " [CRASH LOOP]" } else { "" },
                );
                runtime
                    .mark_child_failed(
                        &child.name,
                        "spawn failed".to_owned(),
                        Some(err.to_string()),
                        0,
                        restarts,
                        consecutive_failures,
                        crash_loop,
                        child.restart && !*shutdown.borrow(),
                    )
                    .await?;
                if !child.restart {
                    return Ok(());
                }
            }
        }

        tokio::select! {
            _ = sleep(backoff) => {}
            changed = shutdown.changed() => {
                changed.context("watching supervisor shutdown")?;
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn supervise_status_server(
    listener: Arc<TcpListener>,
    runtime: SupervisorRuntime,
    mut shutdown: watch::Receiver<bool>,
    control_tx: mpsc::Sender<SupervisorCommand>,
    control_token: Option<Arc<str>>,
) -> Result<()> {
    loop {
        let task = tokio::spawn(status_server(
            listener.clone(),
            runtime.clone(),
            shutdown.clone(),
            control_tx.clone(),
            control_token.clone(),
        ));
        let failure = match task.await {
            Ok(Ok(())) if *shutdown.borrow() => return Ok(()),
            Ok(Ok(())) => "status server stopped unexpectedly".to_owned(),
            Ok(Err(err)) => format!("status server failed: {err:#}"),
            Err(err) => format!("status server panicked: {err}"),
        };
        eprintln!("mayhemd critical component fault: {failure}; restarting status server");
        tokio::select! {
            _ = sleep(Duration::from_millis(250)) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn status_server(
    listener: Arc<TcpListener>,
    runtime: SupervisorRuntime,
    mut shutdown: watch::Receiver<bool>,
    control_tx: mpsc::Sender<SupervisorCommand>,
    control_token: Option<Arc<str>>,
) -> Result<()> {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accepting status connection")?;
                let runtime = runtime.clone();
                let control_tx = control_tx.clone();
                let control_token = control_token.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        handle_status_connection(stream, runtime, control_tx, control_token).await
                    {
                        eprintln!("mayhemd status request failed: {err}");
                    }
                });
            }
            changed = shutdown.changed() => {
                changed.context("watching status shutdown")?;
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn handle_status_connection(
    mut stream: TcpStream,
    runtime: SupervisorRuntime,
    control_tx: mpsc::Sender<SupervisorCommand>,
    control_token: Option<Arc<str>>,
) -> Result<()> {
    let request = match read_status_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            return write_http_json(
                &mut stream,
                error.status(),
                &json!({ "ok": false, "error": error.message() }),
            )
            .await;
        }
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => {
            let snapshot = runtime.snapshot().await;
            let (status, report) = supervisor_health_report(&snapshot);
            write_http_json(&mut stream, status, &report).await
        }
        ("GET", "/status") | ("GET", "/") => {
            let snapshot = runtime.snapshot().await;
            write_http_json(&mut stream, 200, &snapshot).await
        }
        ("POST", "/children/add") => {
            if !control_request_authorized(&request.headers, control_token.as_deref()) {
                return write_http_json(
                    &mut stream,
                    401,
                    &json!({ "ok": false, "error": "unauthorized" }),
                )
                .await;
            }
            let child = match serde_json::from_slice::<ChildConfig>(&request.body) {
                Ok(child) => child,
                Err(_) => {
                    return write_http_json(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": "malformed JSON body" }),
                    )
                    .await;
                }
            };
            let (reply, response) = oneshot::channel();
            control_tx
                .send(SupervisorCommand::Add { child, reply })
                .await
                .context("sending child add command")?;
            match response.await.context("waiting for child add response")? {
                Ok(body) => write_http_json(&mut stream, 200, &body).await,
                Err(error) => {
                    write_http_json(&mut stream, 409, &json!({ "ok": false, "error": error })).await
                }
            }
        }
        ("POST", "/children/remove") => {
            if !control_request_authorized(&request.headers, control_token.as_deref()) {
                return write_http_json(
                    &mut stream,
                    401,
                    &json!({ "ok": false, "error": "unauthorized" }),
                )
                .await;
            }
            let body = match serde_json::from_slice::<serde_json::Value>(&request.body) {
                Ok(body) => body,
                Err(_) => {
                    return write_http_json(
                        &mut stream,
                        400,
                        &json!({ "ok": false, "error": "malformed JSON body" }),
                    )
                    .await;
                }
            };
            let name = body
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let Some(name) = name else {
                return write_http_json(
                    &mut stream,
                    400,
                    &json!({ "ok": false, "error": "child remove request requires name" }),
                )
                .await;
            };
            let name = name.to_owned();
            let (reply, response) = oneshot::channel();
            control_tx
                .send(SupervisorCommand::Remove { name, reply })
                .await
                .context("sending child remove command")?;
            match response
                .await
                .context("waiting for child remove response")?
            {
                Ok(body) => write_http_json(&mut stream, 200, &body).await,
                Err(error) => {
                    write_http_json(&mut stream, 404, &json!({ "ok": false, "error": error })).await
                }
            }
        }
        _ => {
            write_http_json(
                &mut stream,
                404,
                &json!({ "ok": false, "error": "not found" }),
            )
            .await
        }
    }
}

struct StatusRequest {
    method: String,
    path: String,
    headers: String,
    body: Vec<u8>,
}

#[derive(Debug)]
enum StatusRequestError {
    Malformed,
    HeadersTooLarge,
    BodyTooLarge,
    Truncated,
    Io,
}

impl StatusRequestError {
    fn status(&self) -> u16 {
        match self {
            Self::BodyTooLarge => 413,
            Self::HeadersTooLarge => 431,
            Self::Malformed | Self::Truncated | Self::Io => 400,
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Malformed => "malformed HTTP request",
            Self::HeadersTooLarge => "HTTP request headers are too large",
            Self::BodyTooLarge => "HTTP request body is too large",
            Self::Truncated => "truncated HTTP request",
            Self::Io => "failed to read HTTP request",
        }
    }
}

async fn read_status_request(
    stream: &mut TcpStream,
) -> std::result::Result<StatusRequest, StatusRequestError> {
    let mut received = Vec::with_capacity(STATUS_REQUEST_READ_CHUNK_BYTES);
    let mut chunk = [0_u8; STATUS_REQUEST_READ_CHUNK_BYTES];
    let header_end = loop {
        let remaining = MAX_STATUS_REQUEST_HEADER_BYTES.saturating_sub(received.len());
        if remaining == 0 {
            return Err(StatusRequestError::HeadersTooLarge);
        }
        let read_limit = remaining.min(chunk.len());
        let read = stream
            .read(&mut chunk[..read_limit])
            .await
            .map_err(|_| StatusRequestError::Io)?;
        if read == 0 {
            return Err(StatusRequestError::Truncated);
        }
        received.extend_from_slice(&chunk[..read]);
        if let Some(offset) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break offset + 4;
        }
    };

    let header_bytes = &received[..header_end - 4];
    if !header_bytes.is_ascii() {
        return Err(StatusRequestError::Malformed);
    }
    let headers = std::str::from_utf8(header_bytes)
        .map_err(|_| StatusRequestError::Malformed)?
        .to_owned();
    let mut lines = headers.split("\r\n");
    let request_line = lines.next().ok_or(StatusRequestError::Malformed)?;
    let mut request_parts = request_line.split_ascii_whitespace();
    let method = request_parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(StatusRequestError::Malformed)?;
    let path = request_parts
        .next()
        .filter(|value| value.starts_with('/'))
        .ok_or(StatusRequestError::Malformed)?;
    let version = request_parts.next().ok_or(StatusRequestError::Malformed)?;
    if request_parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(StatusRequestError::Malformed);
    }

    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(StatusRequestError::Malformed)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(StatusRequestError::Malformed);
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(StatusRequestError::Malformed);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.trim().is_empty()
                || !value.trim().bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(StatusRequestError::Malformed);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| StatusRequestError::Malformed)?,
            );
        }
    }
    if method == "POST" && content_length.is_none() {
        return Err(StatusRequestError::Malformed);
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_STATUS_REQUEST_BODY_BYTES {
        return Err(StatusRequestError::BodyTooLarge);
    }
    let method = method.to_owned();
    let path = path.to_owned();

    let mut body = received.split_off(header_end);
    body.truncate(content_length);
    while body.len() < content_length {
        let remaining = content_length - body.len();
        let read_limit = remaining.min(chunk.len());
        let read = stream
            .read(&mut chunk[..read_limit])
            .await
            .map_err(|_| StatusRequestError::Io)?;
        if read == 0 {
            return Err(StatusRequestError::Truncated);
        }
        body.extend_from_slice(&chunk[..read]);
    }

    Ok(StatusRequest {
        method,
        path,
        headers,
        body,
    })
}

async fn write_http_json<T: Serialize>(
    stream: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let bytes = serde_json::to_vec_pretty(body)?;
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.shutdown().await?;
    Ok(())
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn recompute_supervisor_health(state: &mut SupervisorState) {
    state.ok = state.children.values().all(|child| !child.crash_loop);
}

fn supervisor_health_report(state: &SupervisorState) -> (u16, serde_json::Value) {
    let crash_loop_children = state
        .children
        .values()
        .filter(|child| child.crash_loop)
        .map(|child| child.name.clone())
        .collect::<Vec<_>>();
    let status = if state.ok { 200 } else { 503 };
    (
        status,
        json!({
            "ok": state.ok,
            "crash_loop_children": crash_loop_children,
        }),
    )
}

impl SupervisorRuntime {
    async fn snapshot(&self) -> SupervisorState {
        self.state.lock().await.clone()
    }

    async fn persist_state(&self) -> Result<()> {
        let snapshot = self.snapshot().await;
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(&snapshot)?;
        write_private_file(&self.state_file, &bytes)
            .with_context(|| format!("writing {}", self.state_file.display()))?;
        Ok(())
    }

    async fn mark_child_started(
        &self,
        child: &ChildConfig,
        pid: Option<u32>,
        restarts: u64,
        consecutive_failures: u64,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            let entry = state
                .children
                .entry(child.name.clone())
                .or_insert_with(|| child.initial_state());
            entry.pid = pid;
            entry.running = true;
            entry.restart_pending = false;
            entry.restarts = restarts;
            entry.consecutive_failures = consecutive_failures;
            entry.crash_loop = consecutive_failures >= child.crash_loop_threshold.max(1);
            entry.started_at_ms = Some(unix_epoch_millis()?);
            entry.stopped_at_ms = None;
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    async fn mark_child_stopped(
        &self,
        name: &str,
        exit: String,
        error: Option<String>,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.children.get_mut(name) {
                entry.pid = None;
                entry.running = false;
                entry.restart_pending = false;
                entry.stopped_at_ms = Some(unix_epoch_millis()?);
                entry.last_exit = Some(exit);
                entry.last_error = error;
                entry.crash_loop = false;
            }
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mark_child_failed(
        &self,
        name: &str,
        exit: String,
        error: Option<String>,
        uptime_ms: u64,
        restarts: u64,
        consecutive_failures: u64,
        crash_loop: bool,
        restart_pending: bool,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.children.get_mut(name) {
                entry.pid = None;
                entry.running = false;
                entry.restart_pending = restart_pending;
                entry.restarts = restarts;
                entry.consecutive_failures = consecutive_failures;
                entry.crash_loop = crash_loop;
                entry.stopped_at_ms = Some(unix_epoch_millis()?);
                entry.last_uptime_ms = Some(uptime_ms);
                entry.last_exit = Some(exit);
                entry.last_error = error;
            }
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    async fn mark_child_stable(&self, name: &str) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.children.get_mut(name) {
                entry.consecutive_failures = 0;
                entry.crash_loop = false;
            }
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    async fn mark_child_component_failure(&self, name: &str, error: String) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.children.get_mut(name) {
                entry.pid = None;
                entry.running = false;
                entry.restart_pending = true;
                entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
                entry.crash_loop = true;
                entry.stopped_at_ms = Some(unix_epoch_millis()?);
                entry.last_exit = Some("supervisor component fault".to_owned());
                entry.last_error = Some(error);
            }
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    async fn add_child_config(&self, child: &ChildConfig) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if state.children.contains_key(&child.name) {
                bail!("supervisor child {} already exists", child.name);
            }
            state
                .children
                .insert(child.name.clone(), child.initial_state());
            recompute_supervisor_health(&mut state);
        }
        self.persist_state().await
    }

    async fn remove_child_config(&self, name: &str) -> Result<bool> {
        let removed = {
            let mut state = self.state.lock().await;
            let removed = state.children.remove(name).is_some();
            recompute_supervisor_health(&mut state);
            removed
        };
        self.persist_state().await?;
        Ok(removed)
    }
}

impl ChildConfig {
    fn initial_state(&self) -> ChildState {
        ChildState {
            name: self.name.clone(),
            command: self.command.clone(),
            args: redact_child_args(&self.args),
            pid: None,
            running: false,
            restart: self.restart,
            restart_pending: false,
            restarts: 0,
            consecutive_failures: 0,
            crash_loop: false,
            started_at_ms: None,
            stopped_at_ms: None,
            last_uptime_ms: None,
            last_exit: None,
            last_error: None,
        }
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)?;
        Ok(())
    }
}

fn initial_child_states(children: &[ChildConfig]) -> BTreeMap<String, ChildState> {
    children
        .iter()
        .map(|child| (child.name.clone(), child.initial_state()))
        .collect()
}

fn validate_children(children: &[ChildConfig]) -> Result<()> {
    let mut names = BTreeMap::<&str, usize>::new();
    for child in children {
        if child.name.trim().is_empty() {
            bail!("supervisor child name must not be empty");
        }
        if child.command.trim().is_empty() {
            bail!("supervisor child {} command must not be empty", child.name);
        }
        if child.restart_stable_after_ms == 0 {
            bail!(
                "supervisor child {} restart_stable_after_ms must be positive",
                child.name
            );
        }
        if child.crash_loop_threshold == 0 {
            bail!(
                "supervisor child {} crash_loop_threshold must be positive",
                child.name
            );
        }
        if names.insert(child.name.as_str(), 1).is_some() {
            bail!("duplicate supervisor child name {}", child.name);
        }
    }
    Ok(())
}

fn read_config(path: Option<&Path>) -> Result<FileConfig> {
    let Some(path) = path else {
        return Ok(FileConfig {
            supervisor: SupervisorConfig::default(),
        });
    };
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn write_pid_file(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, format!("{}\n", std::process::id()))
        .with_context(|| format!("writing {}", path.display()))
}

fn exit_status_string(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "terminated by signal".to_owned(),
    }
}

async fn bind_status_listener(bind: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding status endpoint on {bind}"))
}

fn parse_bind(value: &str) -> Result<SocketAddr> {
    value
        .parse()
        .with_context(|| format!("parsing supervisor bind address {value:?}"))
}

fn read_control_token() -> Result<Option<String>> {
    let Some(token) = env::var(CONTROL_TOKEN_ENV).ok() else {
        return Ok(None);
    };
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() < MIN_CONTROL_TOKEN_BYTES {
        bail!("{CONTROL_TOKEN_ENV} must contain at least {MIN_CONTROL_TOKEN_BYTES} bytes");
    }
    if token.len() > 1_024 || token.chars().any(char::is_control) {
        bail!("{CONTROL_TOKEN_ENV} is invalid");
    }
    Ok(Some(token.to_owned()))
}

fn validate_control_bind(bind: SocketAddr, control_token: Option<&str>) -> Result<()> {
    if bind.ip().is_loopback() || control_token.is_some() {
        return Ok(());
    }
    bail!(
        "refusing non-loopback supervisor bind {bind} without {CONTROL_TOKEN_ENV}; status may be exposed remotely only with authenticated control"
    )
}

fn control_request_authorized(request: &str, expected_token: Option<&str>) -> bool {
    let Some(expected_token) = expected_token else {
        return false;
    };
    let Some(header) = request
        .split("\r\n\r\n")
        .next()
        .unwrap_or(request)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim())
    else {
        return false;
    };
    let Some((scheme, supplied_token)) = header.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && constant_time_token_eq(supplied_token.trim().as_bytes(), expected_token.as_bytes())
}

fn constant_time_token_eq(supplied: &[u8], expected: &[u8]) -> bool {
    let mut difference = supplied.len() ^ expected.len();
    let max_len = supplied.len().max(expected.len());
    for index in 0..max_len {
        let supplied_byte = supplied.get(index).copied().unwrap_or(0);
        let expected_byte = expected.get(index).copied().unwrap_or(0);
        difference |= usize::from(supplied_byte ^ expected_byte);
    }
    difference == 0
}

fn default_home() -> Result<PathBuf> {
    if let Ok(home) = env::var("MAYHEM_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    let home = env::var("HOME").context("HOME is not set; pass --home")?;
    Ok(PathBuf::from(home).join(".mayhem"))
}

fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(env::current_dir()?.join(path))
}

fn unix_epoch_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("Unix epoch milliseconds overflowed u64")
}

fn default_bind_string() -> String {
    DEFAULT_BIND.to_owned()
}

fn default_true() -> bool {
    true
}

fn default_backoff_ms() -> u64 {
    1_000
}

fn default_restart_stable_after_ms() -> u64 {
    DEFAULT_RESTART_STABLE_AFTER_MS
}

fn default_crash_loop_threshold() -> u64 {
    DEFAULT_CRASH_LOOP_THRESHOLD
}

fn example_config() -> &'static str {
    r#"[supervisor]
bind = "127.0.0.1:11437"

[[supervisor.children]]
name = "gateway"
command = "mayhem"
args = ["use", "--home", "/absolute/path/to/.mayhem"]
restart = true
restart_backoff_ms = 1000
restart_stable_after_ms = 60000
crash_loop_threshold = 5

[[supervisor.children]]
name = "paygate"
command = "mayhem-paygate"
args = ["--config", "/absolute/path/to/paygate.toml"]
restart = true
restart_backoff_ms = 1000
restart_stable_after_ms = 60000
crash_loop_threshold = 5
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_runtime(temp: &Path, children: &[ChildConfig]) -> SupervisorRuntime {
        SupervisorRuntime {
            state: Arc::new(Mutex::new(SupervisorState {
                ok: true,
                pid: std::process::id(),
                started_at_ms: unix_epoch_millis().unwrap(),
                bind: "127.0.0.1:0".to_owned(),
                config_path: None,
                pid_file: temp.join("mayhemd.pid"),
                state_file: temp.join("mayhemd-state.json"),
                children: initial_child_states(children),
            })),
            state_file: temp.join("mayhemd-state.json"),
        }
    }

    async fn wait_for_test_state(
        timeout: Duration,
        mut ready: impl FnMut(&SupervisorState) -> bool,
        runtime: &SupervisorRuntime,
    ) -> SupervisorState {
        tokio::time::timeout(timeout, async {
            loop {
                let snapshot = runtime.snapshot().await;
                if ready(&snapshot) {
                    return snapshot;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("supervisor state condition should become true")
    }

    #[test]
    fn parses_supervisor_children_from_toml() {
        let config: FileConfig = toml::from_str(
            r#"
            [supervisor]
            bind = "127.0.0.1:19001"

            [[supervisor.children]]
            name = "gateway"
            command = "mayhem"
            args = ["use", "--home", "/tmp/mayhem"]
            restart = true
            restart_backoff_ms = 250
            "#,
        )
        .unwrap();
        assert_eq!(config.supervisor.bind, "127.0.0.1:19001");
        assert_eq!(config.supervisor.children.len(), 1);
        assert_eq!(config.supervisor.children[0].name, "gateway");
        assert_eq!(config.supervisor.children[0].args[0], "use");
        validate_children(&config.supervisor.children).unwrap();
    }

    #[test]
    fn rejects_duplicate_child_names() {
        let children = vec![
            ChildConfig {
                name: "gateway".to_owned(),
                command: "true".to_owned(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                restart: true,
                restart_backoff_ms: 1_000,
                restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
                crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
            },
            ChildConfig {
                name: "gateway".to_owned(),
                command: "true".to_owned(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                restart: true,
                restart_backoff_ms: 1_000,
                restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
                crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
            },
        ];
        assert!(validate_children(&children).is_err());
    }

    #[test]
    fn rejects_invalid_bind_address() {
        assert!(parse_bind("not-a-socket").is_err());
    }

    #[test]
    fn remote_control_bind_requires_an_explicit_auth_secret() {
        let loopback = "127.0.0.1:11437".parse().unwrap();
        let remote = "0.0.0.0:11437".parse().unwrap();
        assert!(validate_control_bind(loopback, None).is_ok());
        assert!(validate_control_bind(remote, None).is_err());
        assert!(validate_control_bind(remote, Some("configured-token")).is_ok());
    }

    #[test]
    fn control_auth_requires_the_exact_bearer_token() {
        let expected = "test-control-token-0123456789abcdef";
        assert!(control_request_authorized(
            "POST /children/add HTTP/1.1\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\n\r\n",
            Some(expected),
        ));
        assert!(!control_request_authorized(
            "POST /children/add HTTP/1.1\r\n\r\n",
            Some(expected),
        ));
        assert!(!control_request_authorized(
            "POST /children/add HTTP/1.1\r\nAuthorization: Bearer wrong-token\r\n\r\n",
            Some(expected),
        ));
        assert!(!control_request_authorized(
            "POST /children/add HTTP/1.1\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\n\r\n",
            None,
        ));
    }

    async fn send_control_request(
        runtime: SupervisorRuntime,
        control_tx: mpsc::Sender<SupervisorCommand>,
        control_token: Option<&str>,
        request: &str,
    ) -> String {
        send_fragmented_control_request(
            runtime,
            control_tx,
            control_token,
            vec![request.as_bytes().to_vec()],
        )
        .await
    }

    async fn send_fragmented_control_request(
        runtime: SupervisorRuntime,
        control_tx: mpsc::Sender<SupervisorCommand>,
        control_token: Option<&str>,
        fragments: Vec<Vec<u8>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let token = control_token.map(|value| Arc::<str>::from(value.to_owned()));
        let task = tokio::spawn(handle_status_connection(server, runtime, control_tx, token));
        let fragment_count = fragments.len();
        for (index, fragment) in fragments.into_iter().enumerate() {
            client.write_all(&fragment).await.unwrap();
            if index + 1 < fragment_count {
                sleep(Duration::from_millis(20)).await;
            }
        }
        client.shutdown().await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        response
    }

    #[tokio::test]
    async fn unauthenticated_child_add_is_rejected_before_dispatch() {
        let temp = env::temp_dir().join(format!("mayhemd-auth-reject-test-{}", std::process::id()));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let body = r#"{"name":"attacker","command":"sh","args":["-c","exit 0"]}"#;
        let request = format!(
            "POST /children/add HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = send_control_request(
            runtime,
            control_tx,
            Some("test-control-token-0123456789abcdef"),
            &request,
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), control_rx.recv())
                .await
                .expect("control channel should close without dispatch")
                .is_none()
        );
    }

    #[tokio::test]
    async fn fragmented_authenticated_child_add_reaches_the_supervisor() {
        let temp = env::temp_dir().join(format!(
            "mayhemd-fragmented-add-test-{}",
            std::process::id()
        ));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let body = r#"{"name":"worker","command":"true","restart":false}"#;
        let headers = format!(
            "POST /children/add HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let body_split = body.len() / 2;
        let fragments = vec![
            headers.into_bytes(),
            body.as_bytes()[..body_split].to_vec(),
            body.as_bytes()[body_split..].to_vec(),
        ];
        let response_task = tokio::spawn(async move {
            send_fragmented_control_request(
                runtime,
                control_tx,
                Some("test-control-token-0123456789abcdef"),
                fragments,
            )
            .await
        });
        let command = control_rx.recv().await.expect("authenticated command");
        let SupervisorCommand::Add { child, reply } = command else {
            panic!("expected child add command");
        };
        assert_eq!(child.name, "worker");
        reply
            .send(Ok(json!({ "ok": true, "name": child.name })))
            .unwrap();
        let response = response_task.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""name": "worker""#));
    }

    #[tokio::test]
    async fn fragmented_authenticated_child_remove_reaches_the_supervisor() {
        let temp = env::temp_dir().join(format!(
            "mayhemd-fragmented-remove-test-{}",
            std::process::id()
        ));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let body = r#"{"name":"worker"}"#;
        let headers = format!(
            "POST /children/remove HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let fragments = vec![
            headers.into_bytes(),
            body.as_bytes()[..5].to_vec(),
            body.as_bytes()[5..].to_vec(),
        ];
        let response_task = tokio::spawn(async move {
            send_fragmented_control_request(
                runtime,
                control_tx,
                Some("test-control-token-0123456789abcdef"),
                fragments,
            )
            .await
        });
        let command = control_rx.recv().await.expect("authenticated command");
        let SupervisorCommand::Remove { name, reply } = command else {
            panic!("expected child remove command");
        };
        assert_eq!(name, "worker");
        reply.send(Ok(json!({ "ok": true, "name": name }))).unwrap();
        let response = response_task.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""name": "worker""#));
    }

    #[tokio::test]
    async fn normal_get_status_request_still_succeeds() {
        let temp = env::temp_dir().join(format!("mayhemd-get-test-{}", std::process::id()));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, _control_rx) = mpsc::channel(1);
        let response = send_control_request(
            runtime,
            control_tx,
            None,
            "GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""ok": true"#));
    }

    #[tokio::test]
    async fn truncated_control_request_body_is_rejected() {
        let temp = env::temp_dir().join(format!("mayhemd-truncated-test-{}", std::process::id()));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let request = "POST /children/remove HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\nContent-Length: 17\r\n\r\n{}";
        let response = send_control_request(
            runtime,
            control_tx,
            Some("test-control-token-0123456789abcdef"),
            request,
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(response.contains("truncated HTTP request"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), control_rx.recv())
                .await
                .expect("control channel should close without dispatch")
                .is_none()
        );
    }

    #[tokio::test]
    async fn oversized_control_request_body_is_rejected() {
        let temp = env::temp_dir().join(format!("mayhemd-oversized-test-{}", std::process::id()));
        let runtime = test_runtime(&temp, &[]);
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let request = format!(
            "POST /children/add HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-control-token-0123456789abcdef\r\nContent-Length: {}\r\n\r\n",
            MAX_STATUS_REQUEST_BODY_BYTES + 1
        );
        let response = send_control_request(
            runtime,
            control_tx,
            Some("test-control-token-0123456789abcdef"),
            &request,
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 413 Content Too Large"));
        assert!(response.contains("HTTP request body is too large"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), control_rx.recv())
                .await
                .expect("control channel should close without dispatch")
                .is_none()
        );
    }

    #[test]
    fn initial_state_marks_children_not_running() {
        let child = ChildConfig {
            name: "peer".to_owned(),
            command: "pear".to_owned(),
            args: vec!["run".to_owned(), "intercom".to_owned()],
            cwd: None,
            env: BTreeMap::new(),
            restart: true,
            restart_backoff_ms: 1_000,
            restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
            crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
        };
        let states = initial_child_states(&[child]);
        let state = states.get("peer").unwrap();
        assert!(!state.running);
        assert_eq!(state.command, "pear");
        assert_eq!(state.args, ["run", "intercom"]);
    }

    #[test]
    fn supervisor_state_redacts_credentials_without_changing_launch_config() {
        let child = ChildConfig {
            name: "peer".to_owned(),
            command: "pear-runtime".to_owned(),
            args: vec![
                "run".to_owned(),
                ".".to_owned(),
                "--sc-bridge-token".to_owned(),
                "bridge-secret".to_owned(),
                "--wallet-password=hunter2".to_owned(),
                "https://user:pass@example.test/path".to_owned(),
                "https://example.test/path?api_key=query-secret".to_owned(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            restart: true,
            restart_backoff_ms: 1_000,
            restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
            crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
        };

        let state = child.initial_state();
        assert_eq!(child.args[3], "bridge-secret");
        assert_eq!(state.args[3], REDACTED_ARGUMENT);
        assert_eq!(state.args[4], "--wallet-password=[REDACTED]");
        assert_eq!(state.args[5], "[REDACTED_URL]");
        assert_eq!(state.args[6], "[REDACTED_URL]");
        let serialized = serde_json::to_string(&state).unwrap();
        for secret in ["bridge-secret", "hunter2", "user:pass", "query-secret"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_supervisor_state_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp =
            env::temp_dir().join(format!("mayhemd-private-state-test-{}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let runtime = test_runtime(&temp, &[]);
        fs::write(&runtime.state_file, b"stale").unwrap();
        fs::set_permissions(&runtime.state_file, fs::Permissions::from_mode(0o644)).unwrap();

        runtime.persist_state().await.unwrap();

        let mode = fs::metadata(&runtime.state_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_dir_all(temp).unwrap();
    }

    fn long_running_test_child(name: &str) -> ChildConfig {
        #[cfg(windows)]
        {
            ChildConfig {
                name: name.to_owned(),
                command: "cmd".to_owned(),
                args: vec!["/C".to_owned(), "ping 127.0.0.1 -n 6 > nul".to_owned()],
                cwd: None,
                env: BTreeMap::new(),
                restart: false,
                restart_backoff_ms: 250,
                restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
                crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
            }
        }
        #[cfg(not(windows))]
        {
            ChildConfig {
                name: name.to_owned(),
                command: "sh".to_owned(),
                args: vec!["-c".to_owned(), "sleep 5".to_owned()],
                cwd: None,
                env: BTreeMap::new(),
                restart: false,
                restart_backoff_ms: 250,
                restart_stable_after_ms: DEFAULT_RESTART_STABLE_AFTER_MS,
                crash_loop_threshold: DEFAULT_CRASH_LOOP_THRESHOLD,
            }
        }
    }

    #[tokio::test]
    async fn supervisor_control_add_remove_child_updates_state() {
        let temp = env::temp_dir().join(format!("mayhemd-control-test-{}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let runtime = test_runtime(&temp, &[]);
        let mut tasks = JoinSet::new();
        let mut child_shutdowns = BTreeMap::new();

        let (reply, response) = oneshot::channel();
        handle_supervisor_command(
            SupervisorCommand::Add {
                child: long_running_test_child("provider-live-test"),
                reply,
            },
            &runtime,
            &mut tasks,
            &mut child_shutdowns,
        )
        .await;
        assert!(response.await.unwrap().unwrap()["ok"].as_bool().unwrap());
        assert!(runtime
            .snapshot()
            .await
            .children
            .contains_key("provider-live-test"));
        assert!(child_shutdowns.contains_key("provider-live-test"));

        let (reply, response) = oneshot::channel();
        handle_supervisor_command(
            SupervisorCommand::Remove {
                name: "provider-live-test".to_owned(),
                reply,
            },
            &runtime,
            &mut tasks,
            &mut child_shutdowns,
        )
        .await;
        assert!(response.await.unwrap().unwrap()["ok"].as_bool().unwrap());
        assert!(!runtime
            .snapshot()
            .await
            .children
            .contains_key("provider-live-test"));
        assert!(!child_shutdowns.contains_key("provider-live-test"));

        let joined = tokio::time::timeout(Duration::from_secs(3), tasks.join_next())
            .await
            .expect("removed child task should stop");
        assert!(joined.expect("child task result").is_ok());
        let _ = fs::remove_dir_all(temp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn crashed_provider_command_restarts_full_load_and_heartbeat_lifecycle() {
        let temp = env::temp_dir().join(format!(
            "mayhemd-reload-test-{}-{}",
            std::process::id(),
            unix_epoch_millis().unwrap()
        ));
        fs::create_dir_all(&temp).unwrap();
        let script = r#"
count_file="$1/count"
events_file="$1/events"
count=0
if [ -f "$count_file" ]; then count=$(cat "$count_file"); fi
count=$((count + 1))
printf '%s\n' "$count" > "$count_file"
printf 'load-%s\nheartbeat-%s\n' "$count" "$count" >> "$events_file"
if [ "$count" -eq 1 ]; then exit 71; fi
trap 'exit 0' TERM INT
while true; do sleep 1; done
"#;
        let child = ChildConfig {
            name: "provider".to_owned(),
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                script.to_owned(),
                "mayhemd-provider-test".to_owned(),
                temp.display().to_string(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            restart: true,
            restart_backoff_ms: 25,
            restart_stable_after_ms: 5_000,
            crash_loop_threshold: 3,
        };
        let runtime = test_runtime(&temp, std::slice::from_ref(&child));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(supervise_child_component(
            child,
            runtime.clone(),
            shutdown_rx,
        ));

        let snapshot = wait_for_test_state(
            Duration::from_secs(5),
            |state| {
                fs::read_to_string(temp.join("events"))
                    .map(|events| events.contains("load-2") && events.contains("heartbeat-2"))
                    .unwrap_or(false)
                    && state.children["provider"].running
            },
            &runtime,
        )
        .await;
        let provider = &snapshot.children["provider"];
        assert_eq!(provider.restarts, 1);
        assert!(!provider.crash_loop);
        assert!(snapshot.ok);
        let (health_status, health) = supervisor_health_report(&snapshot);
        assert_eq!(health_status, 200);
        assert_eq!(health["ok"], json!(true));

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("provider supervisor should stop")
            .expect("provider supervisor task should join")
            .expect("provider supervisor should finish cleanly");
        let _ = fs::remove_dir_all(temp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_child_failures_surface_a_crash_loop() {
        let temp = env::temp_dir().join(format!(
            "mayhemd-crash-loop-test-{}-{}",
            std::process::id(),
            unix_epoch_millis().unwrap()
        ));
        fs::create_dir_all(&temp).unwrap();
        let child = ChildConfig {
            name: "provider".to_owned(),
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), "exit 73".to_owned()],
            cwd: None,
            env: BTreeMap::new(),
            restart: true,
            restart_backoff_ms: 25,
            restart_stable_after_ms: 5_000,
            crash_loop_threshold: 2,
        };
        let runtime = test_runtime(&temp, std::slice::from_ref(&child));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(supervise_child_component(
            child,
            runtime.clone(),
            shutdown_rx,
        ));

        let snapshot = wait_for_test_state(
            Duration::from_secs(5),
            |state| state.children["provider"].crash_loop,
            &runtime,
        )
        .await;
        let provider = &snapshot.children["provider"];
        assert!(provider.restarts >= 2);
        assert!(provider.consecutive_failures >= 2);
        assert_eq!(provider.last_exit.as_deref(), Some("exit 73"));
        assert!(!snapshot.ok);
        let (health_status, health) = supervisor_health_report(&snapshot);
        assert_eq!(health_status, 503);
        assert_eq!(health["crash_loop_children"], json!(["provider"]));

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("crash-loop supervisor should stop")
            .expect("crash-loop supervisor task should join")
            .expect("crash-loop supervisor should finish cleanly");
        let _ = fs::remove_dir_all(temp);
    }
}
