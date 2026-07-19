use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, GetLastError, LocalFree, SetHandleInformation,
    DUPLICATE_SAME_ACCESS, ERROR_ALREADY_EXISTS, GENERIC_ALL, GENERIC_WRITE, HANDLE,
    HANDLE_FLAG_INHERIT, HLOCAL, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    CopySid, DeriveCapabilitySidsFromName, FreeSid, GetLengthSid, GetTokenInformation,
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, TokenUser, ACL,
    DACL_SECURITY_INFORMATION, NO_INHERITANCE, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SECURITY_DESCRIPTOR, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW,
    GetProcessWindowStation, SetProcessWindowStation, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW,
    DESKTOP_ENUMERATE, DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS, HDESK,
    HWINSTA,
};
use windows_sys::Win32::System::SystemServices::{SECURITY_DESCRIPTOR_REVISION, SE_GROUP_ENABLED};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetCurrentProcessId,
    GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken, ResumeThread,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WINSTA_ACCESSCLIPBOARD, WINSTA_ACCESSGLOBALATOMS, WINSTA_CREATEDESKTOP, WINSTA_EXITWINDOWS,
    WINSTA_READATTRIBUTES,
};

use crate::{
    Result, WindowsSandboxCommand, WindowsSandboxConfig, WindowsSandboxError,
    WindowsSandboxRunReport, WindowsSandboxStderr,
};

const READ_CONTROL_ACCESS: u32 = 0x0002_0000;
const WINDOW_STATION_CHILD_ACCESS: u32 = READ_CONTROL_ACCESS
    | WINSTA_ACCESSCLIPBOARD as u32
    | WINSTA_ACCESSGLOBALATOMS as u32
    | WINSTA_CREATEDESKTOP as u32
    | WINSTA_EXITWINDOWS as u32
    | WINSTA_READATTRIBUTES as u32;
const DESKTOP_CHILD_ACCESS: u32 = READ_CONTROL_ACCESS
    | DESKTOP_CREATEMENU
    | DESKTOP_CREATEWINDOW
    | DESKTOP_ENUMERATE
    | DESKTOP_HOOKCONTROL
    | DESKTOP_READOBJECTS
    | DESKTOP_WRITEOBJECTS;
static WINDOW_STATION_SWITCH_LOCK: Mutex<()> = Mutex::new(());

pub struct WindowsSandboxChild {
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    process: HandleGuard,
    job: HandleGuard,
    _profile: AppContainerProfile,
    _desktop: PrivateDesktop,
    id: u32,
    exit_code: Option<u32>,
}

struct ResolvedWindowsSandboxConfig {
    read_only_dirs: Vec<PathBuf>,
    writable_dirs: Vec<PathBuf>,
    memory_limit_bytes: Option<u64>,
}

impl WindowsSandboxChild {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn try_wait(&mut self) -> Result<Option<u32>> {
        if let Some(exit_code) = self.exit_code {
            return Ok(Some(exit_code));
        }
        let wait = unsafe { WaitForSingleObject(self.process.handle, 0) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait == WAIT_FAILED {
            return Err(last_error("WaitForSingleObject"));
        }
        let exit_code = process_exit_code(self.process.handle)?;
        self.exit_code = Some(exit_code);
        Ok(Some(exit_code))
    }

    pub fn kill(&mut self) -> Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        let ok = unsafe { TerminateJobObject(self.job.handle, 1) };
        if ok == 0 {
            return Err(last_error("TerminateJobObject"));
        }
        Ok(())
    }

    pub fn wait(&mut self) -> Result<u32> {
        self.stdin.take();
        if let Some(exit_code) = self.exit_code {
            return Ok(exit_code);
        }
        let wait = unsafe { WaitForSingleObject(self.process.handle, INFINITE) };
        if wait == WAIT_FAILED {
            return Err(last_error("WaitForSingleObject"));
        }
        let exit_code = process_exit_code(self.process.handle)?;
        self.exit_code = Some(exit_code);
        Ok(exit_code)
    }
}

