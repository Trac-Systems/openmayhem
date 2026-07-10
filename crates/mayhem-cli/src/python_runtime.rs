use std::env;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};

const GIB: u64 = 1024 * 1024 * 1024;

const VLLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/vllm.txt");
const TRT_LLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/trt-llm.txt");
const MLX_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/mlx.txt");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PythonRuntime {
    pub(crate) python: PathBuf,
    pub(crate) source: String,
    pub(crate) requirements_sha256: String,
}

#[derive(Clone, Copy, Debug)]
struct PythonRuntimeSpec {
    backend: &'static str,
    override_env: &'static str,
    distribution: &'static str,
    import_name: &'static str,
    version: &'static str,
    requirements: &'static [u8],
    requirements_sha256: &'static str,
    min_free_bytes: u64,
}

pub(crate) fn ensure_backend_python(home: &Path, backend: &str) -> Result<PythonRuntime> {
    let spec = python_runtime_spec(backend)
        .with_context(|| format!("backend {backend} does not use a managed Python runtime"))?;
    verify_requirements(&spec)?;

    if let Some(explicit) = env::var_os(spec.override_env) {
        let python = PathBuf::from(explicit);
        validate_python(&python, &spec).with_context(|| {
            format!(
                "{} points to an unusable {} runtime; fix or unset the explicit override",
                spec.override_env, spec.backend
            )
        })?;
        return Ok(PythonRuntime {
            python,
            source: format!("explicit {}", spec.override_env),
            requirements_sha256: spec.requirements_sha256.to_owned(),
        });
    }

    let venvs = home.join("venvs");
    fs::create_dir_all(&venvs)
        .with_context(|| format!("creating managed Python directory {}", venvs.display()))?;
    let venv = venvs.join(spec.backend);
    let python = venv_python(&venv);
    if validate_python(&python, &spec).is_ok() {
        return Ok(PythonRuntime {
            python,
            source: "managed existing venv".to_owned(),
            requirements_sha256: spec.requirements_sha256.to_owned(),
        });
    }

    let lock_path = venvs.join(format!(".{}.bootstrap.lock", spec.backend));
    let lock = open_lock_file(&lock_path)?;
    lock.lock_exclusive().with_context(|| {
        format!(
            "locking managed {} bootstrap {}",
            spec.backend,
            lock_path.display()
        )
    })?;

    let result = (|| {
        if validate_python(&python, &spec).is_ok() {
            return Ok(PythonRuntime {
                python,
                source: "managed existing venv".to_owned(),
                requirements_sha256: spec.requirements_sha256.to_owned(),
            });
        }

        let free_bytes = fs2::available_space(&venvs).with_context(|| {
            format!(
                "reading free space before {} Python bootstrap",
                spec.backend
            )
        })?;
        if free_bytes < spec.min_free_bytes {
            bail!(
                "{} Python bootstrap needs at least {} GiB free under {}; only {} GiB is available",
                spec.backend,
                spec.min_free_bytes / GIB,
                venvs.display(),
                free_bytes / GIB
            );
        }

        if venv.exists() {
            fs::remove_dir_all(&venv)
                .with_context(|| format!("removing incomplete managed venv {}", venv.display()))?;
        }

        let base_python = resolve_base_python()?;
        let create = Command::new(&base_python)
            .arg("-m")
            .arg("venv")
            .arg(&venv)
            .output()
            .with_context(|| format!("starting {} -m venv", base_python.display()))?;
        if !create.status.success() {
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "creating the managed {} venv failed with {}; install the OS Python venv package and retry",
                spec.backend,
                create.status
            );
        }

        let requirements_path = venv.join("mayhem-requirements.txt");
        fs::write(&requirements_path, spec.requirements).with_context(|| {
            format!(
                "writing checked {} requirements {}",
                spec.backend,
                requirements_path.display()
            )
        })?;
        let managed_python = venv_python(&venv);
        let install = Command::new(&managed_python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--disable-pip-version-check")
            .arg("--no-input")
            .arg("--requirement")
            .arg(&requirements_path)
            .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
            .env("PIP_NO_INPUT", "1")
            .output()
            .with_context(|| {
                format!(
                    "starting pip for managed {} runtime {}",
                    spec.backend,
                    managed_python.display()
                )
            })?;
        if !install.status.success() {
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "installing pinned {}=={} for {} failed with {}; check network access and the backend OS prerequisites, then retry",
                spec.distribution,
                spec.version,
                spec.backend,
                install.status
            );
        }
        validate_python(&managed_python, &spec).with_context(|| {
            format!(
                "managed {} install completed but its import/version check failed",
                spec.backend
            )
        })?;
        Ok(PythonRuntime {
            python: managed_python,
            source: "managed bootstrapped venv".to_owned(),
            requirements_sha256: spec.requirements_sha256.to_owned(),
        })
    })();

    let _ = FileExt::unlock(&lock);
    result
}

