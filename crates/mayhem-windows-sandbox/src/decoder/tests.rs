//! Native Windows enforcement tests. Cross-compilation is not acceptance.
use super::*;
use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetHandleInformation;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

fn file_identity(handle: HANDLE) -> Option<(u32, u32, u32)> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    (unsafe { GetFileInformationByHandle(handle, &mut info) } != 0).then_some((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

pub(super) struct Fixture(pub(super) PathBuf);
impl Fixture {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mayhem-decoder-test-{}", nonce().unwrap()));
        Self::create_directory(&path);
        Self(path)
    }
    fn create_directory(path: &Path) {
        let user = current_user_sid().unwrap();
        let mut sid_text = null_mut();
        assert_ne!(
            unsafe {
                windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW(
                    user.as_ptr() as _,
                    &mut sid_text,
                )
            },
            0
        );
        let mut size = 0;
        while unsafe { *sid_text.add(size) } != 0 {
            size += 1;
        }
        let user =
            String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, size) }).unwrap();
        unsafe {
            LocalFree(sid_text as _);
        }
        let sddl = to_wide_null(format!("O:{user}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user})"));
        let mut sd = null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut sd,
                    null_mut(),
                )
            },
            0
        );
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let result = unsafe { CreateDirectoryW(to_wide_null(path.as_os_str()).as_ptr(), &sa) };
        unsafe {
            LocalFree(sd);
        }
        assert_ne!(result, 0);
    }
    fn launcher(&self) -> DecoderLauncher {
        let work = self.0.join("work");
        Self::create_directory(&work);
        DecoderLauncher::new(&std::env::current_exe().unwrap(), &work).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn probe(launcher: &DecoderLauncher, lines: &[String]) -> DecoderChild {
    probe_mode(launcher, DecoderMode::Decoder, lines)
}
fn probe_mode(launcher: &DecoderLauncher, mode: DecoderMode, lines: &[String]) -> DecoderChild {
    let args = [
        "--exact",
        "platform::decoder::tests::contained_probe",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(OsString::from);
    let mut child = launch(launcher.image.clone(), mode, &args).unwrap();
    let mut input = child.stdin.take().unwrap();
    for line in lines {
        writeln!(input, "{line}").unwrap();
    }
    drop(input);
    child
}
fn finish(mut child: DecoderChild) -> (u32, String) {
    let until = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(code) = child.try_wait().unwrap() {
            break code;
        }
        assert!(Instant::now() < until, "contained test exceeded deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut output = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(8192)
        .read_to_string(&mut output)
        .unwrap();
    (code, output)
}

#[test]
fn unrestricted_process_cannot_pass_decoder_verification() {
    assert!(verify_decoder_process().is_err());
    assert!(verify_tokenizer_process().is_err());
}

#[test]
fn effective_lpac_check_rejects_an_ordinary_appcontainer_token() {
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let name = to_wide_null(nonce().unwrap());
    let mut sid = null_mut();
    assert_eq!(
        unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) },
        0
    );
    let sid = SidGuard::new(sid);
    let mut parent = null_mut();
    assert_ne!(
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut parent,
            )
        },
        0
    );
    let parent = HandleGuard::new(parent);
    let module = unsafe { GetModuleHandleW(to_wide_null("kernelbase.dll").as_ptr()) };
    let function =
        unsafe { GetProcAddress(module, c"CreateAppContainerToken".as_ptr() as _) }.unwrap();
    let create: unsafe extern "system" fn(
        HANDLE,
        *const SECURITY_CAPABILITIES,
        *mut HANDLE,
    ) -> i32 = unsafe { std::mem::transmute(function) };
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.as_ptr(),
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let mut token = null_mut();
    assert_ne!(
        unsafe { create(parent.handle, &capabilities, &mut token) },
        0
    );
    let token = HandleGuard::new(token);
    let mut is_container = 0u32;
    let mut bytes = 0u32;
    assert_ne!(
        unsafe {
            GetTokenInformation(
                token.handle,
                TokenIsAppContainer,
                &mut is_container as *mut _ as _,
                size_of_val(&is_container) as u32,
                &mut bytes,
            )
        },
        0
    );
    assert_eq!(is_container, 1);
    assert!(!verify::is_lpac(token.handle).unwrap());
}