pub fn spawn_appcontainer(
    config: &WindowsSandboxConfig,
    command: &WindowsSandboxCommand,
) -> Result<WindowsSandboxChild> {
    if command.program.is_empty() {
        return Err(WindowsSandboxError::EmptyCommand);
    }

    let config = resolve_windows_sandbox_config(config)?;
    let profile_name = unique_profile_name(&config.read_only_dirs[0]);
    let mut profile_capability_sids = command_capability_sids(command)?;
    let mut profile_capabilities = capability_attributes(&mut profile_capability_sids);
    let profile = AppContainerProfile::create(&profile_name, &mut profile_capabilities)?;
    drop(profile_capabilities);
    drop(profile_capability_sids);

    grant_configured_directory_access(&profile.sid, &config)?;
    grant_command_executable_access(&profile.sid, &config, command)?;
    if let Some(exe) = command_executable_path_os(&command.program, command.current_dir.as_deref())
    {
        grant_appcontainer_access(
            &profile.sid,
            &exe,
            false,
            AppContainerFileAccess::Executable,
        )?;
    }

    let desktop = PrivateDesktop::create(&profile.sid)?;
    launch_child(profile, desktop, command, config.memory_limit_bytes)
}

pub fn run_appcontainer(
    config: &WindowsSandboxConfig,
    command: &[String],
) -> Result<WindowsSandboxRunReport> {
    if command.is_empty() {
        return Err(WindowsSandboxError::EmptyCommand);
    }

    let config = resolve_windows_sandbox_config(config)?;
    let profile_name = unique_profile_name(&config.read_only_dirs[0]);
    let mut profile = AppContainerProfile::create(&profile_name, &mut [])?;

    grant_configured_directory_access(&profile.sid, &config)?;
    if let Some(exe) = command_executable_path(&command[0]) {
        grant_appcontainer_access(
            &profile.sid,
            &exe,
            false,
            AppContainerFileAccess::Executable,
        )?;
    }

    let mut desktop = PrivateDesktop::create(&profile.sid)?;
    let report = launch(
        profile.sid.as_ptr(),
        &mut desktop,
        command,
        config.memory_limit_bytes,
    );
    drop(desktop);
    let delete_result = profile.delete();
    match (report, delete_result) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
    }
}

fn resolve_windows_sandbox_config(
    config: &WindowsSandboxConfig,
) -> Result<ResolvedWindowsSandboxConfig> {
    if config.read_only_dirs.is_empty() {
        return Err(WindowsSandboxError::InvalidConfig(
            "read_only_dirs must contain at least one directory".to_owned(),
        ));
    }
    let read_only_dirs = resolve_windows_sandbox_dirs("read_only_dirs", &config.read_only_dirs)?;
    let writable_dirs = resolve_windows_sandbox_dirs("writable_dirs", &config.writable_dirs)?;
    reject_windows_sandbox_overlaps(&read_only_dirs, &writable_dirs)?;
    Ok(ResolvedWindowsSandboxConfig {
        read_only_dirs,
        writable_dirs,
        memory_limit_bytes: config.memory_limit_bytes,
    })
}

fn resolve_windows_sandbox_dirs(field: &str, dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    dirs.iter()
        .enumerate()
        .map(|(index, path)| resolve_windows_sandbox_dir(field, index, path))
        .collect()
}

fn resolve_windows_sandbox_dir(field: &str, index: usize, path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] cannot be empty"
        )));
    }
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] {} is unavailable: {err}",
            path.display()
        ))
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] {} must not be a symlink or reparse point",
            path.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] {} must be a directory",
            path.display()
        )));
    }
    let canonical = fs::canonicalize(path).map_err(|err| {
        WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] {} could not be canonicalized: {err}",
            path.display()
        ))
    })?;
    reject_windows_tree_reparse_points(field, index, &canonical)?;
    Ok(canonical)
}

fn reject_windows_tree_reparse_points(field: &str, index: usize, path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] tree {} could not be inspected: {err}",
            path.display()
        ))
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(WindowsSandboxError::InvalidConfig(format!(
            "{field}[{index}] tree must not contain symlink or reparse point {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|err| {
            WindowsSandboxError::InvalidConfig(format!(
                "{field}[{index}] tree {} could not be inspected: {err}",
                path.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                WindowsSandboxError::InvalidConfig(format!(
                    "{field}[{index}] tree {} could not be inspected: {err}",
                    path.display()
                ))
            })?;
            reject_windows_tree_reparse_points(field, index, &entry.path())?;
        }
    }
    Ok(())
}

