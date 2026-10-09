use std::ffi::{c_char, c_int};

#[link(name = "System")]
extern "C" {
    static kSBXProfilePureComputation: c_char;
    fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(error: *mut c_char);
    fn close(fd: c_int) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}

pub(super) fn enter() -> Result<(), ()> {
    // Already-open descriptors retain access despite the named profile. Remove
    // every inherited descriptor other than the broker's stdio pipes. This runs
    // only at dedicated child startup, before any worker thread or untrusted IPC.
    let inherited = std::fs::read_dir("/dev/fd")
        .map_err(|_| ())?
        .map(|entry| {
            entry
                .map_err(|_| ())?
                .file_name()
                .to_str()
                .ok_or(())?
                .parse::<c_int>()
                .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, ()>>()?;
    for fd in inherited.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: no Rust object owns these inherited handles. The directory
        // iterator was dropped above, so its former fd is already invalid.
        // Darwin's F_GETFD=1 and EBADF=9 verify closure, including close errors.
        unsafe {
            close(fd);
        }
        let result = unsafe { fcntl(fd, 1) };
        if result != -1 || std::io::Error::last_os_error().raw_os_error() != Some(9) {
            return Err(());
        }
    }
    let mut error = std::ptr::null_mut();
    // SAFETY: Apple's named profile is a static NUL-terminated C string. The
    // out-pointer is valid for this call, and only an error allocated by this
    // API is passed back to its paired deallocator. No untrusted bytes enter
    // the profile and no diagnostic (which could contain paths) is emitted.
    let status = unsafe {
        let status = sandbox_init(
            std::ptr::addr_of!(kSBXProfilePureComputation),
            1,
            &mut error,
        );
        if !error.is_null() {
            sandbox_free_error(error);
        }
        status
    };
    if status == 0 {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs, net::TcpListener, os::fd::AsRawFd, os::unix::net::UnixListener, process::Command,
    };

    #[test]
    fn decoder_profile_denies_files_network_and_child_execution() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("synthetic-private.txt");
        let output = dir.path().join("forbidden-output.txt");
        fs::write(&input, "synthetic private data").unwrap();
        let inherited = fs::File::open(&input).unwrap();
        // Deliberately leave one synthetic private file descriptor inheritable.
        // F_SETFD=2, flags=0 clears close-on-exec on this owned test handle only.
        assert_eq!(unsafe { super::fcntl(inherited.as_raw_fd(), 2, 0) }, 0);
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let unix_path = dir.path().join("test.sock");
        let _unix = UnixListener::bind(&unix_path).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "containment::macos::tests::probe_child",
                "--ignored",
                "--nocapture",
            ])
            .env("PROXY_TEST_PRIVATE", &input)
            .env("PROXY_TEST_OUTPUT", &output)
            .env("PROXY_TEST_TCP", tcp.local_addr().unwrap().to_string())
            .env("PROXY_TEST_UNIX", &unix_path)
            .env("PROXY_TEST_FD", inherited.as_raw_fd().to_string())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("containment verified"));
        assert!(!output.exists());
        assert_eq!(fs::read_to_string(input).unwrap(), "synthetic private data");
    }

    #[test]
    #[ignore = "runs only as an isolated child of the containment test"]
    fn probe_child() {
        let input = std::env::var_os("PROXY_TEST_PRIVATE").unwrap();
        let output = std::env::var_os("PROXY_TEST_OUTPUT").unwrap();
        let tcp = std::env::var("PROXY_TEST_TCP").unwrap();
        let unix = std::env::var_os("PROXY_TEST_UNIX").unwrap();
        let inherited: i32 = std::env::var("PROXY_TEST_FD").unwrap().parse().unwrap();
        assert!(
            unsafe { super::fcntl(inherited, 1) } >= 0,
            "fixture must really inherit an open descriptor"
        );
        super::enter().expect("OS containment must install successfully");
        assert_eq!(unsafe { super::fcntl(inherited, 1) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(9));
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
        println!("containment verified");
        // No test harness cleanup or unrelated tests run in this sandbox.
        std::process::exit(0);
    }
}