#[test]
fn tokenizer_verifier_requires_its_exact_job_and_preserves_decoder_verification() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    for (mode, command) in [
        (DecoderMode::Decoder, "decoder_not_tokenizer"),
        (DecoderMode::Tokenizer, "tokenizer_limits"),
    ] {
        let (code, output) = finish(probe_mode(&launcher, mode, &[command.into()]));
        assert_eq!(code, 0, "{output}");
        assert!(output.contains("EXACT_TOKENIZER_JOB_CHECKED"), "{output}");
    }
}

#[test]
fn ambiguous_paths_and_nonempty_workspaces_are_refused() {
    let fixture = Fixture::new();
    let image = std::env::current_exe().unwrap();
    assert!(DecoderLauncher::new(Path::new("relative.exe"), &fixture.0).is_err());
    assert!(DecoderLauncher::new(Path::new("\\\\server\\share\\worker.exe"), &fixture.0).is_err());
    fs::write(fixture.0.join("caller-data"), b"synthetic").unwrap();
    assert!(DecoderLauncher::new(&image, &fixture.0).is_err());
    assert_eq!(
        fs::read(fixture.0.join("caller-data")).unwrap(),
        b"synthetic"
    );
}

#[test]
fn lpac_denies_private_files_network_processes_and_unlisted_handles() {
    let fixture = Fixture::new();
    let private = fixture.0.join("private.txt");
    fs::write(&private, b"synthetic canary").unwrap();
    let canary = File::open(&private).unwrap();
    let (volume, index_high, index_low) = file_identity(canary.as_raw_handle()).unwrap();
    assert_ne!(
        unsafe {
            SetHandleInformation(
                canary.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        0
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let launcher = fixture.launcher();
    // Strict-handle mitigation may terminate the process for querying an
    // excluded handle. Isolate that check so the other authority checks run.
    let (code, output) = finish(probe(
        &launcher,
        &[
            "unlisted_handle".into(),
            (canary.as_raw_handle() as usize).to_string(),
            format!("{volume} {index_high} {index_low}"),
        ],
    ));
    assert!(output.contains("CHECK_UNLISTED_HANDLE"), "{output}");
    assert!(
        code == 0xc0000008 || (code == 0 && output.contains("UNLISTED_HANDLE_DENIED")),
        "unexpected unlisted-handle result {code:#x}: {output}"
    );
    let child = probe(
        &launcher,
        &[
            "authority".into(),
            private.to_str().unwrap().into(),
            listener.local_addr().unwrap().to_string(),
            std::process::id().to_string(),
        ],
    );
    let (code, output) = finish(child);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("LPAC_AUTHORITY_DENIED"), "{output}");
    assert_eq!(fs::read(&private).unwrap(), b"synthetic canary");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn job_bounds_committed_memory_and_drop_reaps_child() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    let (code, output) = finish(probe(&launcher, &["memory".into()]));
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("JOB_MEMORY_DENIED"));

    let child = probe(&launcher, &["idle".into()]);
    let mut retained = null_mut();
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                child.process.as_raw_handle(),
                GetCurrentProcess(),
                &mut retained,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        },
        0
    );
    let retained = HandleGuard::new(retained);
    drop(child);
    assert_eq!(unsafe { WaitForSingleObject(retained.handle, 2000) }, 0);
}

#[test]
fn image_is_immutable_shared_and_cleaned_without_workspace_grants() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    let image = launcher.image.program.clone();
    assert!(image.is_file());
    assert!(fs::OpenOptions::new().write(true).open(&image).is_err());
    assert!(fs::remove_file(&image).is_err());
    let second = launcher.clone();
    drop(launcher);
    assert!(image.is_file());
    drop(second);
    assert!(!image.exists());
}

