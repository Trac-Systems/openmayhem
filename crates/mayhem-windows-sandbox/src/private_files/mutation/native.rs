use super::*;
use std::mem::{align_of, offset_of};
use windows_sys::{
    Wdk::Storage::FileSystem as nt,
    Win32::{
        Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        System::{IO::OVERLAPPED, SystemServices::FILE_PERSISTENT_ACLS},
    },
};

pub(super) struct Descriptor(pub(super) *mut core::ffi::c_void);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
pub(super) fn descriptor(owner: &[u8]) -> Outcome<Descriptor> {
    let mut pointer = null_mut();
    if unsafe { ConvertSidToStringSidW(owner.as_ptr() as _, &mut pointer) } == 0 {
        return Err(MutationError::Protection);
    }
    let allocated = Descriptor(pointer as _);
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
        if length > 256 {
            return Err(MutationError::Protection);
        }
    }
    let user = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| MutationError::Protection)?;
    drop(allocated);
    // Explicit owner and protected DACL. No inheritance from a permissive parent.
    let text = wide(&format!(
        "O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)"
    ));
    let mut result = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut result,
            null_mut(),
        )
    } == 0
    {
        return Err(MutationError::Protection);
    }
    Ok(Descriptor(result))
}

pub(in crate::private_files) fn require_ntfs(directory: &File) -> Outcome<()> {
    let mut name = [0u16; 32];
    let mut flags = 0;
    if unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return Err(MutationError::Protection);
    }
    if name[..5] != [78, 84, 70, 83, 0] || flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(MutationError::UnsupportedFilesystem);
    }
    Ok(())
}
fn status_error(status: i32) -> MutationError {
    match status as u32 {
        0xc0000035 => MutationError::Conflict, // STATUS_OBJECT_NAME_COLLISION
        0xc0000043 | 0xc0000054 | 0xc0000055 => MutationError::Busy,
        _ => MutationError::Protection,
    }
}
pub(super) fn validate_file(file: &File, owner: &[u8]) -> Outcome<()> {
    information(file, false).map_err(|_| MutationError::Protection)?;
    acl::validate(file, owner, true).map_err(|_| MutationError::Protection)
}

