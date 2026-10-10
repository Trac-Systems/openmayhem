//! Strict bundled-parser launcher. No provider-selected commands, capabilities,
//! writable directories, network grants or unrestricted fallback.
use super::*;
use std::mem::size_of_val;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::Arc;
use windows_sys::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount};
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::WindowsProgramming::{
    PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT, PROCESS_CREATION_CHILD_PROCESS_RESTRICTED,
};

mod identity;
mod storage;
mod verify;
pub use verify::{verify_decoder_process, verify_tokenizer_process};

// Trusted host ceilings: process committed memory, not RSS or IPC reservation.
// No total CPU/generation timer is installed. Tokenizer uses a distinct ceiling.
const DECODER_COMMIT_BYTES: usize = 1024 * 1024 * 1024;
// Separate from the tokenizer's 512 MiB counted heap: leave 256 MiB for stacks,
// runtime/loader allocations and other committed pages; still one fixed Job cap.
const TOKENIZER_COMMIT_BYTES: usize = 768 * 1024 * 1024;

// SDK PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY masks. Apply at creation, so
// image initialization cannot run before the policies are active. Unsupported
// kernels refuse the launch; never mask out unsupported protections.
const CREATION_MITIGATIONS: u64 = (1 << 24) // strict handles
    | (1 << 28) // no Win32k system calls
    | (1 << 32) // no third-party extension points
    | (1 << 36) // no dynamic executable code
    | (1 << 52) // no remote images
    | (1 << 56) // no low-integrity images
    | (1 << 60); // prefer System32 image resolution

#[derive(Clone, Copy)]
pub enum DecoderMode {
    Decoder,
    Tokenizer,
}
impl DecoderMode {
    fn argument(self) -> &'static str {
        match self {
            Self::Decoder => "--stdio-v1",
            Self::Tokenizer => "--tokenizer-stdio-v2",
        }
    }
    fn memory(self) -> usize {
        match self {
            Self::Decoder => DECODER_COMMIT_BYTES,
            Self::Tokenizer => TOKENIZER_COMMIT_BYTES,
        }
    }
}

/// One immutable staged image per trusted Pool, no artifact/model data on disk.
#[derive(Clone)]
pub struct DecoderLauncher {
    image: Arc<storage::Image>,
}
impl DecoderLauncher {
    pub fn new(program: &Path, workdir: &Path) -> Result<Self> {
        if !cfg!(all(
            target_pointer_width = "64",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            return Err(invalid());
        }
        Ok(Self {
            image: Arc::new(storage::Image::new(program, workdir)?),
        })
    }
    pub fn spawn(&self, mode: DecoderMode) -> Result<DecoderChild> {
        launch(self.image.clone(), mode, &[OsString::from(mode.argument())])
    }
}

pub struct DecoderChild {
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    process: OwnedHandle,
    job: OwnedHandle,
    code: Option<u32>,
    _image: Arc<storage::Image>,
}
impl DecoderChild {
    pub fn try_wait(&mut self) -> Result<Option<u32>> {
        if self.code.is_some() {
            return Ok(self.code);
        }
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            0 => {
                let code = process_exit_code(self.process.as_raw_handle())?;
                self.code = Some(code);
                Ok(self.code)
            }
            _ => Err(last_error("decoder wait")),
        }
    }
    pub fn kill(&mut self) -> Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 2) } == 0 {
            return Err(last_error("decoder job termination"));
        }
        Ok(())
    }
}
impl Drop for DecoderChild {
    fn drop(&mut self) {
        if self.code.is_none() {
            // Also covers failure before assignment/resume. This parent owns a
            // full-access process handle; never leave a suspended orphan.
            unsafe {
                TerminateJobObject(self.job.as_raw_handle(), 2);
                TerminateProcess(self.process.as_raw_handle(), 2);
                WaitForSingleObject(self.process.as_raw_handle(), INFINITE);
            }
        }
    }
}

