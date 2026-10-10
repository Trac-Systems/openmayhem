//! Native Windows enforcement tests. Cross-compilation is not acceptance.
use super::*;
use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetHandleInformation;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mayhem-decoder-test-{}", nonce().unwrap()));
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
        let sddl = to_wide_null(format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user})"));
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
        Self(path)
    }
    fn launcher(&self) -> DecoderLauncher {
        let work = self.0.join("work");
        fs::create_dir(&work).unwrap();
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
    let child = probe(
        &launcher,
        &[
            "authority".into(),
            private.to_str().unwrap().into(),
            listener.local_addr().unwrap().to_string(),
            (canary.as_raw_handle() as usize).to_string(),
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
            let handle: usize = lines.next().unwrap().unwrap().parse().unwrap();
            let parent: u32 = lines.next().unwrap().unwrap().parse().unwrap();
            let mut flags = 0;
            assert_eq!(
                unsafe { GetHandleInformation(handle as HANDLE, &mut flags) },
                0,
                "unlisted inheritable handle reached child"
            );
            assert!(unsafe {
                OpenProcess(
                    PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_CREATE_PROCESS,
                    0,
                    parent,
                )
            }
            .is_null());
            assert!(fs::read(&path).is_err());
            assert!(fs::write(&path, b"forbidden").is_err());
            assert!(fs::write(path.with_file_name("new.txt"), b"forbidden").is_err());
            assert!(fs::write(std::env::current_exe().unwrap(), b"forbidden").is_err());
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
            assert!(TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_err());
            assert!(TcpListener::bind("127.0.0.1:0").is_err());
            assert!(UdpSocket::bind("127.0.0.1:0").is_err());
            assert!(std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--list")
                .spawn()
                .is_err());
            let mut map = std::collections::HashMap::new();
            map.insert("clock", Instant::now());
            let mut bytes = vec![0u8; 1024 * 1024];
            getrandom::fill(&mut bytes[..32]).unwrap();
            assert!(map.contains_key("clock"));
            assert!(SystemTime::now().duration_since(UNIX_EPOCH).is_ok());
            println!("LPAC_AUTHORITY_DENIED");
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
