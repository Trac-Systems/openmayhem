#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::net::TcpListener;
use std::process::Command;

use mayhem_enclave::{
    build_sandbox_profile, SandboxConfig, SandboxPlatform, SandboxedCommand, SandboxedStderr,
};

fn sandbox_config(temp: &tempfile::TempDir) -> SandboxConfig {
    let model_root = temp.path().join("signed-model-root");
    let source_root = temp.path().join("embedded-source-root");
    let worker_cache = temp.path().join("worker-cache");
    for path in [&model_root, &source_root, &worker_cache] {
        std::fs::create_dir_all(path).expect("create sandbox directory");
    }
    SandboxConfig::new(vec![model_root, source_root], vec![worker_cache])
}

fn add_sandbox_cli_args(command: &mut Command, config: &SandboxConfig) {
    for path in &config.read_only_dirs {
        command.arg("--read-only-dir").arg(path);
    }
    for path in &config.writable_dirs {
        command.arg("--writable-dir").arg(path);
    }
}

fn sandboxed_binary(binary: &str, subcommand: &str) -> SandboxedCommand {
    let mut command = SandboxedCommand::new(binary);
    command.sandbox_helper(binary).arg(subcommand);
    command
}

fn run_write_probe(
    config: &SandboxConfig,
    target: &std::path::Path,
    expect_denied: bool,
) -> String {
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");
    let mut command = sandboxed_binary(binary, "sandbox-probe-store-write");
    command
        .arg("--sealed-store")
        .arg(target)
        .arg("--nested")
        .arg("--json")
        .stderr(SandboxedStderr::Piped);
    if expect_denied {
        command
            .arg("--expect-denied")
            .arg("--attempt-restore-permissions");
    }

    let mut child = command.spawn(config).expect("spawn sandboxed write probe");
    child.take_stdin();
    let mut output = String::new();
    child
        .take_stdout()
        .expect("sandboxed stdout")
        .read_to_string(&mut output)
        .expect("read write probe");
    let status = child.wait().expect("wait for write probe");
    assert!(status.success(), "status={status}\nstdout={output}");
    output
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandbox_run_blocks_outbound_tcp_probe() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let addr = listener.local_addr().expect("listener addr").to_string();
    #[cfg(target_os = "windows")]
    let addr = "1.1.1.1:80".to_owned();
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");

    let mut command = Command::new(binary);
    command.arg("sandbox-run");
    add_sandbox_cli_args(&mut command, &config);
    let output = command
        .args([
            "--",
            binary,
            "sandbox-probe-tcp",
            "--addr",
            &addr,
            "--expect-denied",
            "--json",
        ])
        .output()
        .expect("run sandboxed probe");

    assert!(
        output.status.success(),
        "sandboxed probe should report TCP denial\nstatus={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"denied\": true"), "{stdout}");
    assert!(stdout.contains("\"connected\": false"), "{stdout}");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandbox_run_blocks_writes_to_each_read_only_tree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");

    for read_only_dir in &config.read_only_dirs {
        let sealed_file = read_only_dir.join("sealed.bin");
        std::fs::write(&sealed_file, b"sealed").expect("write sealed file");
        let mut command = Command::new(binary);
        command.arg("sandbox-run");
        add_sandbox_cli_args(&mut command, &config);
        let output = command
            .arg("--")
            .arg(binary)
            .arg("sandbox-probe-store-write")
            .arg("--sealed-store")
            .arg(read_only_dir)
            .args(["--expect-denied", "--attempt-restore-permissions", "--json"])
            .output()
            .expect("run sandboxed write probe");

        assert!(
            output.status.success(),
            "sandboxed probe should report read-only write denial\nstatus={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"denied\": true"), "{stdout}");
        assert!(stdout.contains("\"wrote\": false"), "{stdout}");
        assert_eq!(
            std::fs::read(&sealed_file).expect("sealed file after sandbox"),
            b"sealed"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_round_trips_pipes_and_applies_process_options() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let work_dir = config.writable_dirs[0].join("worker-cwd");
    std::fs::create_dir_all(&work_dir).expect("create worker cwd");
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");
    let mut command = sandboxed_binary(binary, "sandbox-probe-stdio");
    command
        .args(["--label", "argument value"])
        .env("MAYHEM_SANDBOX_TEST_ENV", "environment value")
        .current_dir(&work_dir)
        .stderr(SandboxedStderr::Piped);

    let mut child = command.spawn(&config).expect("spawn sandboxed child");
    assert!(child.id() > 0);
    assert_eq!(child.try_wait().expect("try wait"), None);
    {
        let stdin = child.stdin().expect("sandboxed stdin");
        stdin
            .write_all(b"{\"message\":\"ping\"}\n")
            .expect("write worker request");
        stdin.flush().expect("flush worker request");
    }
    child.take_stdin();

    let mut stdout = BufReader::new(child.take_stdout().expect("sandboxed stdout"));
    let mut response = String::new();
    stdout
        .read_line(&mut response)
        .expect("read worker response");
    let response: serde_json::Value =
        serde_json::from_str(&response).expect("parse worker response");
    let mut stderr = String::new();
    child
        .take_stderr()
        .expect("sandboxed stderr")
        .read_to_string(&mut stderr)
        .expect("read worker stderr");
    let status = child.wait().expect("wait for sandboxed child");

    assert!(status.success(), "{status}");
    assert_eq!(response["line"], "{\"message\":\"ping\"}");
    assert_eq!(response["label"], "argument value");
    assert_eq!(response["env"], "environment value");
    assert_eq!(
        response["cwd"],
        std::fs::canonicalize(work_dir)
            .expect("canonical worker cwd")
            .to_string_lossy()
            .as_ref()
    );
    assert!(stderr.contains("mayhem-sandbox-test-stderr"), "{stderr}");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_reads_both_read_only_trees() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");

    for (index, read_only_dir) in config.read_only_dirs.iter().enumerate() {
        let expected = format!("immutable tree {index}");
        let read_path = read_only_dir.join("artifact.bin");
        std::fs::write(&read_path, expected.as_bytes()).expect("write read-only fixture");
        let mut command = sandboxed_binary(binary, "sandbox-probe-store-read");
        command.arg("--path").arg(&read_path);

        let mut child = command.spawn(&config).expect("spawn sandboxed read");
        child.take_stdin();
        let mut output = Vec::new();
        child
            .take_stdout()
            .expect("sandboxed stdout")
            .read_to_end(&mut output)
            .expect("read immutable bytes");
        let status = child.wait().expect("wait for sandboxed read");

        assert!(status.success(), "{status}");
        assert_eq!(output, expected.as_bytes());
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_denies_writes_to_both_read_only_trees() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);

    for read_only_dir in &config.read_only_dirs {
        let sealed_file = read_only_dir.join("sealed.bin");
        std::fs::write(&sealed_file, b"sealed").expect("write sealed file");
        let output = run_write_probe(&config, read_only_dir, true);

        assert!(output.contains("\"denied\": true"), "{output}");
        assert_eq!(
            std::fs::read(sealed_file).expect("sealed file after sandbox"),
            b"sealed"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_allows_nested_writes_in_worker_cache() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let output = run_write_probe(&config, &config.writable_dirs[0], false);

    assert!(output.contains("\"wrote\": true"), "{output}");
    assert!(output.contains("\"denied\": false"), "{output}");
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_denies_writes_outside_explicit_writable_trees() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("create outside directory");
    let output = run_write_probe(&config, &outside, true);

    assert!(output.contains("\"denied\": true"), "{output}");
    assert!(output.contains("\"wrote\": false"), "{output}");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_denies_outbound_network() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let addr = listener.local_addr().expect("listener addr").to_string();
    #[cfg(target_os = "windows")]
    let addr = "1.1.1.1:80".to_owned();
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");
    let mut command = sandboxed_binary(binary, "sandbox-probe-tcp");
    command
        .args(["--addr", &addr, "--expect-denied", "--json"])
        .stderr(SandboxedStderr::Piped);

    let mut child = command.spawn(&config).expect("spawn sandboxed TCP probe");
    child.take_stdin();
    let mut output = String::new();
    child
        .take_stdout()
        .expect("sandboxed stdout")
        .read_to_string(&mut output)
        .expect("read TCP probe");
    let status = child.wait().expect("wait for TCP probe");

    assert!(status.success(), "status={status}\nstdout={output}");
    assert!(output.contains("\"denied\": true"), "{output}");
    assert!(output.contains("\"connected\": false"), "{output}");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_can_be_killed_cleanly() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");
    let mut command = sandboxed_binary(binary, "sandbox-probe-stdio");
    command.args(["--label", "wait-for-input"]);

    let mut child = command.spawn(&config).expect("spawn waiting child");
    assert_eq!(child.try_wait().expect("try wait before kill"), None);
    child.kill().expect("kill sandboxed child");
    let status = child.wait().expect("wait after kill");

    assert!(!status.success(), "{status}");
    assert!(child.try_wait().expect("try wait after kill").is_some());
}

#[test]
fn sandbox_config_rejects_overlapping_trees() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model_root = temp.path().join("model-root");
    let source_root = temp.path().join("source-root");
    let nested_cache = model_root.join("worker-cache");
    for path in [&model_root, &source_root, &nested_cache] {
        std::fs::create_dir_all(path).expect("create sandbox directory");
    }
    let config = SandboxConfig::new(vec![model_root, source_root], vec![nested_cache.clone()]);

    let error = build_sandbox_profile(&config, SandboxPlatform::MacosSandboxExec)
        .expect_err("overlapping policy roots must fail");
    assert!(error.to_string().contains("overlaps"), "{error}");
}

#[cfg(unix)]
#[test]
fn sandbox_config_allows_symlinks_inside_policy_trees() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = sandbox_config(&temp);
    let target = config.read_only_dirs[0].join("runtime-target");
    std::fs::create_dir_all(&target).expect("create in-tree target");
    std::os::unix::fs::symlink(
        "runtime-target",
        config.read_only_dirs[0].join("runtime-link"),
    )
    .expect("create policy-tree symlink");

    build_sandbox_profile(&config, SandboxPlatform::LinuxSeccompBpf)
        .expect("ordinary in-tree runtime symlinks must be accepted");
}

