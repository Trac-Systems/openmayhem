//! Exercise failure of the actual executable's sandbox initialization. The
//! injected library only exists inside this test; production policy is unchanged.
#![cfg(target_os = "macos")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn forced_named_profile_failure_never_reads_ipc_or_emits_ready() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = temp.path().join("refuse.c");
    let library = temp.path().join("refuse.dylib");
    fs::write(&source, include_str!("support/macos_containment_failure.c")).unwrap();
    let build = Command::new("/usr/bin/xcrun")
        .args([
            "clang",
            "-dynamiclib",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-deprecated-declarations",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&library)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    for mode in ["--stdio-v1", "--tokenizer-stdio-v1", "--tokenizer-stdio-v2"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"))
            .arg(mode)
            .env_clear()
            .env("DYLD_INSERT_LIBRARIES", &library)
            .current_dir(temp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // An open but empty stdin distinguishes fail-before-IPC from a worker
        // that silently continued and blocked on its first untrusted packet.
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("worker continued after forced profile failure: {mode}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(2), "{mode}");
        assert!(result.stdout.is_empty(), "no READY after failure: {mode}");
        // Exact fixture-only markers prove both actual entry to sandbox_init
        // and its paired cleanup; an unrelated startup failure cannot pass.
        assert_eq!(
            result.stderr, b"forced sandbox refusal\nerror buffer released\n",
            "{mode}"
        );
    }
}