fn open(
    pinned: &Pinned,
    name: &LeafName,
    disposition: u32,
    access: u32,
    sharing: u32,
    write_through: bool,
) -> Outcome<Option<File>> {
    open_at(
        pinned.file(),
        &pinned.owner,
        name,
        disposition,
        access,
        sharing,
        write_through,
        false,
        true,
    )
}
fn open_at(
    parent: &File,
    owner: &[u8],
    name: &LeafName,
    disposition: u32,
    access: u32,
    sharing: u32,
    write_through: bool,
    directory: bool,
    private_parent: bool,
) -> Outcome<Option<File>> {
    acl::validate(parent, owner, private_parent).map_err(|_| MutationError::Protection)?;
    let security = descriptor(owner)?;
    let mut wide_name = name.0.encode_utf16().collect::<Vec<_>>();
    let length = u16::try_from(wide_name.len() * 2).map_err(|_| MutationError::Invalid)?;
    let mut unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide_name.as_mut_ptr(),
    };
    let object = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut unicode,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.0.cast(),
        SecurityQualityOfService: null_mut(),
    };
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut handle = null_mut();
    let options = nt::FILE_SYNCHRONOUS_IO_NONALERT
        | if directory {
            nt::FILE_DIRECTORY_FILE
        } else {
            nt::FILE_NON_DIRECTORY_FILE
        }
        | nt::FILE_OPEN_REPARSE_POINT
        | if write_through {
            nt::FILE_WRITE_THROUGH
        } else {
            0
        };
    let status = unsafe {
        nt::NtCreateFile(
            &mut handle,
            access | READ_CONTROL | SYNCHRONIZE_ACCESS | FILE_READ_ATTRIBUTES,
            &object,
            &mut io,
            null(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            sharing,
            disposition,
            options,
            null(),
            0,
        )
    };
    if status != 0 {
        if disposition == nt::FILE_OPEN && status as u32 == 0xc0000034 {
            return Ok(None);
        }
        return Err(status_error(status));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(MutationError::Protection);
    }
    let file = unsafe { File::from_raw_handle(handle) };
    information(&file, directory).map_err(|_| MutationError::Protection)?;
    acl::validate(&file, owner, true).map_err(|_| MutationError::Protection)?;
    if write_through {
        let mut mode: nt::FILE_MODE_INFORMATION = unsafe { zeroed() };
        let status = unsafe {
            nt::NtQueryInformationFile(
                file.as_raw_handle(),
                &mut io,
                &mut mode as *mut _ as _,
                size_of::<nt::FILE_MODE_INFORMATION>() as u32,
                nt::FileModeInformation,
            )
        };
        if status != 0 || mode.Mode & nt::FILE_WRITE_THROUGH == 0 {
            return Err(MutationError::Protection);
        }
    }
    Ok(Some(file))
}
pub(super) fn create_directory(parent: &File, owner: &[u8], name: &LeafName) -> Outcome<File> {
    open_at(
        parent,
        owner,
        name,
        nt::FILE_CREATE,
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | DELETE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        true,
        true,
        true,
    )?
    .ok_or(MutationError::Protection)
}
pub(super) fn create_staged_file(parent: &File, owner: &[u8], name: &LeafName) -> Outcome<File> {
    open_at(
        parent,
        owner,
        name,
        nt::FILE_CREATE,
        GENERIC_READ | GENERIC_WRITE | DELETE,
        FILE_SHARE_READ,
        true,
        false,
        true,
    )?
    .ok_or(MutationError::Protection)
}
// An exclusive data handle, not a publishable temporary: no DELETE access and
// no sharing. The owner must retain the pinned ancestors until it is closed.
pub(in crate::private_files) fn database_file(
    pinned: &Pinned,
    name: &LeafName,
    existing: bool,
) -> Outcome<File> {
    open(
        pinned,
        name,
        if existing {
            nt::FILE_OPEN
        } else {
            nt::FILE_OPEN_IF
        },
        GENERIC_READ | GENERIC_WRITE,
        0,
        true,
    )?
    .ok_or(MutationError::Protection)
}
pub(super) fn inspect_directory(
    parent: &File,
    owner: &[u8],
    name: &LeafName,
) -> Outcome<Option<File>> {
    open_at(
        parent,
        owner,
        name,
        nt::FILE_OPEN,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        false,
        true,
        true,
    )
}
pub(super) fn lock_file(pinned: &Pinned, name: &LeafName) -> Outcome<File> {
    let file = open(
        pinned,
        name,
        nt::FILE_OPEN_IF,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        true,
    )?
    .ok_or(MutationError::Protection)?;
    if record_identity(&file)?.bytes != 0 {
        return Err(MutationError::Protection);
    }
    Ok(file)
}
// LOCALAPPDATA can be administrator-owned. It is a pinned, protected
// ancestor; only our newly created child must be current-user-owned/private.
pub(super) fn startup_directory(pinned: &Pinned, name: &LeafName) -> Outcome<File> {
    let directory = open_at(
        pinned.file(),
        &pinned.owner,
        name,
        nt::FILE_OPEN_IF,
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_ADD_FILE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        true,
        true,
        false,
    )?
    .ok_or(MutationError::Protection)?;
    flush(&directory)?;
    Ok(directory)
}
pub(super) fn startup_file(pinned: &Pinned, name: &LeafName) -> Outcome<File> {
    open(
        pinned,
        name,
        nt::FILE_OPEN_IF,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        true,
    )?
    .ok_or(MutationError::Protection)
}
pub(super) fn create_file(pinned: &Pinned, name: &LeafName) -> Outcome<File> {
    open(
        pinned,
        name,
        nt::FILE_CREATE,
        GENERIC_READ | GENERIC_WRITE | DELETE,
        FILE_SHARE_READ,
        true,
    )?
    .ok_or(MutationError::Protection)
}
pub(super) fn read_file(pinned: &Pinned, name: &LeafName) -> Outcome<Option<File>> {
    open(
        pinned,
        name,
        nt::FILE_OPEN,
        FILE_READ_DATA,
        FILE_SHARE_READ,
        false,
    )
}
pub(super) fn inspect_file(pinned: &Pinned, name: &LeafName) -> Outcome<Option<File>> {
    // Metadata-only inspection must share the original pending writer's write
    // and delete access. It confers no data-write/delete authority itself.
    open(
        pinned,
        name,
        nt::FILE_OPEN,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        false,
    )
}
pub(super) fn lock(file: &File) -> Outcome<()> {
    let mut overlap: OVERLAPPED = unsafe { zeroed() };
    if unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlap,
        )
    } == 0
    {
        return Err(if unsafe { GetLastError() } == ERROR_LOCK_VIOLATION {
            MutationError::Busy
        } else {
            MutationError::Protection
        });
    }
    Ok(())
}
pub(super) fn unlock(file: &File) {
    let mut overlap: OVERLAPPED = unsafe { zeroed() };
    unsafe {
        UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlap);
    }
    // Closing the owned handle is the final release even if explicit unlock fails.
}
pub(in crate::private_files) fn flush(file: &File) -> Outcome<()> {
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    // flags=0 includes metadata and the underlying device cache. Never weaken
    // this to DATA_ONLY/NO_SYNC or a successful no-op on unsupported filesystems.
    if unsafe { nt::NtFlushBuffersFileEx(file.as_raw_handle(), 0, null(), 0, &mut io) } != 0 {
        return Err(MutationError::Storage);
    }
    Ok(())
}
pub(super) fn rename(
    file: &File,
    directory: &File,
    name: &LeafName,
    mode: PublishMode,
) -> Outcome<()> {
    let name = name.0.encode_utf16().collect::<Vec<_>>();
    let offset = offset_of!(nt::FILE_RENAME_INFORMATION, FileName);
    let bytes = offset
        .checked_add(name.len() * 2)
        .ok_or(MutationError::Invalid)?;
    // Word allocation provides the structure's required alignment; no byte Vec
    // cast to an under-aligned FILE_RENAME_INFORMATION.
    const {
        assert!(align_of::<nt::FILE_RENAME_INFORMATION>() <= align_of::<usize>());
    }
    let mut storage = vec![
        0usize;
        bytes
            .max(size_of::<nt::FILE_RENAME_INFORMATION>())
            .div_ceil(size_of::<usize>())
    ];
    let data = storage.as_mut_ptr() as *mut nt::FILE_RENAME_INFORMATION;
    unsafe {
        (*data).Anonymous.ReplaceIfExists = mode == PublishMode::Replace;
        (*data).RootDirectory = directory.as_raw_handle();
        (*data).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*data).FileName).cast::<u16>(),
            name.len(),
        );
    }
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    if unsafe {
        nt::NtSetInformationFile(
            file.as_raw_handle(),
            &mut io,
            data as _,
            bytes as u32,
            nt::FileRenameInformation,
        )
    } != 0
    {
        return Err(MutationError::CommitUnknown);
    }
    Ok(())
}
