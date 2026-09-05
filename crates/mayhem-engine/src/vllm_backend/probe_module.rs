use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: &str = include_str!("../mayhem_vllm_execution_probe.py");
const MODULE: &str = "mayhem_vllm_execution_probe.py";
pub(super) const IMPORT_PRELUDE: &str = "import sys\nsys.path.insert(0, sys.argv[1])\n";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct OwnedProbeModule {
    path: PathBuf,
}

impl OwnedProbeModule {
    pub(super) fn create() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..16 {
            let path = std::env::temp_dir().join(format!(
                "mayhem-vllm-probe-{}-{stamp}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
            let owned = Self { path };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(owned.path.join(MODULE))?
                .write_all(SOURCE.as_bytes())?;
            return Ok(owned);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate private vLLM probe directory",
        ))
    }

    pub(super) fn configure(&self, command: &mut Command) -> io::Result<()> {
        let mut paths = vec![self.path.clone()];
        if let Some(existing) = std::env::var_os("PYTHONPATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        let paths = std::env::join_paths(paths).map_err(io::Error::other)?;
        command
            .env("PYTHONPATH", paths)
            .env("PYTHONDONTWRITEBYTECODE", "1");
        Ok(())
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedProbeModule {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vllm_worker_execution_observation_regressions() {
        let output = Command::new("python3")
            .arg("-B")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/test_vllm_execution_probe.py"))
            .output()
            .expect("run CPU-only execution observation tests");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn probe_modules_are_private_disjoint_and_owner_cleaned() {
        let first = OwnedProbeModule::create().unwrap();
        let second = OwnedProbeModule::create().unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(fs::read_to_string(first.path.join(MODULE)).unwrap(), SOURCE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let mut command = Command::new("python3");
        first.configure(&mut command).unwrap();
        assert!(command.get_current_dir().is_none());
        let path = first.path.clone();
        drop(first);
        assert!(!path.exists());
        assert!(second.path.exists());
        let path = second.path.clone();
        drop(second);
        assert!(!path.exists());
    }

    #[test]
    fn probe_import_preserves_relative_paths_and_rejects_cwd_shadowing() {
        let probe = OwnedProbeModule::create().unwrap();
        let caller = OwnedProbeModule::create().unwrap();
        fs::write(
            caller.path.join(MODULE),
            "raise RuntimeError('untrusted CWD module')",
        )
        .unwrap();
        fs::write(caller.path.join("relative-model"), "model").unwrap();
        fs::create_dir(caller.path.join("relative-cache")).unwrap();
        let mut command = Command::new("python3");
        command.current_dir(&caller.path);
        probe.configure(&mut command).unwrap();
        assert_eq!(command.get_current_dir(), Some(caller.path.as_path()));
        let code = format!(
            "{IMPORT_PRELUDE}\
import mayhem_vllm_execution_probe as probe\n\
from pathlib import Path\n\
assert Path(probe.__file__).parent == Path(sys.argv[1])\n\
assert Path('relative-model').read_text() == 'model'\n\
assert Path('relative-cache').is_dir()\n"
        );
        let output = command
            .args(["-B", "-c", &code])
            .arg(probe.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut relative_python = Command::new("./relative-runtime/bin/python");
        relative_python.current_dir(&caller.path);
        probe.configure(&mut relative_python).unwrap();
        assert_eq!(
            relative_python.get_program(),
            "./relative-runtime/bin/python"
        );
        assert_eq!(
            relative_python.get_current_dir(),
            Some(caller.path.as_path())
        );
    }
}
