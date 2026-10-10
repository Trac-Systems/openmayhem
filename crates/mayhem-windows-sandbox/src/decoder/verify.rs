use super::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_CHAR, FILE_TYPE_PIPE};
use windows_sys::Win32::System::Console::{STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::JobObjects::*;

/// Called by the trusted bundled executable before reading any untrusted IPC.
/// A normal/unrestricted launch cannot imitate this with environment variables.
pub fn verify_decoder_process() -> Result<()> {
    verify_process(None)
}

/// Tokenizer initialization requires its exact, stricter Job ceiling in
/// addition to every decoder confinement check. A decoder-mode launch must not
/// substitute its larger memory allowance for the tokenizer policy.
pub fn verify_tokenizer_process() -> Result<()> {
    verify_process(Some(TOKENIZER_COMMIT_BYTES))
}

fn verify_process(exact_memory: Option<usize>) -> Result<()> {
    if !cfg!(all(
        target_pointer_width = "64",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )) {
        return Err(invalid());
    }
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(invalid());
    }
    let token = HandleGuard::new(token);
    if token_u32(token.handle, TokenIsAppContainer)? != 1
        || token_u32(token.handle, TokenIsLessPrivilegedAppContainer)? != 1
    {
        return Err(invalid());
    }
    let caps = information(token.handle, TokenCapabilities)?;
    if std::mem::size_of_val(caps.as_slice()) < size_of::<TOKEN_GROUPS>() {
        return Err(invalid());
    }
    let groups = unsafe { &*(caps.as_ptr() as *const TOKEN_GROUPS) };
    if groups.GroupCount != 1 {
        return Err(invalid());
    }
    let cap = groups.Groups[0];
    if cap.Attributes & SE_GROUP_ENABLED as u32 == 0 || unsafe { IsValidSid(cap.Sid) } == 0 {
        return Err(invalid());
    }
    // Exactly a derived named capability; built-in Internet/private-network,
    // authentication, device and application capabilities have different SIDs.
    let authority = unsafe { GetSidIdentifierAuthority(cap.Sid) };
    if authority.is_null()
        || unsafe { (*authority).Value } != [0, 0, 0, 0, 0, 15]
        || unsafe { *GetSidSubAuthorityCount(cap.Sid) } != 10
        || unsafe { *GetSidSubAuthority(cap.Sid, 0) } != 3
        || unsafe { *GetSidSubAuthority(cap.Sid, 1) } != 1024
    {
        return Err(invalid());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    if unsafe {
        QueryInformationJobObject(
            null_mut(),
            JobObjectExtendedLimitInformation,
            &mut limits as *mut _ as _,
            size_of_val(&limits) as u32,
            null_mut(),
        )
    } == 0
    {
        return Err(invalid());
    }
    let flags = limits.BasicLimitInformation.LimitFlags;
    let required = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_JOB_MEMORY;
    if flags & required != required
        || flags & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK) != 0
        || limits.BasicLimitInformation.ActiveProcessLimit != 1
        || ![DECODER_COMMIT_BYTES, TOKENIZER_COMMIT_BYTES].contains(&limits.ProcessMemoryLimit)
        || limits.JobMemoryLimit != limits.ProcessMemoryLimit
        || exact_memory.is_some_and(|bytes| limits.ProcessMemoryLimit != bytes)
    {
        return Err(invalid());
    }
    for (which, expected) in [
        (STD_INPUT_HANDLE, FILE_TYPE_PIPE),
        (STD_OUTPUT_HANDLE, FILE_TYPE_PIPE),
        (STD_ERROR_HANDLE, FILE_TYPE_CHAR),
    ] {
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null()
            || handle == INVALID_HANDLE_VALUE
            || unsafe { GetFileType(handle) } != expected
            || unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0
        {
            return Err(invalid());
        }
    }
    // Verify the immutable creation policies, before reading caller bytes. DWORD
    // flag layouts are defined by the matching Windows mitigation structures.
    for (policy, flags) in [
        (ProcessDynamicCodePolicy, 1u32),
        (ProcessSystemCallDisablePolicy, 1u32),
        (ProcessStrictHandleCheckPolicy, 3u32),
        (ProcessExtensionPointDisablePolicy, 1u32),
        (ProcessImageLoadPolicy, 7u32),
    ] {
        let mut observed = 0u32;
        if unsafe {
            GetProcessMitigationPolicy(
                GetCurrentProcess(),
                policy,
                &mut observed as *mut _ as _,
                size_of_val(&observed),
            )
        } == 0
            || observed != flags
        {
            return Err(invalid());
        }
    }
    let mut child_policy = 0u32;
    if unsafe {
        GetProcessMitigationPolicy(
            GetCurrentProcess(),
            ProcessChildProcessPolicy,
            &mut child_policy as *mut _ as _,
            size_of_val(&child_policy),
        )
    } == 0
        || child_policy & 1 != 1
    {
        return Err(invalid());
    }
    Ok(())
}

fn information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token, class, null_mut(), 0, &mut size);
    }
    if size == 0 || size > 16384 {
        return Err(invalid());
    }
    let mut bytes = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, bytes.as_mut_ptr() as _, size, &mut size) } == 0 {
        return Err(invalid());
    }
    Ok(bytes)
}
fn token_u32(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<u32> {
    let value = information(token, class)?;
    if std::mem::size_of_val(value.as_slice()) < size_of::<u32>() {
        return Err(invalid());
    }
    Ok(unsafe { *(value.as_ptr() as *const u32) })
}