fn reject_windows_sandbox_overlaps(read_only: &[PathBuf], writable: &[PathBuf]) -> Result<()> {
    let dirs = read_only
        .iter()
        .enumerate()
        .map(|(index, path)| ("read_only_dirs", index, path))
        .chain(
            writable
                .iter()
                .enumerate()
                .map(|(index, path)| ("writable_dirs", index, path)),
        )
        .collect::<Vec<_>>();

    for (left_index, (left_field, left_pos, left_path)) in dirs.iter().enumerate() {
        for (right_field, right_pos, right_path) in &dirs[left_index + 1..] {
            if left_path.starts_with(right_path) || right_path.starts_with(left_path) {
                return Err(WindowsSandboxError::InvalidConfig(format!(
                    "{left_field}[{left_pos}] {} overlaps {right_field}[{right_pos}] {}",
                    left_path.display(),
                    right_path.display()
                )));
            }
        }
    }
    Ok(())
}

fn grant_configured_directory_access(
    sid: &SidGuard,
    config: &ResolvedWindowsSandboxConfig,
) -> Result<()> {
    let mut granted_ancestors = Vec::new();
    for path in config
        .read_only_dirs
        .iter()
        .chain(config.writable_dirs.iter())
    {
        for ancestor in path.ancestors().skip(1) {
            if granted_ancestors
                .iter()
                .any(|granted: &PathBuf| granted == ancestor)
            {
                continue;
            }
            grant_appcontainer_access(sid, ancestor, false, AppContainerFileAccess::Traverse)?;
            granted_ancestors.push(ancestor.to_path_buf());
        }
    }
    for path in &config.read_only_dirs {
        grant_appcontainer_access(sid, path, true, AppContainerFileAccess::ReadOnly)?;
    }
    for path in &config.writable_dirs {
        grant_appcontainer_access(sid, path, true, AppContainerFileAccess::Writable)?;
    }
    Ok(())
}

fn grant_command_executable_access(
    sid: &SidGuard,
    config: &ResolvedWindowsSandboxConfig,
    command: &WindowsSandboxCommand,
) -> Result<()> {
    let mut granted = Vec::new();
    for (index, path) in command.executable_read_only_dirs.iter().enumerate() {
        let canonical = fs::canonicalize(path).map_err(|error| {
            WindowsSandboxError::InvalidConfig(format!(
                "executable_read_only_dirs[{index}] {} could not be canonicalized: {error}",
                path.display()
            ))
        })?;
        if !config.read_only_dirs.contains(&canonical) {
            return Err(WindowsSandboxError::InvalidConfig(format!(
                "executable_read_only_dirs[{index}] {} must exactly match a configured read-only directory",
                path.display()
            )));
        }
        if granted.contains(&canonical) {
            continue;
        }
        grant_appcontainer_access(sid, &canonical, true, AppContainerFileAccess::Executable)?;
        granted.push(canonical);
    }
    Ok(())
}

