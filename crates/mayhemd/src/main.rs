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
use tokio::sync::{watch, Mutex};
use tokio::task::JoinSet;
use tokio::time::sleep;

const DEFAULT_BIND: &str = "127.0.0.1:11437";
const DEFAULT_CONFIG_FILE: &str = "config.toml";
const DEFAULT_PID_FILE: &str = "mayhemd.pid";
const DEFAULT_STATE_FILE: &str = "mayhemd-state.json";

#[derive(Debug, Parser)]
#[command(name = "mayhemd")]
#[command(about = "Mayhem local process supervisor")]
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
    restarts: u64,
    started_at_ms: Option<u64>,
    stopped_at_ms: Option<u64>,
    last_exit: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone)]
struct SupervisorRuntime {
    state: Arc<Mutex<SupervisorState>>,
    state_file: PathBuf,
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
    exit_after_ms: Option<u64>,
) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks = JoinSet::new();

    tasks.spawn(status_server(
        status_listener,
        runtime.clone(),
        shutdown_rx.clone(),
    ));
    for child in children {
        tasks.spawn(supervise_child(child, runtime.clone(), shutdown_rx.clone()));
    }

    tokio::select! {
        signal = wait_for_shutdown_signal() => {
            signal?;
        }
        _ = async {
            if let Some(ms) = exit_after_ms {
                sleep(Duration::from_millis(ms)).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {}
    }

    let _ = shutdown_tx.send(true);
    while let Some(result) = tasks.join_next().await {
        if let Err(err) = result {
            eprintln!("mayhemd task failed: {err}");
        }
    }
    Ok(())
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
    let mut backoff = Duration::from_millis(child.restart_backoff_ms.max(250));
    loop {
        if *shutdown.borrow() {
            runtime
                .mark_child_stopped(&child.name, "shutdown before start".to_owned(), None)
                .await?;
            return Ok(());
        }

        let mut command = Command::new(&child.command);
        command.args(&child.args);
        command.stdin(Stdio::null());
        command.stdout(Stdio::inherit());
        command.stderr(Stdio::inherit());
        if let Some(cwd) = &child.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &child.env {
            command.env(key, value);
        }

        match command.spawn() {
            Ok(mut process) => {
                let pid = process.id();
                runtime
                    .mark_child_started(&child, pid, restarts)
                    .await
                    .with_context(|| format!("updating child {} state", child.name))?;
                tokio::select! {
                    status = process.wait() => {
                        let status = status.with_context(|| format!("waiting for child {}", child.name))?;
                        let exit = exit_status_string(status);
                        runtime
                            .mark_child_stopped(&child.name, exit.clone(), None)
                            .await?;
                        if !child.restart || *shutdown.borrow() {
                            return Ok(());
                        }
                        restarts = restarts.saturating_add(1);
                        runtime
                            .mark_child_restart_pending(&child.name, restarts)
                            .await?;
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
            Err(err) => {
                runtime
                    .mark_child_stopped(
                        &child.name,
                        "spawn failed".to_owned(),
                        Some(err.to_string()),
                    )
                    .await?;
                if !child.restart {
                    return Ok(());
                }
                restarts = restarts.saturating_add(1);
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

async fn status_server(
    listener: TcpListener,
    runtime: SupervisorRuntime,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accepting status connection")?;
                let runtime = runtime.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_status_connection(stream, runtime).await {
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

async fn handle_status_connection(mut stream: TcpStream, runtime: SupervisorRuntime) -> Result<()> {
    let mut buffer = [0_u8; 2048];
    let read = stream.read(&mut buffer).await.context("reading request")?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    match path {
        "/health" => write_http_json(&mut stream, 200, &json!({ "ok": true })).await,
        "/status" | "/" => {
            let snapshot = runtime.snapshot().await;
            write_http_json(&mut stream, 200, &snapshot).await
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

async fn write_http_json<T: Serialize>(
    stream: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
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

impl SupervisorRuntime {
    async fn snapshot(&self) -> SupervisorState {
        self.state.lock().await.clone()
    }

    async fn persist_state(&self) -> Result<()> {
        let snapshot = self.snapshot().await;
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&self.state_file, serde_json::to_vec_pretty(&snapshot)?)
            .with_context(|| format!("writing {}", self.state_file.display()))?;
        Ok(())
    }

    async fn mark_child_started(
        &self,
        child: &ChildConfig,
        pid: Option<u32>,
        restarts: u64,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            let entry = state
                .children
                .entry(child.name.clone())
                .or_insert_with(|| child.initial_state());
            entry.pid = pid;
            entry.running = true;
            entry.restarts = restarts;
            entry.started_at_ms = Some(unix_epoch_millis()?);
            entry.stopped_at_ms = None;
            entry.last_exit = None;
            entry.last_error = None;
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
                entry.stopped_at_ms = Some(unix_epoch_millis()?);
                entry.last_exit = Some(exit);
                entry.last_error = error;
            }
        }
        self.persist_state().await
    }

    async fn mark_child_restart_pending(&self, name: &str, restarts: u64) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.children.get_mut(name) {
                entry.restarts = restarts;
            }
        }
        self.persist_state().await
    }
}

impl ChildConfig {
    fn initial_state(&self) -> ChildState {
        ChildState {
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            pid: None,
            running: false,
            restart: self.restart,
            restarts: 0,
            started_at_ms: None,
            stopped_at_ms: None,
            last_exit: None,
            last_error: None,
        }
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

fn example_config() -> &'static str {
    r#"[supervisor]
bind = "127.0.0.1:11437"

[[supervisor.children]]
name = "gateway"
command = "mayhem"
args = ["use", "--home", "/absolute/path/to/.mayhem"]
restart = true
restart_backoff_ms = 1000

[[supervisor.children]]
name = "paygate"
command = "mayhem-paygate"
args = ["--config", "/absolute/path/to/paygate.toml"]
restart = true
"#
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            ChildConfig {
                name: "gateway".to_owned(),
                command: "true".to_owned(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                restart: true,
                restart_backoff_ms: 1_000,
            },
        ];
        assert!(validate_children(&children).is_err());
    }

    #[test]
    fn rejects_invalid_bind_address() {
        assert!(parse_bind("not-a-socket").is_err());
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
        };
        let states = initial_child_states(&[child]);
        let state = states.get("peer").unwrap();
        assert!(!state.running);
        assert_eq!(state.command, "pear");
        assert_eq!(state.args, ["run", "intercom"]);
    }
}