fn python_runtime_spec(backend: &str) -> Option<PythonRuntimeSpec> {
    match backend {
        "vllm" => Some(PythonRuntimeSpec {
            backend: "vllm",
            override_env: "MAYHEM_VLLM_PYTHON",
            distribution: "vllm",
            import_name: "vllm",
            version: "0.24.0",
            requirements: VLLM_REQUIREMENTS,
            requirements_sha256: "13bac7dc6e708d6176792e5e93c9067ebf4f772960cba7a626b71d4aae4a1d2c",
            min_free_bytes: 8 * GIB,
        }),
        "trt-llm" => Some(PythonRuntimeSpec {
            backend: "trt-llm",
            override_env: "MAYHEM_TRTLLM_PYTHON",
            distribution: "tensorrt_llm",
            import_name: "tensorrt_llm",
            version: "1.2.1",
            requirements: TRT_LLM_REQUIREMENTS,
            requirements_sha256: "50ca3c97d922f6687224aeb44a2b5e4530d450c2e9e1df793f9a111e12df6703",
            min_free_bytes: 12 * GIB,
        }),
        "mlx" => Some(PythonRuntimeSpec {
            backend: "mlx",
            override_env: "MAYHEM_MLX_PYTHON",
            distribution: "mlx-lm",
            import_name: "mlx_lm",
            version: "0.31.3",
            requirements: MLX_REQUIREMENTS,
            requirements_sha256: "5dc4a038a260c2db6e72a1025842f2d8d229bd4e87f95f2f757ac61ec49aaa40",
            min_free_bytes: 2 * GIB,
        }),
        _ => None,
    }
}

fn verify_requirements(spec: &PythonRuntimeSpec) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(spec.requirements));
    if actual != spec.requirements_sha256 {
        bail!(
            "embedded {} requirements checksum mismatch: expected {}, got {}",
            spec.backend,
            spec.requirements_sha256,
            actual
        );
    }
    let text = std::str::from_utf8(spec.requirements)
        .with_context(|| format!("{} requirements are not UTF-8", spec.backend))?;
    let expected = format!("{}=={}", spec.distribution, spec.version);
    if text.lines().filter(|line| !line.trim().is_empty()).count() != 1 || text.trim() != expected {
        bail!(
            "embedded {} requirements must contain exactly {}",
            spec.backend,
            expected
        );
    }
    Ok(())
}

fn validate_python(python: &Path, spec: &PythonRuntimeSpec) -> Result<()> {
    let script = format!(
        "import importlib.metadata as m; import {}; print(m.version({:?}))",
        spec.import_name, spec.distribution
    );
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .output()
        .with_context(|| format!("starting {}", python.display()))?;
    if !output.status.success() {
        bail!(
            "{} could not import {}=={}",
            python.display(),
            spec.distribution,
            spec.version
        );
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if actual != spec.version {
        bail!(
            "{} has {} {}, expected {}",
            python.display(),
            spec.distribution,
            actual,
            spec.version
        );
    }
    Ok(())
}

fn resolve_base_python() -> Result<PathBuf> {
    for candidate in ["python3", "python"] {
        let output = Command::new(candidate).arg("--version").output();
        if output.is_ok_and(|output| output.status.success()) {
            return Ok(PathBuf::from(candidate));
        }
    }
    bail!("Python 3 is required to bootstrap the selected backend; install python3 and retry")
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts/python.exe")
    } else {
        venv.join("bin/python")
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_requirements_are_exact_and_checksummed() {
        for backend in ["vllm", "trt-llm", "mlx"] {
            let spec = python_runtime_spec(backend).expect("known backend");
            verify_requirements(&spec).expect("requirements verify");
            assert!(std::str::from_utf8(spec.requirements)
                .unwrap()
                .trim()
                .contains("=="));
        }
    }

    #[test]
    fn backend_override_names_stay_backend_specific() {
        assert_eq!(
            python_runtime_spec("vllm").map(|spec| spec.override_env),
            Some("MAYHEM_VLLM_PYTHON")
        );
        assert_eq!(
            python_runtime_spec("trt-llm").map(|spec| spec.override_env),
            Some("MAYHEM_TRTLLM_PYTHON")
        );
        assert_eq!(
            python_runtime_spec("mlx").map(|spec| spec.override_env),
            Some("MAYHEM_MLX_PYTHON")
        );
        assert!(python_runtime_spec("llama.cpp").is_none());
    }

    #[test]
    fn managed_python_path_is_platform_native() {
        let root = Path::new("/tmp/mayhem-test-venv");
        let python = venv_python(root);
        if cfg!(windows) {
            assert!(python.ends_with("Scripts/python.exe"));
        } else {
            assert!(python.ends_with("bin/python"));
        }
    }
}
