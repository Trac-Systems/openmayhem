//! Protected local traversal and reads. Read APIs never create, write or repair
//! permissions. The standalone mutation module requires a separate capability.
//! The current user, SYSTEM and OS Administrators are the trusted principals;
//! protection against them or a malicious kernel/filesystem is not claimed.
use crate::{Result, WindowsSandboxError};
use std::{
    fs::File,
    io::Read,
    mem::{size_of, zeroed},
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtOpenFile, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{Foundation::*, Storage::FileSystem::*, System::IO::IO_STATUS_BLOCK},
};
use zeroize::Zeroizing;
mod acl;
mod database;
pub use database::PrivateDatabaseFile;
pub(crate) mod mutation;
#[cfg(test)]
mod tests;
const READ_CONTROL: u32 = 0x20000;
const SYNCHRONIZE_ACCESS: u32 = 0x100000;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const OBJ_DONT_REPARSE: u32 = 0x1000;
#[cfg_attr(test, track_caller)]
fn invalid() -> WindowsSandboxError {
    #[cfg(test)]
    eprintln!(
        "protected local read refused at {}",
        std::panic::Location::caller()
    );
    WindowsSandboxError::InvalidConfig("protected local read rejected".into())
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

struct Pinned {
    handles: Vec<File>,
    owner: Vec<u8>,
}
impl Pinned {
    fn open(path: &Path, directory: bool) -> Result<Self> {
        Self::open_with_final_sharing(path, directory, FILE_SHARE_READ)
    }
    fn open_with_final_sharing(path: &Path, directory: bool, sharing: u32) -> Result<Self> {
        let text = path.to_str().ok_or_else(invalid)?;
        let (root, components) = components(text)?;
        let name = wide(&root);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL,
                FILE_SHARE_READ,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(invalid());
        }
        let root = unsafe { File::from_raw_handle(handle) };
        information(&root, true)?;
        // A volume GUID result is only available for a local volume. Do not
        // accept UNC/redirector/device paths, including a remapped drive alias.
        let mut volume = vec![0u16; 128];
        let size = unsafe {
            GetFinalPathNameByHandleW(
                root.as_raw_handle(),
                volume.as_mut_ptr(),
                volume.len() as u32,
                VOLUME_NAME_GUID,
            )
        } as usize;
        if size == 0 || size >= volume.len() {
            return Err(invalid());
        }
        let volume = String::from_utf16(&volume[..size]).map_err(|_| invalid())?;
        let guid = volume
            .strip_prefix(r"\\?\Volume{")
            .and_then(|s| s.strip_suffix("}\\"))
            .ok_or_else(invalid)?;
        if guid.len() != 36
            || !guid.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err(invalid());
        }
        let owner = acl::current_user()?;
        acl::validate(&root, &owner, false)?;
        let mut handles = vec![root];
        for (i, part) in components.iter().enumerate() {
            let last = i + 1 == components.len();
            let file = open_child(
                handles.last().ok_or_else(invalid)?,
                part,
                !last || directory,
                if last { sharing } else { FILE_SHARE_READ },
            )?;
            information(&file, !last || directory)?;
            acl::validate(&file, &owner, last)?;
            handles.push(file);
        }
        Ok(Self { handles, owner })
    }
    fn file(&self) -> &File {
        self.handles.last().expect("validated nonempty pinned path")
    }
}
fn components(text: &str) -> Result<(String, Vec<&str>)> {
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let bytes = text.as_bytes();
    if text.encode_utf16().count() > 30_000
        || bytes.len() < 4
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":\\"
        || text.contains('/')
    {
        return Err(invalid());
    }
    let parts = text[3..].split('\\').collect::<Vec<_>>();
    if parts.len() > 128 {
        return Err(invalid());
    }
    for part in &parts {
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.ends_with([' ', '.'])
            || part.encode_utf16().count() > 255
            || part
                .chars()
                .any(|c| c.is_control() || r#"<>:"|?*"#.contains(c))
        {
            return Err(invalid());
        }
        let base = part.split('.').next().ok_or_else(invalid)?.to_uppercase();
        if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str())
            || ["COM", "LPT"].iter().any(|p| {
                base.strip_prefix(p).is_some_and(|n| {
                    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&n)
                })
            })
        {
            return Err(invalid());
        }
    }
    Ok((text[..3].to_owned(), parts))
}
fn open_child(parent: &File, component: &str, directory: bool, sharing: u32) -> Result<File> {
    let mut name = component.encode_utf16().collect::<Vec<_>>();
    let bytes = u16::try_from(name.len() * 2).map_err(|_| invalid())?;
    let mut name = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: null_mut(),
        SecurityQualityOfService: null_mut(),
    };
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut handle = null_mut();
    let access = READ_CONTROL
        | SYNCHRONIZE_ACCESS
        | FILE_READ_ATTRIBUTES
        | if directory {
            FILE_TRAVERSE
        } else {
            FILE_READ_DATA
        };
    let options = FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT
        | if directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    let status = unsafe { NtOpenFile(&mut handle, access, &attributes, &mut io, sharing, options) };
    if status < 0 || handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(invalid());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}
fn information(file: &File, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
    {
        return Err(invalid());
    }
    let forbidden = FILE_ATTRIBUTE_REPARSE_POINT
        | FILE_ATTRIBUTE_DEVICE
        | FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
    if info.dwFileAttributes & forbidden != 0
        || ((info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory)
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(invalid());
    }
    Ok(info)
}
fn identity(info: &BY_HANDLE_FILE_INFORMATION) -> (u32, u32, u32, u32, u32, u32, u32, u32) {
    (
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
        info.nFileSizeHigh,
        info.nFileSizeLow,
        info.ftLastWriteTime.dwHighDateTime,
        info.ftLastWriteTime.dwLowDateTime,
        info.nNumberOfLinks,
    )
}
fn read(pinned: &Pinned, max_bytes: usize) -> Result<Zeroizing<Vec<u8>>> {
    read_file(pinned.file(), &pinned.owner, max_bytes)
}
fn read_file(file: &File, owner: &[u8], max_bytes: usize) -> Result<Zeroizing<Vec<u8>>> {
    let maximum = max_bytes
        .checked_add(1)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(invalid)?;
    let before = information(file, false)?;
    let length = (u64::from(before.nFileSizeHigh) << 32) | u64::from(before.nFileSizeLow);
    if length > max_bytes as u64 {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    bytes
        .try_reserve_exact(usize::try_from(length).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    file.take(maximum)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 != length
        || bytes.len() > max_bytes
        || identity(&before) != identity(&information(file, false)?)
    {
        return Err(invalid());
    }
    acl::validate(file, owner, true)?;
    Ok(bytes)
}
/// Read an existing private, regular, single-link local file using one stable
/// handle. Partial bytes are zeroized on every error; no path is reopened.
pub fn read_private_file(path: &Path, max_bytes: usize) -> Result<Zeroizing<Vec<u8>>> {
    read(&Pinned::open(path, false)?, max_bytes)
}
/// Validate an existing private directory, without creating it or granting
/// write/lock authority. This is a snapshot, not a persistence capability.
pub fn validate_private_directory(path: &Path) -> Result<()> {
    Pinned::open(path, true).map(|_| ())
}
