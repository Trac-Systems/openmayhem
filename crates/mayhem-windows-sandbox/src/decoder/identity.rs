//! Minimal OS identity registration, not a writable AppContainer profile.
use super::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::private_files::mutation::startup::{PREFIX, RECORD_BYTES, StartupJournal, valid_name};

type Register = unsafe extern "system" fn(PSID, *const u16, *const u16) -> i32;
type Unregister = unsafe extern "system" fn(PSID) -> i32;
type Lookup = unsafe extern "system" fn(PSID, *mut *mut u16) -> i32;
type Free = unsafe extern "system" fn(*mut u16);

struct Api {
    register: Register,
    unregister: Unregister,
    lookup: Lookup,
    free: Free,
}
impl Api {
    fn load() -> Result<Self> {
        // Already loaded by Windows: no caller PATH or provider DLL lookup.
        let module = unsafe { GetModuleHandleW(to_wide_null("kernelbase.dll").as_ptr()) };
        if module.is_null() {
            return Err(invalid());
        }
        macro_rules! export {
            ($name:literal, $ty:ty) => {{
                let function = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) }
                    .ok_or_else(invalid)?;
                unsafe { std::mem::transmute::<_, $ty>(function) }
            }};
        }
        Ok(Self {
            register: export!("AppContainerRegisterSid", Register),
            unregister: export!("AppContainerUnregisterSid", Unregister),
            lookup: export!("AppContainerLookupMoniker", Lookup),
            free: export!("AppContainerFreeMemory", Free),
        })
    }
    fn present(&self, sid: &SidGuard, name: &[u16]) -> Result<bool> {
        let mut value = null_mut();
        let result = unsafe { (self.lookup)(sid.as_ptr(), &mut value) };
        // These are absence, not permission/IO failures. Do not overwrite the
        // recovery record when we cannot determine registration ownership.
        let absent = [0x80070002u32, 0x80070490].contains(&(result as u32));
        let mut matches = !value.is_null() && result >= 0;
        if matches {
            for (i, expected) in name.iter().enumerate() {
                let actual = unsafe { *value.add(i) };
                if actual != *expected {
                    matches = false;
                    break;
                }
                if actual == 0 {
                    break;
                }
            }
        }
        if !value.is_null() {
            unsafe { (self.free)(value) };
        }
        if absent {
            return Ok(false);
        }
        if result < 0 {
            return Err(hresult_error("decoder identity lookup", result));
        }
        if !matches {
            return Err(invalid());
        }
        Ok(true)
    }
    fn remove(&self, sid: &SidGuard) -> Result<()> {
        let result = unsafe { (self.unregister)(sid.as_ptr()) };
        if result < 0 {
            return Err(hresult_error("decoder identity cleanup", result));
        }
        Ok(())
    }
}

pub(super) fn name() -> Result<Vec<u16>> {
    Ok(to_wide_null(format!(
        "{PREFIX}{}",
        &nonce()?[..RECORD_BYTES - PREFIX.len()]
    )))
}
fn derive(name: &[u16]) -> Result<SidGuard> {
    let mut sid = null_mut();
    let result = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if result < 0 || sid.is_null() {
        return Err(invalid());
    }
    Ok(SidGuard::new(sid))
}

