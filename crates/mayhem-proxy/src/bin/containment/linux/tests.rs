use super::*;
use std::{fs, net::TcpListener, os::fd::AsRawFd, os::unix::net::UnixListener, process::Command};

// Small independent evaluator checks every emitted branch and scalar condition.
// The actual kernel installation/security test below remains authoritative.
fn decision(nr: c_long, arch: u32, args: [u64; 6]) -> u32 {
    let code = filter();
    let mut words = [0u32; 16];
    words[0] = nr as u32;
    words[1] = arch;
    for (i, value) in args.iter().enumerate() {
        words[4 + i * 2] = *value as u32;
        words[5 + i * 2] = (value >> 32) as u32;
    }
    let mut a = 0;
    let mut pc = 0;
    for _ in 0..4096 {
        let op = &code[pc];
        pc += 1;
        match op.code as u32 {
            x if x == libc::BPF_LD | libc::BPF_W | libc::BPF_ABS => a = words[op.k as usize / 4],
            x if x == libc::BPF_ALU | libc::BPF_AND | libc::BPF_K => a &= op.k,
            x if x == libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K => {
                pc += if a == op.k { op.jt } else { op.jf } as usize
            }
            x if x == libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K => {
                pc += if a & op.k != 0 { op.jt } else { op.jf } as usize
            }
            x if x == libc::BPF_RET | libc::BPF_K => return op.k,
            _ => panic!("unexpected filter instruction"),
        }
    }
    panic!("filter did not terminate");
}

#[test]
fn policy_denies_other_abis_descriptors_paths_processes_and_executable_memory() {
    assert!(filter().len() < 4096);
    for arch in [
        0x4000_0003,
        if ARCH == 0xc000_003e {
            0xc000_00b7
        } else {
            0xc000_003e
        },
    ] {
        assert_eq!(decision(libc::SYS_read, arch, [0; 6]), KILL);
    }
    #[cfg(target_arch = "x86_64")]
    for nr in [libc::SYS_read, libc::SYS_execve] {
        assert_eq!(decision(nr | 0x4000_0000, ARCH, [0; 6]), KILL);
    }
    for nr in [
        libc::SYS_openat,
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_clone,
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_ptrace,
        libc::SYS_kill,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_dup,
        libc::SYS_pipe2,
        libc::SYS_prctl,
        libc::SYS_seccomp,
        512,
        547,
        999,
    ] {
        assert_eq!(decision(nr, ARCH, [0; 6]), DENY, "syscall {nr}");
    }
    for fd in [0, 1, 2, 3, u32::MAX as u64, 1 << 32] {
        assert_eq!(
            decision(libc::SYS_read, ARCH, [fd, 0, 0, 0, 0, 0]),
            if fd as u32 == 0 { ALLOW } else { DENY }
        );
        assert_eq!(
            decision(libc::SYS_write, ARCH, [fd, 0, 0, 0, 0, 0]),
            if [1, 2].contains(&(fd as u32)) {
                ALLOW
            } else {
                DENY
            }
        );
    }
    let map = [
        0,
        8192,
        (libc::PROT_READ | libc::PROT_WRITE) as u64,
        (libc::MAP_PRIVATE | libc::MAP_ANONYMOUS) as u64,
        u64::MAX,
        0,
    ];
    assert_eq!(decision(libc::SYS_mmap, ARCH, map), ALLOW);
    for (i, value) in [
        (2, libc::PROT_EXEC as u64),
        (3, libc::MAP_PRIVATE as u64),
        (3, (libc::MAP_SHARED | libc::MAP_ANONYMOUS) as u64),
        (4, 0),
    ] {
        let mut bad = map;
        bad[i] = value;
        assert_eq!(decision(libc::SYS_mmap, ARCH, bad), DENY);
    }
    assert_eq!(
        decision(
            libc::SYS_mprotect,
            ARCH,
            [0, 4096, libc::PROT_EXEC as u64, 0, 0, 0]
        ),
        DENY
    );
    assert_eq!(
        decision(
            libc::SYS_futex,
            ARCH,
            [0, libc::FUTEX_WAIT as u64, 0, 0, 0, 0]
        ),
        DENY
    );
    assert_eq!(
        decision(
            libc::SYS_futex,
            ARCH,
            [
                0,
                (libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG) as u64,
                0,
                0,
                0,
                0
            ]
        ),
        ALLOW
    );
    assert_eq!(
        decision(libc::SYS_clock_gettime, ARCH, [u64::MAX, 0, 0, 0, 0, 0]),
        DENY
    );
    for flags in [0, libc::GRND_NONBLOCK, libc::GRND_INSECURE] {
        assert_eq!(
            decision(libc::SYS_getrandom, ARCH, [0, 32, flags as u64, 0, 0, 0]),
            ALLOW
        );
    }
    assert_eq!(
        decision(libc::SYS_getrandom, ARCH, [0, 32, u32::MAX as u64, 0, 0, 0]),
        DENY
    );
}