fn nonce() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| WindowsSandboxError::InvalidConfig("decoder entropy unavailable".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn owned(guard: HandleGuard) -> OwnedHandle {
    guard.into_file().into()
}
fn invalid() -> WindowsSandboxError {
    WindowsSandboxError::InvalidConfig("decoder containment precondition failed".into())
}

fn launch(
    image: Arc<storage::Image>,
    mode: DecoderMode,
    args: &[OsString],
) -> Result<DecoderChild> {
    // Register only the identity required by CreateProcess, never a writable
    // AppContainer profile. Registration is removed before the child resumes.
    // Windows limits AppContainer names to 64 characters. The generated
    // name reserves a fixed prefix and over 200 bits of independent entropy.
    let name = identity::name()?;
    let mut sid = null_mut();
    let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if hr < 0 || sid.is_null() {
        return Err(invalid());
    }
    let sid = SidGuard::new(sid);
    let mut capability = SID_AND_ATTRIBUTES {
        Sid: image.capability.as_ptr() as PSID,
        Attributes: SE_GROUP_ENABLED as u32,
    };
    let mut caps = SECURITY_CAPABILITIES {
        AppContainerSid: sid.as_ptr(),
        Capabilities: &mut capability,
        CapabilityCount: 1,
        Reserved: 0,
    };
    let input = child_pipe(PipeDirection::ChildReads)?;
    let output = child_pipe(PipeDirection::ChildWrites)?;
    let stderr = inheritable_null_handle()?;
    let handles = [input.child.handle, output.child.handle, stderr.handle];
    let mut attributes = AttributeList::new(6)?;
    attributes.update_security_capabilities(&mut caps)?;
    attributes.update_handle_list(&handles)?;
    let lpac = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let no_child = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    for (key, value) in [
        (PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, &lpac),
        (PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, &no_child),
    ] {
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.ptr,
                0,
                key as usize,
                value as *const u32 as _,
                size_of::<u32>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(last_error("decoder restricted attribute"));
        }
    }
    let mitigations = CREATION_MITIGATIONS;
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.ptr,
            0,
            PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
            &mitigations as *const u64 as _,
            size_of_val(&mitigations),
            null_mut(),
            null(),
        )
    } == 0
    {
        return Err(last_error("decoder creation mitigations"));
    }
    let job = create_job_handle(Some(mode.memory() as u64))?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_JOB_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = mode.memory();
    limits.JobMemoryLimit = mode.memory();
    if unsafe {
        SetInformationJobObject(
            job.handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(last_error("decoder job limits"));
    }
    // Assign the kill-on-close Job atomically at creation. Parent death in the
    // suspended-startup window must not leave an unowned suspended process.
    let jobs = [job.handle];
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.ptr,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr() as _,
            size_of_val(&jobs),
            null_mut(),
            null(),
        )
    } == 0
    {
        return Err(last_error("decoder creation job"));
    }
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.as_mut_ptr();
    let program = to_wide_null(image.program.as_os_str());
    // CreateProcess rejects a current directory above MAX_PATH even when the
    // executable supports long paths. This stdio-only worker needs no working
    // files. Use the OS directory already required for its system DLLs, never
    // an inherited/user-selected directory. No capability or write ACL changes.
    let mut directory = [0u16; 260];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            directory.as_mut_ptr(),
            directory.len() as u32,
        )
    } as usize;
    if length == 0 || length >= directory.len() - 1 {
        return Err(invalid());
    }
    let mut command = windows_command_line_os(image.program.as_os_str(), args);
    let environment = identity::environment()?;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut registration = identity::Registration::new(&sid, &name)?;
    if unsafe {
        CreateProcessW(
            program.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | DETACHED_PROCESS,
            environment.as_ptr() as _,
            directory.as_ptr(),
            &startup as *const _ as _,
            &mut process,
        )
    } == 0
    {
        return Err(last_error("decoder LPAC creation"));
    }
    let thread = HandleGuard::new(process.hThread);
    let child = DecoderChild {
        stdin: Some(input.parent),
        stdout: Some(output.parent),
        process: owned(HandleGuard::new(process.hProcess)),
        job: owned(job),
        code: None,
        _image: image,
    };
    let mut in_job = 0;
    if unsafe {
        windows_sys::Win32::System::JobObjects::IsProcessInJob(
            child.process.as_raw_handle(),
            child.job.as_raw_handle(),
            &mut in_job,
        )
    } == 0
        || in_job == 0
    {
        return Err(last_error("decoder creation job verification"));
    }
    #[cfg(test)]
    tests::crash_after_creation(&child, &name);
    // Windows has copied the identity into the process token. Retain no
    // registration or writable profile while processing untrusted bytes.
    registration.remove()?;
    if unsafe { ResumeThread(thread.handle) } == u32::MAX {
        return Err(last_error("decoder resume"));
    }
    Ok(child)
}

#[cfg(test)]
mod tests;