#[test]
#[ignore = "only entered through the real restricted launcher"]
fn contained_probe() {
    // Production stderr is deliberately NUL. Only this synthetic test child
    // sends assertion diagnostics over its already captured stdout pipe.
    std::panic::set_hook(Box::new(|info| {
        println!("CONTAINED_TEST_FAILURE: {info}");
    }));
    verify_decoder_process().expect("LPAC/Job/stdio/mitigation verification");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let command = lines.next().unwrap().unwrap();
    match command.as_str() {
        "decoder_not_tokenizer" => {
            assert!(verify_tokenizer_process().is_err());
            verify_decoder_process().unwrap();
            println!("EXACT_TOKENIZER_JOB_CHECKED");
        }
        "tokenizer_limits" => {
            verify_tokenizer_process().unwrap();
            verify_decoder_process().unwrap();
            // The Job's fixed committed-memory ceiling is additional to the
            // actual tokenizer executable's separate counted Rust heap limit.
            let mut bytes = Vec::<u8>::new();
            assert!(bytes.try_reserve_exact(TOKENIZER_COMMIT_BYTES + 1).is_err());
            println!("EXACT_TOKENIZER_JOB_CHECKED");
        }
        "authority" => {
            let path = PathBuf::from(lines.next().unwrap().unwrap());
            let address = lines.next().unwrap().unwrap().parse().unwrap();
            let parent: u32 = lines.next().unwrap().unwrap().parse().unwrap();
            assert!(
                unsafe {
                    OpenProcess(
                        PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_CREATE_PROCESS,
                        0,
                        parent,
                    )
                }
                .is_null()
            );
            assert!(fs::read(&path).is_err());
            assert!(fs::write(&path, b"forbidden").is_err());
            assert!(fs::write(path.with_file_name("new.txt"), b"forbidden").is_err());
            assert!(fs::write(std::env::current_exe().unwrap(), b"forbidden").is_err());
            assert!(fs::write("mayhem-forbidden-relative-write", b"forbidden").is_err());
            let image = to_wide_null(std::env::current_exe().unwrap().as_os_str());
            // Read-only image access must not permit ACL/owner escalation even
            // though the launcher's normal user owns the staged file.
            assert_eq!(
                unsafe {
                    CreateFileW(
                        image.as_ptr(),
                        0x40000 | 0x80000,
                        windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ,
                        null(),
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL,
                        null_mut(),
                    )
                },
                INVALID_HANDLE_VALUE
            );
            // std::net panics when Windows refuses Winsock initialization.
            // Observe that refusal directly, instead of treating the Rust
            // initialization assertion as a sandbox/decoder failure. The
            // unrestricted parent already proved its loopback listener works.
            use windows_sys::Win32::Networking::WinSock::*;
            let mut wsa: WSADATA = unsafe { zeroed() };
            let initialized = unsafe { WSAStartup(0x0202, &mut wsa) };
            if initialized == 0 {
                assert!(TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_err());
                assert!(TcpListener::bind("127.0.0.1:0").is_err());
                assert!(UdpSocket::bind("127.0.0.1:0").is_err());
                unsafe {
                    WSACleanup();
                }
            } else {
                assert!(
                    matches!(initialized, WSASYSCALLFAILURE | WSAEACCES),
                    "unexpected Winsock initialization error: {initialized}"
                );
                println!("WINSOCK_INITIALIZATION_DENIED");
            }
            assert!(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .arg("--list")
                    .spawn()
                    .is_err()
            );
            let mut map = std::collections::HashMap::new();
            map.insert("clock", Instant::now());
            let mut bytes = vec![0u8; 1024 * 1024];
            getrandom::fill(&mut bytes[..32]).unwrap();
            assert!(map.contains_key("clock"));
            assert!(SystemTime::now().duration_since(UNIX_EPOCH).is_ok());
            println!("LPAC_AUTHORITY_DENIED");
        }
        "unlisted_handle" => {
            let handle: usize = lines.next().unwrap().unwrap().parse().unwrap();
            let identity = lines.next().unwrap().unwrap();
            let parts = identity
                .split_whitespace()
                .map(|part| part.parse::<u32>().unwrap())
                .collect::<Vec<_>>();
            let expected = (parts[0], parts[1], parts[2]);
            println!("CHECK_UNLISTED_HANDLE");
            std::io::stdout().flush().unwrap();
            let mut flags = 0;
            if unsafe { GetHandleInformation(handle as HANDLE, &mut flags) } != 0 {
                // Handle numbers can be reused by the child's own loader.
                // Reject the inherited object, not an unrelated equal number.
                assert_ne!(
                    file_identity(handle as HANDLE),
                    Some(expected),
                    "unlisted inheritable file reached child"
                );
            }
            println!("UNLISTED_HANDLE_DENIED");
        }
        "memory" => {
            let mut bytes = Vec::<u8>::new();
            assert!(bytes.try_reserve_exact(DECODER_COMMIT_BYTES * 2).is_err());
            println!("JOB_MEMORY_DENIED");
        }
        "idle" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        _ => panic!("unexpected fixture command"),
    }
}