#[test]
fn actual_linux_filter_closes_inherited_handles_and_denies_external_access() {
    let root = std::env::temp_dir().join(format!("proxy-containment-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("synthetic-private");
    let output = root.join("forbidden-output");
    fs::write(&input, "synthetic private content").unwrap();
    let inherited = fs::File::open(&input).unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let unix = root.join("listener.sock");
    let _listener = UnixListener::bind(&unix).unwrap();
    // The enclosing test environment permits these actions before entry.
    assert_eq!(
        fs::read_to_string(&input).unwrap(),
        "synthetic private content"
    );
    drop(std::net::TcpStream::connect(tcp.local_addr().unwrap()).unwrap());
    drop(std::os::unix::net::UnixStream::connect(&unix).unwrap());
    assert!(Command::new("/usr/bin/true").status().unwrap().success());
    // These test-only handles deliberately survive exec. Neither is a real key,
    // upstream connection, financial store or host service.
    for fd in [inherited.as_raw_fd(), tcp.as_raw_fd()] {
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "containment::linux::tests::probe_child",
            "--ignored",
            "--nocapture",
        ])
        .env("PROXY_TEST_INPUT", &input)
        .env("PROXY_TEST_OUTPUT", &output)
        .env("PROXY_TEST_TCP", tcp.local_addr().unwrap().to_string())
        .env("PROXY_TEST_UNIX", &unix)
        .env(
            "PROXY_TEST_FDS",
            format!("{},{}", inherited.as_raw_fd(), tcp.as_raw_fd()),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            fs::remove_dir_all(&root).unwrap();
            panic!("contained probe did not exit");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(
        result.status.success(),
        "child failed: {:?}; {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Linux containment verified"));
    assert!(!output.exists());
}

#[test]
#[ignore = "isolated child only; installs irreversible process restrictions"]
fn probe_child() {
    let input = std::env::var_os("PROXY_TEST_INPUT").unwrap();
    let output = std::env::var_os("PROXY_TEST_OUTPUT").unwrap();
    let tcp = std::env::var("PROXY_TEST_TCP").unwrap();
    let unix = std::env::var_os("PROXY_TEST_UNIX").unwrap();
    let fds: Vec<i32> = std::env::var("PROXY_TEST_FDS")
        .unwrap()
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    for fd in &fds {
        assert!(unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0);
    }
    close_inherited().unwrap();
    for fd in fds {
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EBADF)
        );
    }
    // An invalid filter must be rejected; there is no unrestricted success path.
    assert!(install(&[]).is_err());
    install(&filter()).expect("kernel must install the exact decoder filter");
    assert_eq!(
        fs::read(input).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        fs::write(output, "forbidden").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        std::net::TcpStream::connect(tcp).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        std::os::unix::net::UnixStream::connect(unix)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(Command::new("/usr/bin/true").status().is_err());
    for nr in [
        libc::SYS_clone,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_kill,
    ] {
        assert_eq!(
            unsafe {
                libc::syscall(
                    nr,
                    0 as c_long,
                    0 as c_long,
                    0 as c_long,
                    0 as c_long,
                    0 as c_long,
                    0 as c_long,
                )
            },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
    }
    let mut random = [0u8; 32];
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_getrandom,
                random.as_mut_ptr(),
                random.len(),
                0 as c_long,
            )
        },
        32
    );
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_clock_gettime,
                libc::CLOCK_MONOTONIC as c_long,
                &mut time,
            )
        },
        0
    );
    let large = vec![23u8; 2 * 1024 * 1024];
    assert_eq!(
        large.iter().map(|v| *v as u64).sum::<u64>(),
        23 * 2 * 1024 * 1024
    );
    let map = std::collections::HashMap::from([("UTF-8 🦉", 37)]);
    assert_eq!(map.get("UTF-8 🦉"), Some(&37));
    let validator = jsonschema::validator_for(&serde_json::json!({
        "type": "object", "required": ["code"], "additionalProperties": false,
        "properties": {"code": {"type": "string", "pattern": "^café-[0-9]{2}$"}}
    }))
    .unwrap();
    assert!(validator.is_valid(&serde_json::json!({"code": "café-42"})));
    assert!(!validator.is_valid(&serde_json::json!({"code": "café-XX"})));
    assert!(!validator.is_valid(&serde_json::json!({"code": 42})));
    println!("Linux containment verified");
    std::process::exit(0);
}
