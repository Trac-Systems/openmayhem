//! A real executable must refuse before READY when its container/kernel prevents
//! installing the filter. No production test bypass or alternate worker path.
#![cfg(all(
    target_os = "linux",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
};

#[test]
fn denied_filter_installation_never_reads_ipc_or_emits_ready() {
    for mode in ["--stdio-v1", "--tokenizer-stdio-v1"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mayhem-proxy-worker"));
        command
            .arg(mode)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: fixed POD filter and async-signal-safe syscalls only between
        // fork and exec. This outer filter denies further seccomp installation;
        // exec and all ordinary initialization remain allowed.
        unsafe {
            command.pre_exec(|| {
                let filter = [
                    libc::sock_filter {
                        code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                        jt: 0,
                        jf: 0,
                        k: 0,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 1,
                        k: libc::SYS_seccomp as u32,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_RET | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 0,
                        k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_RET | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 0,
                        k: libc::SECCOMP_RET_ALLOW,
                    },
                ];
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_ptr().cast_mut(),
                };
                if libc::prctl(
                    libc::PR_SET_NO_NEW_PRIVS,
                    1 as libc::c_long,
                    0 as libc::c_long,
                    0 as libc::c_long,
                    0 as libc::c_long,
                ) != 0
                    || libc::syscall(
                        libc::SYS_seccomp,
                        libc::SECCOMP_SET_MODE_FILTER as libc::c_long,
                        0 as libc::c_long,
                        &program,
                    ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        // Retain an open, empty stdin. An unrestricted fallback would block on
        // IPC instead of exiting, and is killed after a bounded test deadline.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("worker continued after containment initialization failed: {mode}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(2), "{mode}");
        assert!(
            result.stdout.is_empty(),
            "no READY after failed containment"
        );
        assert!(result.stderr.is_empty(), "no untrusted startup diagnostics");
    }
}
