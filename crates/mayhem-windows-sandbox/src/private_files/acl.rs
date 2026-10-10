use super::*;
use std::os::windows::io::OwnedHandle;
use windows_sys::Win32::{
    Security::{
        Authorization::{ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT},
        *,
    },
    System::{
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};
struct Local(*mut core::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn fixed(value: &str) -> Result<Local> {
    let mut sid = null_mut();
    if unsafe { ConvertStringSidToSidW(wide(value).as_ptr(), &mut sid) } == 0 {
        return Err(invalid());
    }
    Ok(Local(sid))
}
pub(super) fn current_user() -> Result<Vec<u8>> {
    let mut handle = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(invalid());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(handle.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
    }
    if size == 0 || size > 16384 {
        return Err(invalid());
    }
    let mut bytes = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            handle.as_raw_handle(),
            TokenUser,
            bytes.as_mut_ptr() as _,
            size,
            &mut size,
        )
    } == 0
        || (size as usize) < size_of::<TOKEN_USER>()
    {
        return Err(invalid());
    }
    let user = unsafe { &*(bytes.as_ptr() as *const TOKEN_USER) };
    if unsafe { IsValidSid(user.User.Sid) } == 0 {
        return Err(invalid());
    }
    let size = unsafe { GetLengthSid(user.User.Sid) };
    if size < 8 || size > 68 {
        return Err(invalid());
    }
    let mut owned = vec![0u8; size as usize];
    if unsafe { CopySid(size, owned.as_mut_ptr() as _, user.User.Sid) } == 0 {
        return Err(invalid());
    }
    Ok(owned)
}
pub(super) fn validate(file: &File, current: &[u8], private: bool) -> Result<()> {
    let system = fixed("S-1-5-18")?;
    let admins = fixed("S-1-5-32-544")?;
    // Windows Resource Protection owns ordinary system ancestors under this
    // exact service SID. This does not trust arbitrary services or authorize
    // a service-owned/readable private leaf.
    let installer = fixed("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464")?;
    let trusted = |sid: PSID| unsafe {
        EqualSid(sid, current.as_ptr() as _) != 0
            || EqualSid(sid, system.0) != 0
            || EqualSid(sid, admins.0) != 0
            || (!private && EqualSid(sid, installer.0) != 0)
    };
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    if unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    } != 0
    {
        return Err(invalid());
    }
    let _descriptor = Local(descriptor);
    if descriptor.is_null()
        || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
        || owner.is_null()
        || unsafe { IsValidSid(owner) } == 0
        || dacl.is_null()
        || unsafe { IsValidAcl(dacl) } == 0
        || !trusted(owner)
        || (private && unsafe { EqualSid(owner, current.as_ptr() as _) } == 0)
    {
        return Err(invalid());
    }
    // Parent object creation rights alone cannot replace pinned existing children.
    // Deletion, ACL/owner changes and attribute/EA mutation are different: refuse
    // those grants to untrusted users even on a publicly traversable ancestor.
    let mutate = GENERIC_ALL
        | GENERIC_WRITE
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | FILE_WRITE_EA;
    for index in 0..unsafe { (*dacl).AceCount } as u32 {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(invalid());
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
            continue;
        }
        if header.AceType as u32 == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE || header.AceSize < 16 {
            return Err(invalid());
        }
        let data = unsafe {
            std::slice::from_raw_parts((ace as *const u8).add(8), header.AceSize as usize - 8)
        };
        if data[0] != 1 || data[1] > 15 || 8 + usize::from(data[1]) * 4 > data.len() {
            return Err(invalid());
        }
        let allow = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        let sid = std::ptr::addr_of!(allow.SidStart) as PSID;
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(invalid());
        }
        if !trusted(sid) && allow.Mask != 0 {
            let known =
                FILE_ALL_ACCESS | GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL;
            if private || allow.Mask & mutate != 0 || allow.Mask & !known != 0 {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
