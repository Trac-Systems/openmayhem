#![forbid(unsafe_code)]

use std::net::TcpListener;
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn sandbox_run_blocks_outbound_tcp_probe() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sealed_store = temp.path().join("sealed-store");
    let ipc_socket = temp.path().join("mayhem.sock");
    std::fs::create_dir_all(&sealed_store).expect("create sealed store");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr").to_string();
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");

    let output = Command::new(binary)
        .args([
            "sandbox-run",
            "--sealed-store",
            sealed_store.to_str().expect("sealed store utf-8"),
            "--ipc-socket",
            ipc_socket.to_str().expect("ipc socket utf-8"),
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

#[cfg(target_os = "linux")]
fn running_as_root_on_linux() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(2))
                .and_then(|uid| uid.parse::<u32>().ok())
        })
        == Some(0)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn sandbox_run_blocks_sealed_store_write_probe() {
    #[cfg(target_os = "linux")]
    if running_as_root_on_linux() {
        eprintln!(
            "skipping Linux sealed-store write probe as root; sandbox-run fails closed there"
        );
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let sealed_store = temp.path().join("sealed-store");
    let ipc_socket = temp.path().join("mayhem.sock");
    std::fs::create_dir_all(&sealed_store).expect("create sealed store");
    std::fs::write(sealed_store.join("sealed.bin"), b"sealed").expect("write sealed file");
    let binary = env!("CARGO_BIN_EXE_mayhem-enclave");

    let output = Command::new(binary)
        .args([
            "sandbox-run",
            "--sealed-store",
            sealed_store.to_str().expect("sealed store utf-8"),
            "--ipc-socket",
            ipc_socket.to_str().expect("ipc socket utf-8"),
            "--",
            binary,
            "sandbox-probe-store-write",
            "--sealed-store",
            sealed_store.to_str().expect("sealed store utf-8"),
            "--expect-denied",
            "--json",
        ])
        .output()
        .expect("run sandboxed write probe");

    assert!(
        output.status.success(),
        "sandboxed probe should report sealed-store write denial\nstatus={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"denied\": true"), "{stdout}");
    assert!(stdout.contains("\"wrote\": false"), "{stdout}");
    assert_eq!(
        std::fs::read(sealed_store.join("sealed.bin")).expect("sealed file after sandbox"),
        b"sealed"
    );
}
