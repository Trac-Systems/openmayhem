use std::ffi::OsStr;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL, WAIT_FAILED,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    FreeSid, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, OBJECT_INHERIT_ACE,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTUPINFOEXW, STARTUPINFOW,
};

use crate::{Result, WindowsSandboxConfig, WindowsSandboxError, WindowsSandboxRunReport};

pub fn run_appcontainer(
    config: &WindowsSandboxConfig,
    command: &[String],
) -> Result<WindowsSandboxRunReport> {
    if command.is_empty() {
        return Err(WindowsSandboxError::EmptyCommand);
    }

    let sealed_store = std::fs::canonicalize(&config.sealed_store_dir)?;
    let profile_name = unique_profile_name(&sealed_store);
    let profile = AppContainerProfile::create(&profile_name)?;

    grant_appcontainer_read(&profile.sid, &sealed_store, true, false)?;
    if let Some(exe) = command_executable_path(&command[0]) {
        grant_appcontainer_read(&profile.sid, &exe, false, true)?;
    }

    let report = launch(profile.sid.as_ptr(), command, config.memory_limit_bytes);
    let delete_result = profile.delete();
    match (report, delete_result) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

fn launch(
    appcontainer_sid: PSID,
    command: &[String],
    memory_limit_bytes: Option<u64>,
) -> Result<WindowsSandboxRunReport> {
    let mut attr = AttributeList::new(1)?;
    let mut caps = SECURITY_CAPABILITIES {
        AppContainerSid: appcontainer_sid,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    attr.update_security_capabilities(&mut caps)?;

    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attr.as_mut_ptr();

    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut command_line = to_wide_null(&windows_command_line(command));
    let creation_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED;

    let ok = unsafe {
        CreateProcessW(
            null(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            0,
            creation_flags,
            null(),
            null(),
            &startup as *const STARTUPINFOEXW as *const STARTUPINFOW,
            &mut process,
        )
    };
    if ok == 0 {
        return Err(last_error("CreateProcessW(AppContainer)"));
    }

    let process_handle = HandleGuard::new(process.hProcess);
    let thread_handle = HandleGuard::new(process.hThread);
    let job = create_job(memory_limit_bytes)?;
    if let Some(job) = &job {
        let ok = unsafe { AssignProcessToJobObject(job.handle, process_handle.handle) };
        if ok == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
    }
    let resumed = unsafe { ResumeThread(thread_handle.handle) };
    if resumed == u32::MAX {
        return Err(last_error("ResumeThread"));
    }
    let wait = unsafe { WaitForSingleObject(process_handle.handle, INFINITE) };
    if wait == WAIT_FAILED {
        return Err(last_error("WaitForSingleObject"));
    }
    let mut exit_code = 1_u32;
    let ok = unsafe { GetExitCodeProcess(process_handle.handle, &mut exit_code) };
    if ok == 0 {
        return Err(last_error("GetExitCodeProcess"));
    }
    drop(job);
    Ok(WindowsSandboxRunReport {
        status_code: exit_code,
    })
}

fn create_job(memory_limit_bytes: Option<u64>) -> Result<Option<HandleGuard>> {
    let Some(limit_bytes) = memory_limit_bytes.filter(|bytes| *bytes > 0) else {
        return Ok(None);
    };
    let limit = usize::try_from(limit_bytes).map_err(|_| {
        WindowsSandboxError::Windows(format!(
            "memory limit {limit_bytes} exceeds Windows process address size"
        ))
    })?;
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    let job = HandleGuard::new(handle);
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    info.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    info.ProcessMemoryLimit = limit;
    let ok = unsafe {
        SetInformationJobObject(
            job.handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error("SetInformationJobObject"));
    }
    Ok(Some(job))
}

fn grant_appcontainer_read(
    appcontainer_sid: &SidGuard,
    path: &Path,
    inherit: bool,
    execute: bool,
) -> Result<()> {
    let path_w = to_wide_null(path.as_os_str());
    let mut old_dacl: *mut ACL = null_mut();
    let mut security_descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let err = unsafe {
        GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old_dacl,
            null_mut(),
            &mut security_descriptor,
        )
    };
    if err != 0 {
        return Err(win32_error("GetNamedSecurityInfoW", err));
    }

    let mut access = EXPLICIT_ACCESS_W::default();
    access.grfAccessPermissions =
        FILE_GENERIC_READ | if execute { FILE_GENERIC_EXECUTE } else { 0 };
    access.grfAccessMode = GRANT_ACCESS;
    access.grfInheritance = if inherit {
        SUB_CONTAINERS_AND_OBJECTS_INHERIT | OBJECT_INHERIT_ACE
    } else {
        NO_INHERITANCE
    };
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
    access.Trustee.ptstrName = appcontainer_sid.as_ptr() as PWSTR;

    let mut new_dacl: *mut ACL = null_mut();
    let err = unsafe { SetEntriesInAclW(1, &access, old_dacl, &mut new_dacl) };
    if err != 0 {
        unsafe {
            LocalFree(security_descriptor as HLOCAL);
        }
        return Err(win32_error("SetEntriesInAclW", err));
    }

    let err = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new_dacl,
            null_mut(),
        )
    };
    unsafe {
        LocalFree(new_dacl as HLOCAL);
        LocalFree(security_descriptor as HLOCAL);
    }
    if err != 0 {
        return Err(win32_error("SetNamedSecurityInfoW", err));
    }
    Ok(())
}