fn launch_child(
    profile: AppContainerProfile,
    mut desktop: PrivateDesktop,
    command: &WindowsSandboxCommand,
    memory_limit_bytes: Option<u64>,
) -> Result<WindowsSandboxChild> {
    let stdin_pipe = child_pipe(PipeDirection::ChildReads)?;
    let stdout_pipe = child_pipe(PipeDirection::ChildWrites)?;
    let (stderr, stderr_handle) = match command.stderr {
        WindowsSandboxStderr::Inherit => (None, inheritable_stderr_handle()?),
        WindowsSandboxStderr::Piped => {
            let pipe = child_pipe(PipeDirection::ChildWrites)?;
            (Some(pipe.parent), pipe.child)
        }
        WindowsSandboxStderr::Null => (None, inheritable_null_handle()?),
    };

    let mut capability_sids = command_capability_sids(command)?;
    let mut capability_attributes = capability_attributes(&mut capability_sids);
    let mut attr = AttributeList::new(2)?;
    let mut caps = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid.as_ptr(),
        Capabilities: if capability_attributes.is_empty() {
            null_mut()
        } else {
            capability_attributes.as_mut_ptr()
        },
        CapabilityCount: capability_attributes.len() as u32,
        Reserved: 0,
    };
    attr.update_security_capabilities(&mut caps)?;
    let inherited_handles = [
        stdin_pipe.child.handle,
        stdout_pipe.child.handle,
        stderr_handle.handle,
    ];
    attr.update_handle_list(&inherited_handles)?;

    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin_pipe.child.handle;
    startup.StartupInfo.hStdOutput = stdout_pipe.child.handle;
    startup.StartupInfo.hStdError = stderr_handle.handle;
    startup.StartupInfo.lpDesktop = desktop.full_name.as_mut_ptr();
    startup.lpAttributeList = attr.as_mut_ptr();

    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut command_line = windows_command_line_os(&command.program, &command.args);
    let environment = windows_environment_block(command);
    let current_dir = command
        .current_dir
        .as_ref()
        .map(|path| to_wide_null(path.as_os_str()));
    let creation_flags =
        EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;

    let ok = unsafe {
        CreateProcessW(
            null(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            creation_flags,
            environment.as_ptr() as *const _,
            current_dir.as_ref().map_or(null(), |path| path.as_ptr()),
            &startup as *const STARTUPINFOEXW as *const STARTUPINFOW,
            &mut process,
        )
    };
    drop(capability_attributes);
    drop(capability_sids);
    if ok == 0 {
        return Err(last_error("CreateProcessW(AppContainer child)"));
    }

    let process_handle = HandleGuard::new(process.hProcess);
    let thread_handle = HandleGuard::new(process.hThread);
    let job = match create_job_handle(memory_limit_bytes) {
        Ok(job) => job,
        Err(err) => {
            unsafe {
                TerminateProcess(process_handle.handle, 1);
            }
            return Err(err);
        }
    };
    let ok = unsafe { AssignProcessToJobObject(job.handle, process_handle.handle) };
    if ok == 0 {
        unsafe {
            TerminateProcess(process_handle.handle, 1);
        }
        return Err(last_error("AssignProcessToJobObject"));
    }
    let resumed = unsafe { ResumeThread(thread_handle.handle) };
    if resumed == u32::MAX {
        unsafe {
            TerminateJobObject(job.handle, 1);
        }
        return Err(last_error("ResumeThread"));
    }

    Ok(WindowsSandboxChild {
        stdin: Some(stdin_pipe.parent),
        stdout: Some(stdout_pipe.parent),
        stderr,
        process: process_handle,
        job,
        _profile: profile,
        _desktop: desktop,
        id: process.dwProcessId,
        exit_code: None,
    })
}

struct PrivateDesktop {
    _station: WindowStationHandle,
    _desktop: DesktopHandle,
    full_name: Vec<u16>,
}

impl PrivateDesktop {
    fn create(appcontainer_sid: &SidGuard) -> Result<Self> {
        let current_user = current_user_sid()?;
        let current_user = current_user.as_ptr() as PSID;
        let appcontainer_sid = appcontainer_sid.as_ptr();

        let suffix = unique_profile_name(Path::new("mayhem-private-desktop"));
        let station_name = format!("mayhem-winsta-{}-{suffix}", unsafe {
            GetCurrentProcessId()
        });
        let desktop_name = "worker";
        let station_name_w = to_wide_null(OsStr::new(&station_name));
        let desktop_name_w = to_wide_null(OsStr::new(desktop_name));

        let mut station_security = WindowObjectSecurity::new(&[
            (current_user, GENERIC_ALL),
            (appcontainer_sid, WINDOW_STATION_CHILD_ACCESS),
        ])?;
        let station_attributes = station_security.attributes();
        let station = unsafe {
            CreateWindowStationW(station_name_w.as_ptr(), 0, GENERIC_ALL, &station_attributes)
        };
        if station.is_null() {
            return Err(last_error(
                "CreateWindowStationW(private AppContainer station)",
            ));
        }
        let station = WindowStationHandle(station);

        let mut desktop_security = WindowObjectSecurity::new(&[
            (current_user, GENERIC_ALL),
            (appcontainer_sid, DESKTOP_CHILD_ACCESS),
        ])?;
        let desktop_attributes = desktop_security.attributes();
        let desktop = {
            let _switch_lock = WINDOW_STATION_SWITCH_LOCK.lock().map_err(|_| {
                WindowsSandboxError::Windows(
                    "private window-station switch lock was poisoned".to_owned(),
                )
            })?;
            let switch = ProcessWindowStationSwitch::activate(station.0)?;
            let desktop = unsafe {
                CreateDesktopW(
                    desktop_name_w.as_ptr(),
                    null(),
                    null(),
                    0,
                    GENERIC_ALL,
                    &desktop_attributes,
                )
            };
            let create_error = desktop
                .is_null()
                .then(|| last_error("CreateDesktopW(private AppContainer desktop)"));
            let restore_result = switch.restore();
            if let Err(error) = restore_result {
                if !desktop.is_null() {
                    unsafe {
                        CloseDesktop(desktop);
                    }
                }
                return Err(error);
            }
            if let Some(error) = create_error {
                return Err(error);
            }
            DesktopHandle(desktop)
        };

        let full_name = format!("{station_name}\\{desktop_name}");
        Ok(Self {
            _station: station,
            _desktop: desktop,
            full_name: to_wide_null(OsStr::new(&full_name)),
        })
    }
}