pub(super) struct Registration<'a> {
    sid: &'a SidGuard,
    api: Api,
    registered: bool,
    journal: Option<StartupJournal>,
}
impl<'a> Registration<'a> {
    pub fn new(sid: &'a SidGuard, name: &[u16]) -> Result<Self> {
        let parent = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(invalid)?);
        Self::at(sid, name, &parent)
    }
    fn at(sid: &'a SidGuard, name: &[u16], parent: &Path) -> Result<Self> {
        if name.len() != RECORD_BYTES + 1 || name.last() != Some(&0) {
            return Err(invalid());
        }
        let text = String::from_utf16(&name[..RECORD_BYTES]).map_err(|_| invalid())?;
        if !valid_name(&text) {
            return Err(invalid());
        }
        let derived = derive(name)?;
        if unsafe { windows_sys::Win32::Security::EqualSid(derived.as_ptr(), sid.as_ptr()) } == 0 {
            return Err(invalid());
        }
        let api = Api::load()?;
        let mut journal = StartupJournal::open(parent).map_err(|_| invalid())?;
        if let Some(previous) = journal.previous().map_err(|_| invalid())? {
            let previous = to_wide_null(previous);
            let old_sid = derive(&previous)?;
            if api.present(&old_sid, &previous)? {
                api.remove(&old_sid)?;
            }
        }
        // No overwrite until the prior registration is demonstrably absent.
        journal.record(&text).map_err(|_| invalid())?;
        let mut registration = Self {
            sid,
            api,
            registered: false,
            journal: Some(journal),
        };
        let result =
            unsafe { (registration.api.register)(sid.as_ptr(), name.as_ptr(), name.as_ptr()) };
        if result < 0 {
            // The durable record remains even if this API partially succeeded.
            return Err(hresult_error("decoder identity registration", result));
        }
        registration.registered = true;
        Ok(registration)
    }
    pub fn remove(&mut self) -> Result<()> {
        if self.registered {
            self.api.remove(self.sid)?;
            self.registered = false;
        }
        self.journal.take(); // Release before inference; retain the fixed record.
        Ok(())
    }
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

