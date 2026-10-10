//! Actual NTFS fixtures. Cross-compiling these is not native enforcement proof.
use super::*;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::Security::{
    Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo, SE_FILE_OBJECT,
    },
    GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    SECURITY_ATTRIBUTES,
};

fn leaf(value: &str) -> LeafName {
    LeafName::new(value).unwrap()
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let name = nonce.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let path = std::env::temp_dir().join(format!("mayhem-ntfs-mutation-{name}"));
        let security = native::descriptor(&acl::current_user().unwrap()).unwrap();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: 0,
        };
        assert_ne!(
            unsafe { CreateDirectoryW(wide(path.to_str().unwrap()).as_ptr(), &attributes) },
            0
        );
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn commit(guard: &mut NtfsGuard<'_>, temporary: &str, name: &str, bytes: &[u8]) -> CommitIdentity {
    let mut p = guard.prepare(leaf(temporary), bytes, 4096).unwrap();
    p.publish(leaf(name), PublishMode::CreateNew).unwrap()
}

#[test]
fn ntfs_leaf_names_bounds_and_lock_name_are_not_mutable_paths() {
    for value in [
        "", ".", "..", "../x", "a/b", "a\\b", "x:y", "nul", "con.txt", "COM1", "Upper", "name ",
        "name.",
    ] {
        assert!(LeafName::new(value).is_err(), "{value}");
    }
    assert!(LeafName::new(&"x".repeat(256)).is_err());
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    assert!(matches!(
        guard.prepare(leaf(".mayhem-ntfs.lock"), b"x", 1),
        Err(MutationError::Invalid)
    ));
    assert!(matches!(
        guard.prepare(leaf("large"), b"xx", 1),
        Err(MutationError::Invalid)
    ));
    assert!(matches!(
        guard.prepare(leaf("large"), b"", MAX_BYTES + 1),
        Err(MutationError::Invalid)
    ));
    assert!(!f.0.join("large").exists());
}

#[test]
fn ntfs_commit_create_replace_and_reopen_preserve_whole_exact_records() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let first;
    let second;
    {
        let mut guard = dir.try_lock().unwrap();
        first = commit(&mut guard, "first.next", "draft.json", b"original");
        assert_eq!(
            &*guard.read(&leaf("draft.json"), 8).unwrap().unwrap(),
            b"original"
        );
        assert!(guard.read(&leaf("draft.json"), 7).is_err());
        let mut pending = guard
            .prepare(leaf("second.next"), b"new complete revision", 128)
            .unwrap();
        second = pending.identity();
        assert_eq!(
            pending.publish(leaf("draft.json"), PublishMode::CreateNew),
            Err(MutationError::Conflict)
        );
        assert!(pending.attempted_destination().is_none());
        assert_eq!(
            pending
                .publish(leaf("draft.json"), PublishMode::Replace)
                .unwrap(),
            second
        );
        assert!(pending.is_committed());
        assert_ne!(first.file_id, second.file_id);
        assert_eq!(pending.reconcile().unwrap(), second);
        assert_eq!(
            pending.publish(leaf("other.json"), PublishMode::Replace),
            Err(MutationError::CommitUnknown)
        );
    }
    drop(dir);
    let restored = NtfsDirectory::open_existing(&f.0).unwrap();
    let guard = restored.try_lock().unwrap();
    assert_eq!(
        &*guard.read(&leaf("draft.json"), 128).unwrap().unwrap(),
        b"new complete revision"
    );
    assert!(guard.read(&leaf("first.next"), 128).unwrap().is_none());
    assert!(guard.read(&leaf("second.next"), 128).unwrap().is_none());
    assert!(!f.0.join("other.json").exists());
}

#[test]
fn ntfs_fault_boundaries_keep_original_identity_and_do_not_retry_rename() {
    for failed in [Stage::BeforeRename, Stage::AfterRename, Stage::AfterFlush] {
        let f = Fixture::new();
        let dir = NtfsDirectory::open_existing(&f.0).unwrap();
        let mut guard = dir.try_lock().unwrap();
        commit(&mut guard, "original.next", "draft.json", b"old");
        let mut pending = guard
            .prepare(leaf("retained.next"), b"exact new", 64)
            .unwrap();
        let original = pending.identity();
        let result = pending.publish_inner(leaf("draft.json"), PublishMode::Replace, |stage| {
            if stage == failed {
                Err(MutationError::Storage)
            } else {
                Ok(())
            }
        });
        if failed == Stage::BeforeRename {
            assert_eq!(result, Err(MutationError::Storage));
            assert!(pending.attempted_destination().is_none());
            drop(pending);
            assert_eq!(
                &*guard.read(&leaf("draft.json"), 64).unwrap().unwrap(),
                b"old"
            );
            assert_eq!(
                &*guard.read(&leaf("retained.next"), 64).unwrap().unwrap(),
                b"exact new"
            );
        } else {
            assert_eq!(result, Err(MutationError::CommitUnknown));
            assert!(!pending.is_committed());
            assert_eq!(pending.identity(), original);
            assert_eq!(pending.attempted_destination(), Some(&leaf("draft.json")));
            assert_eq!(
                pending.publish(leaf("different.json"), PublishMode::Replace),
                Err(MutationError::CommitUnknown)
            );
            assert_eq!(pending.reconcile().unwrap(), original);
            drop(pending);
            assert_eq!(
                &*guard.read(&leaf("draft.json"), 64).unwrap().unwrap(),
                b"exact new"
            );
            assert!(!f.0.join("different.json").exists());
        }
    }
}