pub(super) fn crash_after_creation(child: &DecoderChild, name: &[u16]) {
    if std::env::var_os("MAYHEM_DECODER_CRASH_AT_CREATE").is_none() {
        return;
    }
    println!(
        "\nSUSPENDED {} {}",
        unsafe { GetProcessId(child.process.as_raw_handle()) },
        String::from_utf16(&name[..64]).unwrap()
    );
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
#[ignore = "parent invokes and kills this bounded startup fixture"]
fn suspended_startup_parent() {
    let root = PathBuf::from(std::env::var_os("MAYHEM_DECODER_CRASH_ROOT").unwrap());
    let work = root.join("work");
    Fixture::create_directory(&work);
    let launcher = DecoderLauncher::new(&std::env::current_exe().unwrap(), &work).unwrap();
    let _child = launcher.spawn(DecoderMode::Decoder).unwrap();
    panic!("creation failpoint was not reached");
}

#[test]
fn parent_death_kills_worker_even_before_resume_or_registration_cleanup() {
    use std::process::{Command, Stdio};
    struct Parent(std::process::Child);
    impl Drop for Parent {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = Fixture::new();
    let mut parent = Parent(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::decoder::tests::suspended_startup_parent",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MAYHEM_DECODER_CRASH_AT_CREATE", "1")
            .env("MAYHEM_DECODER_CRASH_ROOT", &fixture.0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = parent.0.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if let Some(value) = line.strip_prefix("SUSPENDED ") {
                let _ = sender.send(value.to_owned());
            }
        }
    });
    let ready = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let pid = ready
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    assert!(!handle.is_null());
    let handle = HandleGuard::new(handle);
    assert_eq!(
        unsafe { WaitForSingleObject(handle.handle, 0) },
        WAIT_TIMEOUT
    );
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(handle.handle, 5000) },
        0,
        "suspended worker survived parent death"
    );
    // Restart in the exact same workdir, recovering both the registration and
    // the staged image. A different directory would hide an image leak.
    let next =
        DecoderLauncher::new(&std::env::current_exe().unwrap(), &fixture.0.join("work")).unwrap();
    let (code, output) = finish(probe(&next, &["memory".into()]));
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("JOB_MEMORY_DENIED"));
}

#[test]
fn image_recovery_is_bounded_and_rejects_unknown_files_and_hardlinks() {
    let fixture = Fixture::new();
    let work = fixture.0.join("work");
    Fixture::create_directory(&work);
    let recorded = "a".repeat(64);
    let record = work.join(".decoder-image-v1");
    {
        let file = crate::private_files::PrivateDatabaseFile::open(&record, false).unwrap();
        file.write(0, recorded.as_bytes()).unwrap();
        file.sync_data().unwrap();
    }
    let old = work.join(format!("decoder-{recorded}"));
    fs::create_dir(&old).unwrap();
    let image = old.join("mayhem-proxy-worker.exe");
    fs::write(&image, b"interrupted image copy").unwrap();
    let unrelated = old.join("must-remain");
    fs::write(&unrelated, b"caller data").unwrap();
    let program = std::env::current_exe().unwrap();
    assert!(DecoderLauncher::new(&program, &work).is_err());
    assert_eq!(fs::read(&unrelated).unwrap(), b"caller data");
    assert_eq!(fs::read(&record).unwrap(), recorded.as_bytes());
    fs::remove_file(&unrelated).unwrap();
    let alias = fixture.0.join("outside-link");
    fs::hard_link(&image, &alias).unwrap();
    assert!(DecoderLauncher::new(&program, &work).is_err());
    assert_eq!(fs::read(&alias).unwrap(), b"interrupted image copy");
    fs::remove_file(&alias).unwrap();
    let launcher = DecoderLauncher::new(&program, &work).unwrap();
    assert!(!old.exists());
    assert_eq!(fs::read_dir(&work).unwrap().count(), 2);
    let child = probe(&launcher, &["idle".into()]);
    drop(launcher);
    assert!(
        DecoderLauncher::new(&program, &work).is_err(),
        "live child retains image ownership"
    );
    drop(child);
    let next = DecoderLauncher::new(&program, &work).unwrap();
    let (code, output) = finish(probe(&next, &["memory".into()]));
    assert_eq!(code, 0, "{output}");
    drop(next);
    assert_eq!(fs::read_dir(&work).unwrap().count(), 1);
    fs::write(&record, [b'!'; 64]).unwrap();
    assert!(DecoderLauncher::new(&program, &work).is_err());
    assert_eq!(fs::read(&record).unwrap(), [b'!'; 64]);
}