struct WindowObjectSecurity {
    descriptor: SECURITY_DESCRIPTOR,
    dacl: *mut ACL,
}

impl WindowObjectSecurity {
    fn new(entries: &[(PSID, u32)]) -> Result<Self> {
        let mut accesses = entries
            .iter()
            .map(|(sid, permissions)| {
                let mut access = EXPLICIT_ACCESS_W {
                    grfAccessPermissions: *permissions,
                    grfAccessMode: GRANT_ACCESS,
                    grfInheritance: NO_INHERITANCE,
                    ..Default::default()
                };
                access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
                access.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
                access.Trustee.ptstrName = *sid as PWSTR;
                access
            })
            .collect::<Vec<_>>();
        let mut dacl: *mut ACL = null_mut();
        let error = unsafe {
            SetEntriesInAclW(
                accesses.len() as u32,
                accesses.as_mut_ptr(),
                null_mut(),
                &mut dacl,
            )
        };
        if error != 0 {
            return Err(win32_error("SetEntriesInAclW(private desktop)", error));
        }
        if dacl.is_null() {
            return Err(WindowsSandboxError::Windows(
                "SetEntriesInAclW(private desktop) returned a null DACL".to_owned(),
            ));
        }

        let mut descriptor: SECURITY_DESCRIPTOR = unsafe { zeroed() };
        let ok = unsafe {
            InitializeSecurityDescriptor(
                &mut descriptor as *mut SECURITY_DESCRIPTOR as PSECURITY_DESCRIPTOR,
                SECURITY_DESCRIPTOR_REVISION,
            )
        };
        if ok == 0 {
            unsafe {
                LocalFree(dacl as HLOCAL);
            }
            return Err(last_error("InitializeSecurityDescriptor(private desktop)"));
        }
        let ok = unsafe {
            SetSecurityDescriptorDacl(
                &mut descriptor as *mut SECURITY_DESCRIPTOR as PSECURITY_DESCRIPTOR,
                1,
                dacl,
                0,
            )
        };
        if ok == 0 {
            unsafe {
                LocalFree(dacl as HLOCAL);
            }
            return Err(last_error("SetSecurityDescriptorDacl(private desktop)"));
        }
        Ok(Self { descriptor, dacl })
    }

    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &mut self.descriptor as *mut SECURITY_DESCRIPTOR as *mut _,
            bInheritHandle: 0,
        }
    }
}

impl Drop for WindowObjectSecurity {
    fn drop(&mut self) {
        if !self.dacl.is_null() {
            unsafe {
                LocalFree(self.dacl as HLOCAL);
            }
        }
    }
}

struct ProcessWindowStationSwitch {
    previous: HWINSTA,
    active: bool,
}

impl ProcessWindowStationSwitch {
    fn activate(target: HWINSTA) -> Result<Self> {
        let previous = unsafe { GetProcessWindowStation() };
        if previous.is_null() {
            return Err(last_error("GetProcessWindowStation"));
        }
        if unsafe { SetProcessWindowStation(target) } == 0 {
            return Err(last_error("SetProcessWindowStation(private)"));
        }
        Ok(Self {
            previous,
            active: true,
        })
    }