#[test]
fn ntfs_actual_sharing_failure_stays_uncertain_without_promoting_temporary() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    commit(&mut guard, "original.next", "draft.json", b"old");
    // Ordinary protected read denies deletion, producing a real rename failure.
    let held = Pinned::open(&f.0.join("draft.json"), false).unwrap();
    let mut pending = guard.prepare(leaf("retained.next"), b"new", 3).unwrap();
    assert_eq!(
        pending.publish(leaf("draft.json"), PublishMode::Replace),
        Err(MutationError::CommitUnknown)
    );
    drop(held);
    assert_eq!(pending.reconcile(), Err(MutationError::CommitUnknown));
    assert_eq!(
        pending.publish(leaf("draft.json"), PublishMode::Replace),
        Err(MutationError::CommitUnknown)
    );
    drop(pending);
    assert_eq!(
        &*guard.read(&leaf("draft.json"), 3).unwrap().unwrap(),
        b"old"
    );
    assert_eq!(
        &*guard.read(&leaf("retained.next"), 3).unwrap().unwrap(),
        b"new"
    );
}

#[test]
fn ntfs_failed_targets_do_not_authorize_replacement_or_unsafe_existing_temps() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let mut guard = dir.try_lock().unwrap();
    commit(&mut guard, "original.next", "draft.json", b"old");
    fs::hard_link(f.0.join("draft.json"), f.0.join("alias")).unwrap();
    let mut pending = guard.prepare(leaf("candidate.next"), b"new", 3).unwrap();
    assert_eq!(
        pending.publish(leaf("draft.json"), PublishMode::Replace),
        Err(MutationError::Protection)
    );
    assert!(pending.attempted_destination().is_none());
    drop(pending);
    assert!(matches!(
        guard.prepare(leaf("candidate.next"), b"overwrite", 32),
        Err(MutationError::Conflict)
    ));
    fs::remove_file(f.0.join("alias")).unwrap();

    // Give another principal effective read access through an explicit DACL.
    let target = fs::OpenOptions::new()
        .read(true)
        .open(f.0.join("draft.json"))
        .unwrap();
    let mut descriptor = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide("D:P(A;;FA;;;OW)(A;;FR;;;WD)").as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        },
        0
    );
    let mut dacl = null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) },
        0
    );
    // Reopen with explicit WRITE_DAC; owner may alter its own fixture ACL.
    drop(target);
    let handle = unsafe {
        CreateFileW(
            wide(f.0.join("draft.json").to_str().unwrap()).as_ptr(),
            WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    let target = unsafe { File::from_raw_handle(handle) };
    assert_eq!(
        unsafe {
            SetSecurityInfo(
                target.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            )
        },
        0
    );
    unsafe {
        LocalFree(descriptor);
    }
    drop(target);
    let mut pending = guard
        .prepare(leaf("public-candidate.next"), b"new", 3)
        .unwrap();
    assert_eq!(
        pending.publish(leaf("draft.json"), PublishMode::Replace),
        Err(MutationError::Protection)
    );
    assert!(pending.attempted_destination().is_none());
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
#[ignore = "invoked only by the bounded parent process fixture"]
fn ntfs_lock_child_fixture() {
    let Some(directory) = std::env::var_os("MAYHEM_NTFS_LOCK_FIXTURE") else {
        return;
    };
    let dir = NtfsDirectory::open_existing(Path::new(&directory)).unwrap();
    let _guard = dir.try_lock().unwrap();
    fs::write(
        Path::new(&directory).join("child-ready"),
        b"synthetic ready",
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(60));
    panic!("parent must terminate this bounded lock fixture");
}
#[test]
fn ntfs_lock_excludes_processes_survives_alias_and_releases_on_owner_exit() {
    let f = Fixture::new();
    let dir = NtfsDirectory::open_existing(&f.0).unwrap();
    let same = NtfsDirectory::open_existing(&f.0).unwrap();
    {
        let _guard = dir.try_lock().unwrap();
        assert!(matches!(same.try_lock(), Err(MutationError::Busy)));
        assert!(fs::remove_file(f.0.join(".mayhem-ntfs.lock")).is_err());
        assert!(fs::rename(&f.0, f.0.with_extension("moved")).is_err());
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "private_files::mutation::tests::ntfs_lock_child_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("MAYHEM_NTFS_LOCK_FIXTURE", &f.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.0.join("child-ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child failed before obtaining actual lock"
        );
        assert!(Instant::now() < deadline, "bounded lock fixture startup");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(dir.try_lock(), Err(MutationError::Busy)));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match dir.try_lock() {
            Ok(_) => break,
            Err(MutationError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => panic!("lock must release after the original process exits"),
        }
    }
    assert!(f.0.join(".mayhem-ntfs.lock").is_file());
}