pub(super) fn environment() -> Result<Vec<u16>> {
    // LOCALAPPDATA is a Windows AppContainer startup requirement, not a file
    // access grant. No parent credential, PATH, TEMP, or provider config passes.
    let mut block = Vec::new();
    for key in ["LOCALAPPDATA", "SystemRoot"] {
        let value = std::env::var_os(key).ok_or_else(invalid)?;
        let value = value.encode_wide().collect::<Vec<_>>();
        if value.is_empty() || value.len() > 32767 || value.contains(&0) {
            return Err(invalid());
        }
        block.extend(key.encode_utf16());
        block.push(b'=' as u16);
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_registration_has_no_profile_and_cleans_up_on_drop() {
        let name = name().unwrap();
        let text = String::from_utf16(&name[..64]).unwrap();
        let mut sid = null_mut();
        assert_eq!(
            unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) },
            0
        );
        let sid = SidGuard::new(sid);
        let profile = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
            .join("Packages")
            .join(&text);
        let module = unsafe { GetModuleHandleW(to_wide_null("kernelbase.dll").as_ptr()) };
        let lookup =
            unsafe { GetProcAddress(module, c"AppContainerLookupMoniker".as_ptr() as _) }.unwrap();
        let free =
            unsafe { GetProcAddress(module, c"AppContainerFreeMemory".as_ptr() as _) }.unwrap();
        let lookup: unsafe extern "system" fn(PSID, *mut *mut u16) -> i32 =
            unsafe { std::mem::transmute(lookup) };
        let free: unsafe extern "system" fn(*mut u16) = unsafe { std::mem::transmute(free) };
        let present = || {
            let mut value = null_mut();
            let result = unsafe { lookup(sid.as_ptr(), &mut value) };
            if !value.is_null() {
                unsafe {
                    free(value);
                }
            }
            result >= 0
        };
        assert!(!present());
        {
            let _registration = Registration::new(&sid, &name).unwrap();
            assert!(present());
            assert!(!profile.exists());
        }
        assert!(!present());
        assert!(!profile.exists());
        let mut registration = Registration::new(&sid, &name).unwrap();
        registration.remove().unwrap();
        registration.remove().unwrap();
        assert!(!present());
    }

    struct CrashChild(std::process::Child);
    impl Drop for CrashChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn crash_child(parent: &Path) -> (CrashChild, std::sync::mpsc::Receiver<String>) {
        use std::io::BufRead;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::decoder::identity::tests::registration_crash_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("MAYHEM_REGISTRATION_TEST_ROOT", parent)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(name) = line.strip_prefix("REGISTERED ") {
                    let _ = sender.send(name.to_owned());
                }
            }
        });
        (CrashChild(child), receiver)
    }

    #[test]
    fn parent_death_recovers_exact_registration_and_serializes_other_launchers() {
        use std::time::Duration;
        let fixture = super::super::tests::Fixture::new();
        let (mut first, first_ready) = crash_child(&fixture.0);
        let first_name = first_ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let first_name = to_wide_null(first_name);
        let first_sid = derive(&first_name).unwrap();
        let api = Api::load().unwrap();
        assert!(api.present(&first_sid, &first_name).unwrap());
        let (mut second, second_ready) = crash_child(&fixture.0);
        assert!(
            second_ready
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        assert!(second.0.try_wait().unwrap().is_none());
        first.0.kill().unwrap();
        first.0.wait().unwrap();
        let second_name = second_ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let second_name = to_wide_null(second_name);
        let second_sid = derive(&second_name).unwrap();
        assert!(!api.present(&first_sid, &first_name).unwrap());
        assert!(api.present(&second_sid, &second_name).unwrap());
        second.0.kill().unwrap();
        second.0.wait().unwrap();
        let third_name = name().unwrap();
        let third_sid = derive(&third_name).unwrap();
        let mut third = Registration::at(&third_sid, &third_name, &fixture.0).unwrap();
        assert!(!api.present(&second_sid, &second_name).unwrap());
        assert!(api.present(&third_sid, &third_name).unwrap());
        third.remove().unwrap();
        assert!(!api.present(&third_sid, &third_name).unwrap());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        assert_eq!(
            std::fs::metadata(fixture.0.join(".mayhem-proxy-startup-v1").join("identity"))
                .unwrap()
                .len(),
            64
        );
    }

    #[test]
    fn incomplete_first_record_recovers_but_invalid_complete_records_refuse() {
        use std::io::Write;
        let fixture = super::super::tests::Fixture::new();
        let path = fixture.0.join(".mayhem-proxy-startup-v1").join("identity");
        drop(StartupJournal::open(&fixture.0).unwrap());
        for len in [0, 7, 63] {
            std::fs::File::create(&path)
                .unwrap()
                .write_all(&vec![b'x'; len])
                .unwrap();
            let name = name().unwrap();
            let sid = derive(&name).unwrap();
            Registration::at(&sid, &name, &fixture.0)
                .unwrap()
                .remove()
                .unwrap();
        }
        for record in [vec![b'x'; 64], vec![b'x'; 65]] {
            std::fs::write(&path, record).unwrap();
            let name = name().unwrap();
            let sid = derive(&name).unwrap();
            assert!(Registration::at(&sid, &name, &fixture.0).is_err());
            assert!(!Api::load().unwrap().present(&sid, &name).unwrap());
        }
    }

    #[test]
    fn linked_recovery_record_is_refused_without_mutating_it() {
        let fixture = super::super::tests::Fixture::new();
        let name = name().unwrap();
        let sid = derive(&name).unwrap();
        Registration::at(&sid, &name, &fixture.0)
            .unwrap()
            .remove()
            .unwrap();
        let record = fixture.0.join(".mayhem-proxy-startup-v1").join("identity");
        let original = std::fs::read(&record).unwrap();
        std::fs::hard_link(&record, fixture.0.join("alias")).unwrap();
        assert!(Registration::at(&sid, &name, &fixture.0).is_err());
        assert_eq!(std::fs::read(&record).unwrap(), original);
        assert!(!Api::load().unwrap().present(&sid, &name).unwrap());
    }

    #[test]
    #[ignore = "bounded child invoked and killed by crash-recovery parent"]
    fn registration_crash_child() {
        use std::io::{Read, Write};
        let parent = PathBuf::from(std::env::var_os("MAYHEM_REGISTRATION_TEST_ROOT").unwrap());
        let name = name().unwrap();
        let sid = derive(&name).unwrap();
        let _registration = Registration::at(&sid, &name, &parent).unwrap();
        println!("\nREGISTERED {}", String::from_utf16(&name[..64]).unwrap());
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read(&mut [0]);
    }

    #[test]
    fn worker_environment_contains_only_required_os_paths() {
        let block = environment().unwrap();
        assert!(block.ends_with(&[0, 0]));
        let entries = block
            .split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf16(s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].starts_with("LOCALAPPDATA="));
        assert!(entries[1].starts_with("SystemRoot="));
    }
}