    fn restore(mut self) -> Result<()> {
        if unsafe { SetProcessWindowStation(self.previous) } == 0 {
            return Err(last_error("SetProcessWindowStation(restore)"));
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for ProcessWindowStationSwitch {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                SetProcessWindowStation(self.previous);
            }
        }
    }
}

struct WindowStationHandle(HWINSTA);

impl Drop for WindowStationHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseWindowStation(self.0);
            }
        }
    }
}

struct DesktopHandle(HDESK);

impl Drop for DesktopHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseDesktop(self.0);
            }
        }
    }
}

fn current_user_sid() -> Result<Vec<u8>> {
    let mut token: HANDLE = null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(last_error("OpenProcessToken(current user)"));
    }
    let token = HandleGuard::new(token);
    let mut required = 0u32;
    unsafe {
        GetTokenInformation(token.handle, TokenUser, null_mut(), 0, &mut required);
    }
    if required == 0 {
        return Err(last_error("GetTokenInformation(TokenUser size)"));
    }
    let mut info = vec![0u8; required as usize];
    let ok = unsafe {
        GetTokenInformation(
            token.handle,
            TokenUser,
            info.as_mut_ptr() as *mut _,
            required,
            &mut required,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation(TokenUser)"));
    }
    let user = unsafe { &*(info.as_ptr() as *const TOKEN_USER) };
    let length = unsafe { GetLengthSid(user.User.Sid) };
    if length == 0 {
        return Err(last_error("GetLengthSid(current user)"));
    }
    let mut sid = vec![0u8; length as usize];
    let ok = unsafe { CopySid(length, sid.as_mut_ptr() as PSID, user.User.Sid) };
    if ok == 0 {
        return Err(last_error("CopySid(current user)"));
    }
    Ok(sid)
}

fn command_capability_sids(command: &WindowsSandboxCommand) -> Result<Vec<Vec<u8>>> {
    if command.allow_code_generation {
        Ok(vec![derive_capability_sid("codeGeneration")?])
    } else {
        Ok(Vec::new())
    }
}

fn capability_attributes(sids: &mut [Vec<u8>]) -> Vec<SID_AND_ATTRIBUTES> {
    sids.iter_mut()
        .map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid.as_mut_ptr() as PSID,
            Attributes: SE_GROUP_ENABLED as u32,
        })
        .collect()
}

fn derive_capability_sid(name: &str) -> Result<Vec<u8>> {
    let name = to_wide_null(name);
    let mut group_sids: *mut PSID = null_mut();
    let mut group_count = 0_u32;
    let mut capability_sids: *mut PSID = null_mut();
    let mut capability_count = 0_u32;
    let ok = unsafe {
        DeriveCapabilitySidsFromName(
            name.as_ptr(),
            &mut group_sids,
            &mut group_count,
            &mut capability_sids,
            &mut capability_count,
        )
    };
    if ok == 0 {
        free_derived_sids(group_sids, group_count);
        free_derived_sids(capability_sids, capability_count);
        return Err(last_error("DeriveCapabilitySidsFromName"));
    }

    let result = if capability_sids.is_null() || capability_count != 1 {
        Err(WindowsSandboxError::InvalidConfig(format!(
            "capability {name:?} resolved to {capability_count} SIDs instead of one"
        )))
    } else {
        let source = unsafe { *capability_sids };
        let length = unsafe { GetLengthSid(source) };
        if length == 0 {
            Err(last_error("GetLengthSid(capability)"))
        } else {
            let mut owned = vec![0_u8; length as usize];
            let copied = unsafe { CopySid(length, owned.as_mut_ptr() as PSID, source) };
            if copied == 0 {
                Err(last_error("CopySid(capability)"))
            } else {
                Ok(owned)
            }
        }
    };
    free_derived_sids(group_sids, group_count);
    free_derived_sids(capability_sids, capability_count);
    result
}

fn free_derived_sids(sids: *mut PSID, count: u32) {
    if sids.is_null() {
        return;
    }
    unsafe {
        for index in 0..count as usize {
            let sid = *sids.add(index);
            if !sid.is_null() {
                LocalFree(sid as HLOCAL);
            }
        }
        LocalFree(sids as HLOCAL);
    }
}