#[cfg(unix)]
#[test]
fn sandbox_config_rejects_symlink_policy_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let real_root = temp.path().join("real-root");
    let linked_root = temp.path().join("linked-root");
    let writable_root = temp.path().join("worker-cache");
    std::fs::create_dir_all(&real_root).expect("create real root");
    std::fs::create_dir_all(&writable_root).expect("create writable root");
    std::os::unix::fs::symlink(&real_root, &linked_root).expect("create root symlink");
    let config = SandboxConfig::new(vec![linked_root], vec![writable_root]);

    let error = build_sandbox_profile(&config, SandboxPlatform::LinuxSeccompBpf)
        .expect_err("a policy root itself must never be a symlink");
    assert!(
        error.to_string().contains("must not be a symlink"),
        "{error}"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn sandboxed_child_never_falls_back_when_setup_is_invalid() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_model_root = temp.path().join("missing-model-root");
    let source_root = temp.path().join("source-root");
    let worker_cache = temp.path().join("worker-cache");
    let fallback_probe_dir = temp.path().join("fallback-probe");
    for path in [&source_root, &worker_cache, &fallback_probe_dir] {
        std::fs::create_dir_all(path).expect("create sandbox directory");
    }
    let config = SandboxConfig::new(vec![missing_model_root, source_root], vec![worker_cache]);
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");
    let mut command = sandboxed_binary(binary, "sandbox-probe-store-write");
    command
        .arg("--sealed-store")
        .arg(&fallback_probe_dir)
        .arg("--json");

    let error = command
        .spawn(&config)
        .err()
        .expect("invalid sandbox setup must fail");

    assert!(
        fallback_probe_dir
            .read_dir()
            .expect("read fallback probe dir")
            .next()
            .is_none(),
        "worker ran outside the rejected sandbox: {error}"
    );
}
