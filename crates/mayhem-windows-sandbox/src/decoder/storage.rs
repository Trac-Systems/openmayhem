use super::*;
use std::io::{Read, Write};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    ConvertStringSidToSidW,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, IsValidSid, ACCESS_ALLOWED_ACE, ACE_HEADER, OWNER_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};

pub(super) struct Image {
    pub program: PathBuf,
    pub directory: PathBuf,
    pub capability: Vec<u8>,
    image_lock: Option<File>,
    _parents: Vec<File>,
}
impl Image {
    pub fn new(program: &Path, workdir: &Path) -> Result<Self> {
        let mut parents = pin_path(program)?;
        parents.extend(pin_path(workdir)?);
        let owner = current_user_sid()?;
        let source = open(
            program,
            windows_sys::Win32::Foundation::GENERIC_READ | 0x20000,
            FILE_SHARE_READ,
        )?;
        let directory = open(
            workdir,
            FILE_READ_ATTRIBUTES | 0x20000,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        validate_acl(&source, &owner, false)?;
        validate_acl(&directory, &owner, true)?;
        if !directory.metadata()?.is_dir() || fs::read_dir(workdir)?.next().is_some() {
            return Err(invalid());
        }
        let length = source.metadata()?.len();
        if length == 0 || length > 256 * 1024 * 1024 || !source.metadata()?.is_file() {
            return Err(invalid());
        }
        let capability = derive_capability_sid(&format!("mayhemProxyDecoderImage{}", nonce()?))?;
        let security = descriptor(&owner, &capability)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: 0,
        };
        let path = workdir.join(format!("decoder-{}", nonce()?));
        let wide = to_wide_null(path.as_os_str());
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
            return Err(last_error("decoder image directory"));
        }
        let executable = path.join("mayhem-proxy-worker.exe");
        let mut image = Self {
            program: executable,
            directory: path,
            capability,
            image_lock: None,
            _parents: parents,
        };
        let wide = to_wide_null(image.program.as_os_str());
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                windows_sys::Win32::Foundation::GENERIC_READ | GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("decoder image creation"));
        }
        let mut destination = HandleGuard::new(handle).into_file();
        let mut limited = source.take(length + 1);
        if std::io::copy(&mut limited, &mut destination)? != length {
            return Err(invalid());
        }
        destination.flush()?;
        destination.sync_all()?;
        drop(destination);
        // A noninheritable handle denies writes/deletion for the entire Pool and
        // every outstanding child, including cancellation/reaping.
        image.image_lock = Some(open(
            &image.program,
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ,
        )?);
        Ok(image)
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        self.image_lock.take();
        // Exact paths only, never recursive cleanup of a mutable caller tree.
        let _ = fs::remove_file(&self.program);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn open(path: &Path, access: u32, sharing: u32) -> Result<File> {
    let name = to_wide_null(path.as_os_str());
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            sharing,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("decoder protected path"));
    }
    let file = HandleGuard::new(handle).into_file();
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid());
    }
    Ok(file)
}

fn pin_path(path: &Path) -> Result<Vec<File>> {
    // Local drive paths only. Pin each ancestor against rename/deletion so an
    // unprivileged junction/parent swap cannot redirect subsequent path APIs.
    let text = path.to_str().ok_or_else(invalid)?;
    let text = text.strip_prefix("\\\\?\\").unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() < 4
        || bytes.len() > 30000
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":\\"
        || text[3..].contains(':')
        || text.contains('/')
        || text.contains('\0')
    {
        return Err(invalid());
    }
    let mut built = PathBuf::from(&text[..3]);
    let mut handles = vec![open(
        &built,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )?];
    let components = text[3..].split('\\').collect::<Vec<_>>();
    if components.len() > 128 {
        return Err(invalid());
    }
    for component in components {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with([' ', '.'])
        {
            return Err(invalid());
        }
        built.push(component);
        handles.push(open(
            &built,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?);
    }
    Ok(handles)
}

struct Local(*mut core::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn sid_text(sid: PSID) -> Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(invalid());
    }
    let _memory = Local(text as _);
    let mut length = 0;
    while length < 256 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    if length == 256 {
        return Err(invalid());
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) }).map_err(|_| invalid())
}
fn descriptor(owner: &[u8], capability: &[u8]) -> Result<Local> {
    let user = sid_text(owner.as_ptr() as PSID)?;
    let cap = sid_text(capability.as_ptr() as PSID)?;
    let sddl = to_wide_null(format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user})(A;OICI;FRFX;;;{cap})"
    ));
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(invalid());
    }
    Ok(Local(descriptor))
}
fn fixed_sid(text: &str) -> Result<Local> {
    let mut sid = null_mut();
    if unsafe { ConvertStringSidToSidW(to_wide_null(text).as_ptr(), &mut sid) } == 0 {
        return Err(invalid());
    }
    Ok(Local(sid))
}
fn validate_acl(file: &File, owner: &[u8], private: bool) -> Result<()> {
    let system = fixed_sid("S-1-5-18")?;
    let admins = fixed_sid("S-1-5-32-544")?;
    let trusted = |sid: PSID| unsafe {
        EqualSid(sid, owner.as_ptr() as PSID) != 0
            || EqualSid(sid, system.0) != 0
            || EqualSid(sid, admins.0) != 0
    };
    let mut sid = null_mut();
    let mut acl = null_mut();
    let mut descriptor = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut sid,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(invalid());
    }
    let _memory = Local(descriptor);
    if sid.is_null() || acl.is_null() || !trusted(sid) {
        return Err(invalid());
    }
    let dangerous = windows_sys::Win32::Foundation::GENERIC_ALL
        | GENERIC_WRITE
        | DELETE
        | 0x40000
        | 0x80000
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | FILE_DELETE_CHILD;
    for i in 0..unsafe { (*acl).AceCount } as u32 {
        let mut ace = null_mut();
        if unsafe { GetAce(acl, i, &mut ace) } == 0 || ace.is_null() {
            return Err(invalid());
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceFlags & 8 != 0 {
            continue;
        } // INHERIT_ONLY does not grant this object.
        if header.AceType as u32 == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        // An allowed ACE has an eight-byte header/mask followed by a complete
        // SID. Bound its variable-length subauthorities before SID APIs read it.
        if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE || header.AceSize < 16 {
            return Err(invalid());
        }
        let sid_bytes = unsafe {
            std::slice::from_raw_parts((ace as *const u8).add(8), header.AceSize as usize - 8)
        };
        if sid_bytes[0] != 1
            || sid_bytes[1] > 15
            || 8 + usize::from(sid_bytes[1]) * 4 > sid_bytes.len()
        {
            return Err(invalid());
        }
        let ace = ace as *const ACCESS_ALLOWED_ACE;
        let sid = unsafe { std::ptr::addr_of!((*ace).SidStart) } as PSID;
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(invalid());
        }
        if !trusted(sid) && (private || unsafe { (*ace).Mask } & dangerous != 0) {
            return Err(invalid());
        }
    }
    Ok(())
}
