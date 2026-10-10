//! Native Windows fixtures. Cross-checking is not filesystem enforcement proof.
use super::*;
use std::{fs, io::Write, path::PathBuf};
use windows_sys::Win32::{
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        SECURITY_ATTRIBUTES,
    },
    System::{
        Ioctl::FSCTL_SET_REPARSE_POINT, SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
        IO::DeviceIoControl,
    },
};
struct Descriptor(*mut core::ffi::c_void);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn descriptor(public: bool) -> Descriptor {
    descriptor_with_grants(if public { "(A;OICI;FR;;;WD)" } else { "" })
}
fn descriptor_with_grants(grants: &str) -> Descriptor {
    let owner = acl::current_user().unwrap();
    let mut pointer = null_mut();
    assert_ne!(
        unsafe { ConvertSidToStringSidW(owner.as_ptr() as _, &mut pointer) },
        0
    );
    let mut count = 0;
    while unsafe { *pointer.add(count) } != 0 {
        assert!(count < 256);
        count += 1;
    }
    let user = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, count) }).unwrap();
    unsafe {
        LocalFree(pointer as _);
    }
    let mut result = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(&format!(
                    "O:{user}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user}){grants}"
                ))
                .as_ptr(),
                1,
                &mut result,
                null_mut(),
            )
        },
        0
    );
    Descriptor(result)
}
fn directory(path: &Path, public: bool) {
    let descriptor = descriptor(public);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    assert_ne!(
        unsafe { CreateDirectoryW(wide(path.to_str().unwrap()).as_ptr(), &attributes) },
        0
    );
}
fn file(path: &Path, bytes: &[u8], public: bool) {
    let descriptor = descriptor(public);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            wide(path.to_str().unwrap()).as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    let mut file = unsafe { File::from_raw_handle(handle) };
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let name = nonce.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let path = std::env::temp_dir().join(format!("mayhem-private-read-{name}"));
        directory(&path, false);
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ambiguous_nonlocal_and_device_paths_are_rejected_before_open() {
    for value in [
        "relative.txt",
        r"C:relative.txt",
        r"\\server\share\private",
        r"\\?\UNC\server\share\private",
        r"\\.\PhysicalDrive0",
        r"C:\folder\..\secret",
        r"C:\folder\.\secret",
        r"C:\folder\\secret",
        r"C:\folder\secret:stream",
        r"C:\folder\secret.",
        r"C:\folder\secret ",
        r"C:\folder\NUL.txt",
        r"C:\folder\COM¹",
        r"C:/folder/private",
        "C:\\folder\\nul\0x",
    ] {
        assert!(components(value).is_err(), "{value}");
        assert!(read_private_file(Path::new(value), 64).is_err(), "{value}");
    }
    assert!(components(&format!("C:\\{}", ["part"; 129].join("\\"))).is_err());
    assert!(components(&format!("C:\\{}", "x".repeat(256))).is_err());
    assert!(components(r"C:\private\data.json").is_ok());
    assert!(components(r"\\?\C:\private\data.json").is_ok());
}

#[test]
fn private_file_reads_are_bounded_regular_and_single_link() {
    let f = Fixture::new();
    let path = f.path("original");
    file(&path, b"synthetic private", false);
    assert_eq!(
        &*read_private_file(&path, 17).unwrap(),
        b"synthetic private"
    );
    assert!(read_private_file(&path, 16).is_err());
    assert!(read_private_file(&path, usize::MAX).is_err());
    assert!(validate_private_directory(&f.0).is_ok());
    assert!(validate_private_directory(&path).is_err());
    assert!(read_private_file(&f.0, 64).is_err());
    let empty = f.path("empty");
    file(&empty, b"", false);
    assert!(read_private_file(&empty, 0).unwrap().is_empty());
    let linked = f.path("hardlink");
    fs::hard_link(&path, &linked).unwrap();
    assert!(read_private_file(&path, 64).is_err());
    assert!(read_private_file(&linked, 64).is_err());
    fs::remove_file(linked).unwrap();
    assert!(read_private_file(&path, 64).is_ok());
}

#[test]
fn public_acl_and_mutable_open_handles_are_refused() {
    let f = Fixture::new();
    let public = f.path("public");
    file(&public, b"synthetic only", true);
    assert!(read_private_file(&public, 64).is_err());
    let public_dir = f.path("public-directory");
    directory(&public_dir, true);
    assert!(validate_private_directory(&public_dir).is_err());
    let private = f.path("mutable");
    file(&private, b"original", false);
    let writer = fs::OpenOptions::new().write(true).open(&private).unwrap();
    assert!(read_private_file(&private, 64).is_err());
    drop(writer);
    assert!(read_private_file(&private, 64).is_ok());
}

#[test]
fn ancestor_read_access_does_not_authorize_mutation_or_private_directory_use() {
    let f = Fixture::new();
    let readable = f.path("readable-parent");
    directory(&readable, true);
    file(&readable.join("private"), b"synthetic", false);
    assert!(read_private_file(&readable.join("private"), 64).is_ok());
    assert!(validate_private_directory(&readable).is_err());

    let mutable = f.path("mutable-parent");
    let descriptor = descriptor_with_grants("(A;OICI;GA;;;WD)");
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    assert_ne!(
        unsafe { CreateDirectoryW(wide(mutable.to_str().unwrap()).as_ptr(), &attributes) },
        0
    );
    file(&mutable.join("private"), b"synthetic", false);
    assert!(read_private_file(&mutable.join("private"), 64).is_err());
}

#[test]
fn trusted_installer_ancestor_does_not_trust_other_services_or_expose_private_leaves() {
    let f = Fixture::new();
    for (name, sid, allowed) in [
        (
            "installer",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
            true,
        ),
        (
            "other-service",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478465",
            false,
        ),
        ("all-services", "S-1-5-80-0", false),
    ] {
        let parent = f.path(name);
        let descriptor = descriptor_with_grants(&format!("(A;;FA;;;{sid})"));
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        assert_ne!(
            unsafe { CreateDirectoryW(wide(parent.to_str().unwrap()).as_ptr(), &attributes) },
            0
        );
        file(&parent.join("private"), b"synthetic", false);
        assert_eq!(
            read_private_file(&parent.join("private"), 64).is_ok(),
            allowed,
            "{name}"
        );
        assert!(validate_private_directory(&parent).is_err(), "{name}");
    }
}

#[test]
fn pinned_read_blocks_path_replacement_then_next_read_revalidates_new_authority() {
    let f = Fixture::new();
    let parent = f.path("parent");
    directory(&parent, false);
    let path = parent.join("original");
    file(&path, b"original", false);
    let replacement = f.path("replacement");
    file(&replacement, b"untrusted public", true);
    let pinned = Pinned::open(&path, false).unwrap();
    assert!(fs::rename(&path, parent.join("moved")).is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(&replacement, &path).is_err());
    assert!(fs::rename(&parent, f.path("moved-parent")).is_err());
    assert_eq!(&*read(&pinned, 64).unwrap(), b"original");
    drop(pinned);
    fs::remove_file(&path).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(read_private_file(&path, 64).is_err());
}

#[test]
fn junctions_are_rejected_as_final_objects_and_as_ancestors() {
    let f = Fixture::new();
    let target = f.path("target");
    directory(&target, false);
    file(&target.join("private"), b"never follow", false);
    let junction = f.path("junction");
    directory(&junction, false);
    let target_text = target.to_str().unwrap();
    let substitute = format!(r"\??\{}", target_text);
    let sub = substitute.encode_utf16().collect::<Vec<_>>();
    let print = target_text.encode_utf16().collect::<Vec<_>>();
    let mut bytes = Vec::new();
    bytes.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    bytes.extend(
        u16::try_from(8 + (sub.len() + print.len() + 2) * 2)
            .unwrap()
            .to_le_bytes(),
    );
    bytes.extend(0u16.to_le_bytes());
    bytes.extend(0u16.to_le_bytes());
    bytes.extend(u16::try_from(sub.len() * 2).unwrap().to_le_bytes());
    bytes.extend(u16::try_from((sub.len() + 1) * 2).unwrap().to_le_bytes());
    bytes.extend(u16::try_from(print.len() * 2).unwrap().to_le_bytes());
    for character in sub.into_iter().chain(Some(0)).chain(print).chain(Some(0)) {
        bytes.extend(character.to_le_bytes());
    }
    let handle = unsafe {
        CreateFileW(
            wide(junction.to_str().unwrap()).as_ptr(),
            GENERIC_WRITE,
            0,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    let file = unsafe { File::from_raw_handle(handle) };
    let mut returned = 0;
    assert_ne!(
        unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                bytes.as_ptr() as _,
                bytes.len() as u32,
                null_mut(),
                0,
                &mut returned,
                null_mut(),
            )
        },
        0,
        "native fixture must create a real junction; no skipped enforcement claim"
    );
    drop(file);
    assert!(validate_private_directory(&junction).is_err());
    assert!(read_private_file(&junction.join("private"), 64).is_err());
    fs::remove_dir(junction).unwrap();
    assert_eq!(fs::read(target.join("private")).unwrap(), b"never follow");
}