fn launch(
    appcontainer_sid: PSID,
    desktop: &mut PrivateDesktop,
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
    startup.StartupInfo.lpDesktop = desktop.full_name.as_mut_ptr();
    startup.lpAttributeList = attr.as_mut_ptr();

    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut command_line = to_wide_null(windows_command_line(command));
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
    let job = if memory_limit_bytes.is_some_and(|bytes| bytes > 0) {
        match create_job_handle(memory_limit_bytes) {
            Ok(job) => Some(job),
            Err(err) => {
                unsafe {
                    TerminateProcess(process_handle.handle, 1);
                }
                return Err(err);
            }
        }
    } else {
        None
    };
    if let Some(job) = &job {
        let ok = unsafe { AssignProcessToJobObject(job.handle, process_handle.handle) };
        if ok == 0 {
            unsafe {
                TerminateProcess(process_handle.handle, 1);
            }
            return Err(last_error("AssignProcessToJobObject"));
        }
    }
    let resumed = unsafe { ResumeThread(thread_handle.handle) };
    if resumed == u32::MAX {
        if let Some(job) = &job {
            unsafe {
                TerminateJobObject(job.handle, 1);
            }
        } else {
            unsafe {
                TerminateProcess(process_handle.handle, 1);
            }
        }
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

fn create_job_handle(memory_limit_bytes: Option<u64>) -> Result<HandleGuard> {
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    let job = HandleGuard::new(handle);
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if let Some(limit_bytes) = memory_limit_bytes.filter(|bytes| *bytes > 0) {
        let limit = usize::try_from(limit_bytes).map_err(|_| {
            WindowsSandboxError::Windows(format!(
                "memory limit {limit_bytes} exceeds Windows process address size"
            ))
        })?;
        info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = limit;
    }
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
    Ok(job)
}

#[derive(Clone, Copy)]
enum AppContainerFileAccess {
    Traverse,
    ReadOnly,
    Writable,
    Executable,
}

fn grant_appcontainer_access(
    appcontainer_sid: &SidGuard,
    path: &Path,
    inherit: bool,
    file_access: AppContainerFileAccess,
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

    let permissions = match file_access {
        // Python resolves directory roots through handles, which also requires
        // read-attributes and synchronization rights on each parent.
        AppContainerFileAccess::Traverse => FILE_GENERIC_EXECUTE,
        AppContainerFileAccess::ReadOnly => FILE_GENERIC_READ,
        // Traverse must inherit too: workers commonly create a cache directory
        // and then create and remove request-scoped children beneath it.
        AppContainerFileAccess::Writable => {
            FILE_GENERIC_READ
                | FILE_GENERIC_WRITE
                | FILE_GENERIC_EXECUTE
                | FILE_DELETE_CHILD
                | DELETE
        }
        AppContainerFileAccess::Executable => FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
    };
    let mut access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: permissions,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: if inherit {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT | OBJECT_INHERIT_ACE
        } else {
            NO_INHERITANCE
        },
        ..Default::default()
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
    deleted: bool,
}

impl AppContainerProfile {
    fn create(name: &str, capabilities: &mut [SID_AND_ATTRIBUTES]) -> Result<Self> {
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
                if capabilities.is_empty() {
                    null()
                } else {
                    capabilities.as_ptr()
                },
                capabilities.len() as u32,
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
            deleted: false,
        })
    }

    fn delete(&mut self) -> Result<()> {
        let hr = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
        if hr_failed(hr) {
            return Err(hresult_error("DeleteAppContainerProfile", hr));
        }
        self.deleted = true;
        Ok(())
    }
}