struct AppContainerProfile {
    name: Vec<u16>,
    sid: SidGuard,
}

impl AppContainerProfile {
    fn create(name: &str) -> Result<Self> {
        let name_w = to_wide_null(name);
        let display = to_wide_null("Mayhem enclave sandbox");
        let description =
            to_wide_null("Mayhem Windows AppContainer for one sandboxed enclave process");
        let mut sid: PSID = null_mut();
        let hr = unsafe {
            CreateAppContainerProfile(
                name_w.as_ptr(),
                display.as_ptr(),
                description.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if hr_failed(hr) && hr as u32 != hresult_from_win32(ERROR_ALREADY_EXISTS) {
            return Err(hresult_error("CreateAppContainerProfile", hr));
        }
        if hr_failed(hr) {
            let derive_hr =
                unsafe { DeriveAppContainerSidFromAppContainerName(name_w.as_ptr(), &mut sid) };
            if hr_failed(derive_hr) {
                return Err(hresult_error(
                    "DeriveAppContainerSidFromAppContainerName",
                    derive_hr,
                ));
            }
        }
        Ok(Self {
            name: name_w,
            sid: SidGuard::new(sid),
        })
    }

    fn delete(&self) -> Result<()> {
        let hr = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
        if hr_failed(hr) {
            return Err(hresult_error("DeleteAppContainerProfile", hr));
        }
        Ok(())
    }
}

struct SidGuard {
    sid: PSID,
}

impl SidGuard {
    fn new(sid: PSID) -> Self {
        Self { sid }
    }

    fn as_ptr(&self) -> PSID {
        self.sid
    }
}

impl Drop for SidGuard {
    fn drop(&mut self) {
        if !self.sid.is_null() {
            unsafe {
                FreeSid(self.sid);
            }
        }
    }
}

struct HandleGuard {
    handle: HANDLE,
}

impl HandleGuard {
    fn new(handle: HANDLE) -> Self {
        Self { handle }
    }
}

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

struct AttributeList {
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> Result<Self> {
        let mut size = 0_usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size);
        }
        if size == 0 {
            return Err(last_error("InitializeProcThreadAttributeList(size)"));
        }
        let words = size.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let ptr = storage.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        let ok = unsafe { InitializeProcThreadAttributeList(ptr, count, 0, &mut size) };
        if ok == 0 {
            return Err(last_error("InitializeProcThreadAttributeList"));
        }
        Ok(Self {
            ptr,
            _storage: storage,
        })
    }

    fn update_security_capabilities(&mut self, caps: &mut SECURITY_CAPABILITIES) -> Result<()> {
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.ptr,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                caps as *mut SECURITY_CAPABILITIES as *const _,
                size_of::<SECURITY_CAPABILITIES>(),
                null_mut(),
                null(),
            )
        };
        if ok == 0 {
            return Err(last_error("UpdateProcThreadAttribute"));
        }
        Ok(())
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.ptr
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                DeleteProcThreadAttributeList(self.ptr);
            }
        }
    }
}

fn command_executable_path(program: &str) -> Option<PathBuf> {
    let path = PathBuf::from(program);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    path.exists()
        .then(|| std::fs::canonicalize(&path).unwrap_or(path))
}

fn windows_command_line(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    if !arg.bytes().any(|byte| matches!(byte, b' ' | b'\t' | b'"')) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0_usize;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
        } else if ch == '"' {
            out.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
            out.push('"');
            backslashes = 0;
        } else {
            out.extend(std::iter::repeat('\\').take(backslashes));
            backslashes = 0;
            out.push(ch);
        }
    }
    out.extend(std::iter::repeat('\\').take(backslashes * 2));
    out.push('"');
    out
}

fn unique_profile_name(sealed_store: &Path) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    let tail = sealed_store
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(16)
        .collect::<String>();
    format!("mayhem.enclave.{pid}.{now}.{tail}")
}

fn to_wide_null(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn hr_failed(hr: i32) -> bool {
    hr < 0
}

fn hresult_from_win32(code: u32) -> u32 {
    if code <= 0 {
        code
    } else {
        (code & 0x0000_FFFF) | 0x8007_0000
    }
}

fn hresult_error(api: &str, hr: i32) -> WindowsSandboxError {
    WindowsSandboxError::Windows(format!("{api} failed with HRESULT 0x{:08X}", hr as u32))
}

fn win32_error(api: &str, code: u32) -> WindowsSandboxError {
    WindowsSandboxError::Windows(format!("{api} failed with Win32 error {code}"))
}

fn last_error(api: &str) -> WindowsSandboxError {
    let code = unsafe { GetLastError() };
    WindowsSandboxError::Windows(format!("{api} failed with Win32 error {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_command_line_quotes_spaces_and_quotes() {
        assert_eq!(
            windows_command_line(&[
                "C:\\Program Files\\Mayhem\\mayhem.exe".to_owned(),
                "hello \"world\"".to_owned(),
            ]),
            r#""C:\Program Files\Mayhem\mayhem.exe" "hello \"world\"""#
        );
    }
}