impl Drop for AppContainerProfile {
    fn drop(&mut self) {
        if !self.deleted {
            let _ = self.delete();
        }
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

    fn into_file(mut self) -> File {
        let handle = self.handle;
        self.handle = null_mut();
        unsafe { File::from_raw_handle(handle as RawHandle) }
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

enum PipeDirection {
    ChildReads,
    ChildWrites,
}

struct ChildPipe {
    parent: File,
    child: HandleGuard,
}

fn child_pipe(direction: PipeDirection) -> Result<ChildPipe> {
    let mut read = null_mut();
    let mut write = null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let ok = unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) };
    if ok == 0 {
        return Err(last_error("CreatePipe"));
    }
    let read = HandleGuard::new(read);
    let write = HandleGuard::new(write);
    let (parent, child) = match direction {
        PipeDirection::ChildReads => (write, read),
        PipeDirection::ChildWrites => (read, write),
    };
    let ok = unsafe { SetHandleInformation(parent.handle, HANDLE_FLAG_INHERIT, 0) };
    if ok == 0 {
        return Err(last_error("SetHandleInformation(pipe parent)"));
    }
    Ok(ChildPipe {
        parent: parent.into_file(),
        child,
    })
}

fn inheritable_stderr_handle() -> Result<HandleGuard> {
    let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if stderr.is_null() || stderr == INVALID_HANDLE_VALUE {
        return inheritable_null_handle();
    }
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate = null_mut();
    let ok = unsafe {
        DuplicateHandle(
            process,
            stderr,
            process,
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(last_error("DuplicateHandle(stderr)"));
    }
    Ok(HandleGuard::new(duplicate))
}

fn inheritable_null_handle() -> Result<HandleGuard> {
    let path = to_wide_null("NUL");
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW(NUL)"));
    }
    Ok(HandleGuard::new(handle))
}

fn process_exit_code(process: HANDLE) -> Result<u32> {
    let mut exit_code = 1_u32;
    let ok = unsafe { GetExitCodeProcess(process, &mut exit_code) };
    if ok == 0 {
        return Err(last_error("GetExitCodeProcess"));
    }
    Ok(exit_code)
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

    fn update_handle_list(&mut self, handles: &[HANDLE]) -> Result<()> {
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.ptr,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr() as *const _,
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        };
        if ok == 0 {
            return Err(last_error("UpdateProcThreadAttribute(handle list)"));
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

fn command_executable_path_os(program: &OsStr, current_dir: Option<&Path>) -> Option<PathBuf> {
    let path = PathBuf::from(program);
    let path = if path.is_absolute() {
        path
    } else {
        current_dir
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())?
            .join(path)
    };
    path.exists()
        .then(|| std::fs::canonicalize(&path).unwrap_or(path))
}

fn command_executable_path(program: &str) -> Option<PathBuf> {
    command_executable_path_os(OsStr::new(program), None)
}

fn windows_command_line_os(program: &OsStr, args: &[OsString]) -> Vec<u16> {
    let mut command_line = Vec::new();
    append_quoted_windows_arg(&mut command_line, program);
    for arg in args {
        command_line.push(b' ' as u16);
        append_quoted_windows_arg(&mut command_line, arg);
    }
    command_line.push(0);
    command_line
}

fn append_quoted_windows_arg(output: &mut Vec<u16>, arg: &OsStr) {
    let units = arg.encode_wide().collect::<Vec<_>>();
    let needs_quotes = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16 || *unit == b'"' as u16);
    if !needs_quotes {
        output.extend_from_slice(&units);
        return;
    }

    output.push(b'"' as u16);
    let mut backslashes = 0_usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            output.push(unit);
            backslashes = 0;
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            backslashes = 0;
            output.push(unit);
        }
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
}

fn windows_environment_block(command: &WindowsSandboxCommand) -> Vec<u16> {
    let mut environment = BTreeMap::<String, (OsString, OsString)>::new();
    if !command.env_clear {
        for (key, value) in std::env::vars_os() {
            environment.insert(windows_env_sort_key(&key), (key, value));
        }
    }
    for (key, value) in &command.env {
        let sort_key = windows_env_sort_key(key);
        match value {
            Some(value) => {
                environment.insert(sort_key, (key.clone(), value.clone()));
            }
            None => {
                environment.remove(&sort_key);
            }
        }
    }

    if environment.is_empty() {
        return vec![0, 0];
    }
    let mut block = Vec::new();
    for (_, (key, value)) in environment {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn windows_env_sort_key(key: &OsStr) -> String {
    key.to_string_lossy().to_uppercase()
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
            out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            out.push('"');
            backslashes = 0;
        } else {
            out.extend(std::iter::repeat_n('\\', backslashes));
            backslashes = 0;
            out.push(ch);
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

fn unique_profile_name(policy_root: &Path) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    let tail = policy_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root")
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
    if code == 0 {
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
