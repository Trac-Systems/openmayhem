use std::borrow::Cow;
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};

const GIB: u64 = 1024 * 1024 * 1024;

const VLLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/vllm.txt");
const TRT_LLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/trt-llm.txt");
const MLX_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/mlx.txt");
const LLAMA_MEDIA_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/llama-media.txt");
const TRANSFORMERS_ASR_REQUIREMENTS: &[u8] =
    include_bytes!("../resources/python/transformers-asr.txt");
const SULPHUR_REQUIREMENTS: &[u8] =
    include_bytes!("../resources/python/sulphur-runtime-requirements.txt");
const SULPHUR_RUNTIME_ADAPTER: &[u8] = include_bytes!("../resources/python/sulphur_runtime.py");
const SULPHUR_RUNTIME_ADAPTER_SHA256: &str =
    "8e3c0e8d0f1668e29b234c34fd1ddbbaaac4cb3992bac0c1cb6080df796d2fe6";
const SULPHUR_MLX_REQUIREMENTS: &[u8] =
    include_bytes!("../resources/python/sulphur-mlx-runtime-requirements.txt");
const SULPHUR_MLX_RUNTIME_ADAPTER: &[u8] =
    include_bytes!("../resources/python/sulphur_mlx_runtime.py");
const SULPHUR_MLX_RUNTIME_ADAPTER_SHA256: &str =
    "52e52709b4002eb8f978c0113778e148350834f2814b7b45b317d35353074abf";
// Reproducible pure-Python wheels from dgrauet/ltx-2-mlx@e1838a855bfd1640135c424c96cb27a0c0ad150e.
const SULPHUR_MLX_CORE_WHEEL: &[u8] =
    include_bytes!("../resources/python/ltx_core_mlx-0.14.19-py3-none-any.whl");
const SULPHUR_MLX_PIPELINES_WHEEL: &[u8] =
    include_bytes!("../resources/python/ltx_pipelines_mlx-0.14.19-py3-none-any.whl");
const SULPHUR_MLX_WHEELS: &[EmbeddedPythonWheel] = &[
    EmbeddedPythonWheel {
        filename: "ltx_core_mlx-0.14.19-py3-none-any.whl",
        distribution: "ltx-core-mlx",
        version: "0.14.19",
        source: SULPHUR_MLX_CORE_WHEEL,
        source_sha256: "9cf321efd63a3b268ac9d0c8654db2edf304e4ff3411905014517ee672f671ff",
    },
    EmbeddedPythonWheel {
        filename: "ltx_pipelines_mlx-0.14.19-py3-none-any.whl",
        distribution: "ltx-pipelines-mlx",
        version: "0.14.19",
        source: SULPHUR_MLX_PIPELINES_WHEEL,
        source_sha256: "8c3961053f43461efae12b80480343873bece9d78b745537b88036acfa4d6873",
    },
];
const MANAGED_UV_VERSION: &str = "0.11.29";
const MANAGED_PYTHON_VERSION: &str = "3.12";
const ACE_STEP_LOCK_SHA256: &str =
    "0a9c8067b3299bfc6881a06e097ff95e55e1b7bb8f9d1f84192ac23e59b995ab";
const ACE_STEP_SUPPLEMENTAL_REQUIREMENTS: &[u8] = b"av==18.0.0\n";
const ACE_STEP_SUPPLEMENTAL_REQUIREMENTS_SHA256: &str =
    "24cede85ce0cf7759803ac67fce16c071a42dd71625a87e7af7393ea52679c78";
const ACE_STEP_RUNTIME_SHA256: &str =
    "8302df0ede20984cfc67c0ed41657a289d5450dca2146afc0493390c2623f22b";
const ACE_STEP_MIN_FREE_BYTES: u64 = 24 * GIB;

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
    required_imports: &'static [&'static str],
    version: &'static str,
    requirements: &'static [u8],
    requirements_sha256: &'static str,
    extra_index_urls: &'static [&'static str],
    min_free_bytes: u64,
    embedded_module: Option<EmbeddedPythonModule>,
}

#[derive(Clone, Copy, Debug)]
struct EmbeddedPythonModule {
    name: &'static str,
    source: &'static [u8],
    source_sha256: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct EmbeddedPythonWheel {
    filename: &'static str,
    distribution: &'static str,
    version: &'static str,
    source: &'static [u8],
    source_sha256: &'static str,
}

pub(crate) fn ensure_backend_python(home: &Path, backend: &str) -> Result<PythonRuntime> {
    if backend == "ace-step" {
        return ensure_ace_step_python(home);
    }
    let spec = python_runtime_spec(backend)
        .with_context(|| format!("backend {backend} does not use a managed Python runtime"))?;
    verify_requirements(&spec)?;
    let cache_root = home.join("cache").join(spec.backend);

    if let Some(explicit) = env::var_os(spec.override_env) {
        let python = PathBuf::from(explicit);
        validate_python(&python, &spec, &cache_root).with_context(|| {
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
    if validate_python(&python, &spec, &cache_root).is_ok() {
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
        if validate_python(&python, &spec, &cache_root).is_ok() {
            return Ok(PythonRuntime {
                python,
                source: "managed existing venv".to_owned(),
                requirements_sha256: spec.requirements_sha256.to_owned(),
            });
        }

        if python.is_file() {
            let repair = install_embedded_python_wheels(&python, &venv, &spec)
                .and_then(|()| install_embedded_python_module(&python, &venv, &spec));
            if repair.is_ok() && validate_python(&python, &spec, &cache_root).is_ok() {
                return Ok(PythonRuntime {
                    python,
                    source: "managed repaired venv".to_owned(),
                    requirements_sha256: spec.requirements_sha256.to_owned(),
                });
            }
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

        let uv = ensure_managed_uv(home)?;
        let python_install_dir = home.join("python");
        let uv_cache = cache_root.join("uv");
        fs::create_dir_all(&python_install_dir).with_context(|| {
            format!(
                "creating managed Python install directory {}",
                python_install_dir.display()
            )
        })?;
        fs::create_dir_all(&uv_cache)
            .with_context(|| format!("creating uv cache directory {}", uv_cache.display()))?;
        let create = Command::new(&uv)
            .arg("venv")
            .arg("--python")
            .arg(MANAGED_PYTHON_VERSION)
            .arg("--seed")
            .arg(&venv)
            .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
            .env("UV_CACHE_DIR", &uv_cache)
            .env("UV_NO_PROGRESS", "1")
            .output()
            .with_context(|| {
                format!(
                    "starting {} venv for managed {} Python {}",
                    uv.display(),
                    spec.backend,
                    MANAGED_PYTHON_VERSION
                )
            })?;
        if !create.status.success() {
            let detail = command_output_detail(&create);
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "creating the managed {} Python {} venv failed with {}{}",
                spec.backend,
                MANAGED_PYTHON_VERSION,
                create.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }

        let requirements_path = venv.join("mayhem-requirements.txt");
        let requirements = canonical_requirements(spec.requirements, spec.backend)?;
        fs::write(&requirements_path, requirements.as_ref()).with_context(|| {
            format!(
                "writing checked {} requirements {}",
                spec.backend,
                requirements_path.display()
            )
        })?;
        let managed_python = venv_python(&venv);
        let mut install_command = Command::new(&managed_python);
        install_command
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--disable-pip-version-check")
            .arg("--no-input");
        for extra_index_url in spec.extra_index_urls {
            install_command
                .arg("--extra-index-url")
                .arg(extra_index_url);
        }
        let install = install_command
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
            let detail = command_output_detail(&install);
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "installing pinned {}=={} for {} failed with {}{}; check network access and the backend OS prerequisites, then retry",
                spec.distribution,
                spec.version,
                spec.backend,
                install.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        install_embedded_python_wheels(&managed_python, &venv, &spec)?;
        install_embedded_python_module(&managed_python, &venv, &spec)?;
        validate_python(&managed_python, &spec, &cache_root).with_context(|| {
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

fn ensure_ace_step_python(home: &Path) -> Result<PythonRuntime> {
    verify_ace_step_supplemental_requirements()?;
    let cache_root = home.join("cache").join("ace-step");
    let source_root = mayhem_engine::ensure_ace_step_source(&cache_root)
        .context("materializing the pinned ACE-Step runtime source")?;
    let lock_path = source_root.join("uv.lock");
    let lock_sha256 = file_sha256(&lock_path)?;
    if lock_sha256 != ACE_STEP_LOCK_SHA256 {
        bail!(
            "embedded ACE-Step uv.lock checksum mismatch: expected {}, got {}",
            ACE_STEP_LOCK_SHA256,
            lock_sha256
        );
    }

    let venvs = home.join("venvs");
    fs::create_dir_all(&venvs)
        .with_context(|| format!("creating managed Python directory {}", venvs.display()))?;
    let venv = venvs.join("ace-step");
    let python = venv_python(&venv);
    if validate_ace_step_python(&python, &source_root, &cache_root).is_ok() {
        return Ok(PythonRuntime {
            python,
            source: "managed existing venv".to_owned(),
            requirements_sha256: ACE_STEP_RUNTIME_SHA256.to_owned(),
        });
    }

    let bootstrap_lock_path = venvs.join(".ace-step.bootstrap.lock");
    let bootstrap_lock = open_lock_file(&bootstrap_lock_path)?;
    bootstrap_lock.lock_exclusive().with_context(|| {
        format!(
            "locking managed ACE-Step bootstrap {}",
            bootstrap_lock_path.display()
        )
    })?;

    let result = (|| {
        if validate_ace_step_python(&python, &source_root, &cache_root).is_ok() {
            return Ok(PythonRuntime {
                python,
                source: "managed existing venv".to_owned(),
                requirements_sha256: ACE_STEP_RUNTIME_SHA256.to_owned(),
            });
        }

        let free_bytes = fs2::available_space(&venvs).with_context(|| {
            format!(
                "reading free space before ACE-Step Python bootstrap under {}",
                venvs.display()
            )
        })?;
        if free_bytes < ACE_STEP_MIN_FREE_BYTES {
            bail!(
                "ACE-Step Python bootstrap needs at least {} GiB free under {}; only {} GiB is available",
                ACE_STEP_MIN_FREE_BYTES / GIB,
                venvs.display(),
                free_bytes / GIB
            );
        }
        if venv.exists() {
            fs::remove_dir_all(&venv)
                .with_context(|| format!("removing incomplete managed venv {}", venv.display()))?;
        }

        let uv = ensure_managed_uv(home)?;
        let install_project = temporary_ace_step_install_project(&source_root, &cache_root)?;
        let python_install_dir = home.join("python");
        let uv_cache = cache_root.join("uv");
        fs::create_dir_all(&python_install_dir).with_context(|| {
            format!(
                "creating managed Python install directory {}",
                python_install_dir.display()
            )
        })?;
        fs::create_dir_all(&uv_cache)
            .with_context(|| format!("creating uv cache directory {}", uv_cache.display()))?;
        let install = Command::new(&uv)
            .arg("sync")
            .arg("--frozen")
            .arg("--no-dev")
            .arg("--no-install-project")
            .arg("--python")
            .arg(MANAGED_PYTHON_VERSION)
            .arg("--project")
            .arg(install_project.path())
            .env("UV_PROJECT_ENVIRONMENT", &venv)
            .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
            .env("UV_CACHE_DIR", &uv_cache)
            .env("UV_NO_PROGRESS", "1")
            .output()
            .with_context(|| format!("starting {} sync for ACE-Step", uv.display()))?;
        if !install.status.success() {
            let detail = command_output_detail(&install);
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "installing the frozen ACE-Step runtime failed with {}{}",
                install.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        let supplemental_requirements_path = venv.join("mayhem-requirements.txt");
        fs::write(
            &supplemental_requirements_path,
            ACE_STEP_SUPPLEMENTAL_REQUIREMENTS,
        )
        .with_context(|| {
            format!(
                "writing checked ACE-Step supplemental requirements {}",
                supplemental_requirements_path.display()
            )
        })?;
        let supplemental_install = Command::new(&uv)
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&python)
            .arg("--no-deps")
            .arg("--requirement")
            .arg(&supplemental_requirements_path)
            .env("UV_CACHE_DIR", &uv_cache)
            .env("UV_NO_PROGRESS", "1")
            .output()
            .with_context(|| {
                format!(
                    "starting {} supplemental install for ACE-Step",
                    uv.display()
                )
            })?;
        if !supplemental_install.status.success() {
            let detail = command_output_detail(&supplemental_install);
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "installing the pinned ACE-Step supplemental runtime failed with {}{}",
                supplemental_install.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        validate_ace_step_python(&python, &source_root, &cache_root).with_context(|| {
            format!(
                "managed ACE-Step install completed but {} failed validation",
                python.display()
            )
        })?;
        Ok(PythonRuntime {
            python,
            source: "managed frozen uv runtime".to_owned(),
            requirements_sha256: ACE_STEP_RUNTIME_SHA256.to_owned(),
        })
    })();

    let _ = FileExt::unlock(&bootstrap_lock);
    result
}

struct TemporaryAceStepProject {
    path: PathBuf,
}

impl TemporaryAceStepProject {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryAceStepProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn temporary_ace_step_install_project(
    source_root: &Path,
    cache_root: &Path,
) -> Result<TemporaryAceStepProject> {
    let projects = cache_root.join("uv-projects");
    fs::create_dir_all(&projects)
        .with_context(|| format!("creating ACE-Step uv project root {}", projects.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = projects.join(format!(".ace-step.{}.{}.tmp", std::process::id(), nonce));
    if let Err(error) = copy_regular_directory_tree(source_root, &path) {
        let _ = fs::remove_dir_all(&path);
        return Err(error).with_context(|| {
            format!(
                "copying the verified ACE-Step source into disposable uv project {}",
                path.display()
            )
        });
    }
    Ok(TemporaryAceStepProject { path })
}

fn copy_regular_directory_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("reading source directory {}", source.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{} is not a real directory",
        source.display()
    );
    fs::create_dir(destination)
        .with_context(|| format!("creating directory {}", destination.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("reading source directory {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading an entry under {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)
            .with_context(|| format!("reading source path {}", source_path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "refusing symbolic link in verified ACE-Step source {}",
            source_path.display()
        );
        if metadata.is_dir() {
            copy_regular_directory_tree(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copying verified ACE-Step source {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            bail!(
                "refusing special file in verified ACE-Step source {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

fn ensure_managed_uv(home: &Path) -> Result<PathBuf> {
    let tools = home.join("tools");
    fs::create_dir_all(&tools)
        .with_context(|| format!("creating managed tools directory {}", tools.display()))?;
    let venv = tools.join(format!("uv-{MANAGED_UV_VERSION}"));
    let uv = venv_executable(&venv, if cfg!(windows) { "uv.exe" } else { "uv" });
    if validate_uv(&uv).is_ok() {
        return Ok(uv);
    }
    if venv.exists() {
        fs::remove_dir_all(&venv)
            .with_context(|| format!("removing incomplete uv environment {}", venv.display()))?;
    }
    let base_python = resolve_base_python()?;
    let create = Command::new(&base_python)
        .arg("-m")
        .arg("venv")
        .arg(&venv)
        .output()
        .with_context(|| format!("starting {} -m venv for uv", base_python.display()))?;
    if !create.status.success() {
        let _ = fs::remove_dir_all(&venv);
        bail!(
            "creating the managed uv environment failed with {}{}",
            create.status,
            {
                let detail = command_output_detail(&create);
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            }
        );
    }
    let install = Command::new(venv_python(&venv))
        .arg("-m")
        .arg("pip")
        .arg("install")
        .arg("--disable-pip-version-check")
        .arg("--no-input")
        .arg(format!("uv=={MANAGED_UV_VERSION}"))
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .env("PIP_NO_INPUT", "1")
        .output()
        .context("installing the pinned uv bootstrap")?;
    if !install.status.success() {
        let detail = command_output_detail(&install);
        let _ = fs::remove_dir_all(&venv);
        bail!(
            "installing uv=={} failed with {}{}",
            MANAGED_UV_VERSION,
            install.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    validate_uv(&uv)?;
    Ok(uv)
}

fn validate_uv(uv: &Path) -> Result<()> {
    let output = Command::new(uv)
        .arg("--version")
        .output()
        .with_context(|| format!("starting {}", uv.display()))?;
    if !output.status.success() {
        bail!("{} --version failed with {}", uv.display(), output.status);
    }
    let version = String::from_utf8_lossy(&output.stdout);
    if !uv_version_output_matches(&version) {
        bail!(
            "{} is {}, expected uv {}",
            uv.display(),
            version.trim(),
            MANAGED_UV_VERSION
        );
    }
    Ok(())
}

fn uv_version_output_matches(output: &str) -> bool {
    let mut fields = output.split_whitespace();
    fields.next() == Some("uv") && fields.next() == Some(MANAGED_UV_VERSION)
}

fn validate_ace_step_python(python: &Path, source_root: &Path, cache_root: &Path) -> Result<()> {
    let source_root_json = serde_json::to_string(&source_root.display().to_string())?;
    let script = format!(
        "import importlib.metadata,pathlib,sys; root=pathlib.Path({source_root_json}).resolve(); assert sys.version_info[:2] in ((3,11),(3,12)), sys.version; sys.path.insert(0,str(root)); import av,torch,transformers,diffusers,soundfile,acestep; assert importlib.metadata.version('av') == '18.0.0'; module=pathlib.Path(acestep.__file__).resolve(); assert module.is_relative_to(root), f'untrusted ACE-Step import: {{module}}'; print('__MAYHEM_ACE_STEP_RUNTIME__=' + str(module))"
    );
    let mut command = Command::new(python);
    configure_validation_cache(&mut command, cache_root)?;
    let output = command
        .arg("-c")
        .arg(script)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("DIFFUSERS_OFFLINE", "1")
        .output()
        .with_context(|| format!("starting {}", python.display()))?;
    if !output.status.success() {
        let detail = command_output_detail(&output);
        bail!(
            "{} could not validate the pinned ACE-Step runtime{}",
            python.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(())
}

fn verify_ace_step_supplemental_requirements() -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(ACE_STEP_SUPPLEMENTAL_REQUIREMENTS));
    ensure!(
        actual == ACE_STEP_SUPPLEMENTAL_REQUIREMENTS_SHA256,
        "embedded ACE-Step supplemental requirements checksum mismatch: expected {}, got {}",
        ACE_STEP_SUPPLEMENTAL_REQUIREMENTS_SHA256,
        actual
    );
    let pairs = exact_requirement_pairs(
        std::str::from_utf8(ACE_STEP_SUPPLEMENTAL_REQUIREMENTS)
            .context("ACE-Step supplemental requirements are not UTF-8")?,
    )
    .context("ACE-Step supplemental requirements must contain exact pins only")?;
    ensure!(
        pairs == [("av".to_owned(), "18.0.0".to_owned())],
        "ACE-Step supplemental requirements differ from the reviewed decoder pin"
    );
    let mut runtime = Sha256::new();
    runtime.update(b"mayhem/ace-step/runtime/v2\0");
    runtime.update(ACE_STEP_LOCK_SHA256.as_bytes());
    runtime.update(b"\0");
    runtime.update(ACE_STEP_SUPPLEMENTAL_REQUIREMENTS);
    let runtime_sha256 = format!("{:x}", runtime.finalize());
    ensure!(
        runtime_sha256 == ACE_STEP_RUNTIME_SHA256,
        "ACE-Step runtime identity mismatch: expected {}, got {}",
        ACE_STEP_RUNTIME_SHA256,
        runtime_sha256
    );
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn python_runtime_spec(backend: &str) -> Option<PythonRuntimeSpec> {
    match backend {
        "vllm" => Some(PythonRuntimeSpec {
            backend: "vllm",
            override_env: "MAYHEM_VLLM_PYTHON",
            distribution: "vllm",
            required_imports: &[
                "vllm",
                "torch",
                "transformers",
                "tokenizers",
                "safetensors",
                "compressed_tensors",
                "triton",
                "av",
            ],
            version: "0.24.0",
            requirements: VLLM_REQUIREMENTS,
            requirements_sha256: "34176fbb8bce38f1b9cb1e8be9754ad44aedebb7a7c3152b8050f6d6c99bbb5b",
            extra_index_urls: &[],
            min_free_bytes: 8 * GIB,
            embedded_module: None,
        }),
        "trt-llm" => Some(PythonRuntimeSpec {
            backend: "trt-llm",
            override_env: "MAYHEM_TRTLLM_PYTHON",
            distribution: "tensorrt_llm",
            required_imports: &["tensorrt_llm", "torch", "torchvision"],
            version: "1.3.0rc20",
            requirements: TRT_LLM_REQUIREMENTS,
            requirements_sha256: "af04f36cac8fa64b2694ab8d7709e837a69f49c7f5f3fe856594a628c8d5a8ff",
            extra_index_urls: &[
                "https://pypi.nvidia.com",
                "https://download.pytorch.org/whl/cu130",
            ],
            min_free_bytes: 12 * GIB,
            embedded_module: None,
        }),
        "mlx" => Some(PythonRuntimeSpec {
            backend: "mlx",
            override_env: "MAYHEM_MLX_PYTHON",
            distribution: "mlx-lm",
            required_imports: &[
                "mlx_lm",
                "mlx_vlm",
                "mlx",
                "llguidance",
                "av",
                "transformers",
                "tokenizers",
                "safetensors",
            ],
            version: "0.31.3",
            requirements: MLX_REQUIREMENTS,
            requirements_sha256: "6274d7958feccb4152cb6aa2494d6a59b4ba1966ce2eff7df6b245b5e272d111",
            extra_index_urls: &[],
            min_free_bytes: 2 * GIB,
            embedded_module: None,
        }),
        "llama-media" => Some(PythonRuntimeSpec {
            backend: "llama-media",
            override_env: "MAYHEM_LLAMA_MEDIA_PYTHON",
            distribution: "av",
            required_imports: &["av"],
            version: "18.0.0",
            requirements: LLAMA_MEDIA_REQUIREMENTS,
            requirements_sha256: "24cede85ce0cf7759803ac67fce16c071a42dd71625a87e7af7393ea52679c78",
            extra_index_urls: &[],
            min_free_bytes: GIB,
            embedded_module: None,
        }),
        "transformers-asr" => Some(PythonRuntimeSpec {
            backend: "transformers-asr",
            override_env: "MAYHEM_TRANSFORMERS_ASR_PYTHON",
            distribution: "transformers",
            required_imports: &[
                "transformers",
                "transformers.models.parakeet.modeling_parakeet",
                "torch",
                "tokenizers",
                "safetensors",
                "numpy",
                "soundfile",
                "soxr",
                "librosa",
            ],
            version: "5.14.1",
            requirements: TRANSFORMERS_ASR_REQUIREMENTS,
            requirements_sha256: "293ff8c2998e0fe7962e561e53b7f379b295460f294a407eabc2cacd5464827c",
            extra_index_urls: &[],
            min_free_bytes: 8 * GIB,
            embedded_module: None,
        }),
        "sulphur" => Some(PythonRuntimeSpec {
            backend: "sulphur",
            override_env: "MAYHEM_SULPHUR_PYTHON",
            distribution: "diffusers",
            required_imports: &[
                "mayhem_sulphur_runtime",
                "diffusers",
                "torch",
                "torchvision",
                "transformers",
                "tokenizers",
                "accelerate",
                "bitsandbytes",
                "peft",
                "safetensors",
                "gguf",
                "huggingface_hub",
                "av",
                "numpy",
                "PIL",
                "tqdm",
            ],
            version: "0.39.0",
            requirements: SULPHUR_REQUIREMENTS,
            requirements_sha256: "29e59b1cc3c096d3ceb521eac93e07c6c17c65c12f4c340cd21ad92919bbd23d",
            extra_index_urls: &["https://download.pytorch.org/whl/cu130"],
            min_free_bytes: 16 * GIB,
            embedded_module: Some(EmbeddedPythonModule {
                name: "mayhem_sulphur_runtime",
                source: SULPHUR_RUNTIME_ADAPTER,
                source_sha256: SULPHUR_RUNTIME_ADAPTER_SHA256,
            }),
        }),
        "sulphur-mlx" => Some(PythonRuntimeSpec {
            backend: "sulphur-mlx",
            override_env: "MAYHEM_SULPHUR_PYTHON",
            distribution: "ltx-pipelines-mlx",
            required_imports: &[
                "mayhem_sulphur_runtime",
                "ltx_pipelines_mlx",
                "ltx_core_mlx",
                "mlx",
                "mlx_lm",
                "transformers",
                "tokenizers",
                "safetensors",
                "numpy",
                "PIL",
            ],
            version: "0.14.19",
            requirements: SULPHUR_MLX_REQUIREMENTS,
            requirements_sha256: "8ce2e2f1a3d78256e4b41fe1adf4650ec8e84c6d4e9c48eef5734e09021fc7e3",
            extra_index_urls: &[],
            min_free_bytes: 16 * GIB,
            embedded_module: Some(EmbeddedPythonModule {
                name: "mayhem_sulphur_runtime",
                source: SULPHUR_MLX_RUNTIME_ADAPTER,
                source_sha256: SULPHUR_MLX_RUNTIME_ADAPTER_SHA256,
            }),
        }),
        _ => None,
    }
}

fn verify_requirements(spec: &PythonRuntimeSpec) -> Result<()> {
    let requirements = canonical_requirements(spec.requirements, spec.backend)?;
    let actual = format!("{:x}", Sha256::digest(requirements.as_ref()));
    if actual != spec.requirements_sha256 {
        bail!(
            "embedded {} requirements checksum mismatch: expected {}, got {}",
            spec.backend,
            spec.requirements_sha256,
            actual
        );
    }
    let text = std::str::from_utf8(requirements.as_ref())
        .with_context(|| format!("{} requirements are not UTF-8", spec.backend))?;
    let expected = format!("{}=={}", spec.distribution, spec.version);
    let pairs = exact_requirement_pairs(text).with_context(|| {
        format!(
            "embedded {} requirements must contain only exact name==version pins",
            spec.backend
        )
    })?;
    verify_embedded_python_wheels(spec)?;
    for wheel in embedded_python_wheels(spec) {
        let normalized = normalize_python_distribution(wheel.distribution);
        ensure!(
            !pairs.iter().any(|(distribution, _)| {
                normalize_python_distribution(distribution) == normalized
            }),
            "embedded {} wheel duplicates requirements distribution {}",
            spec.backend,
            wheel.distribution
        );
    }
    let expected_distribution = normalize_python_distribution(spec.distribution);
    let has_expected_distribution = pairs.iter().any(|(distribution, version)| {
        normalize_python_distribution(distribution) == expected_distribution
            && version == spec.version
    }) || embedded_python_wheels(spec).iter().any(|wheel| {
        normalize_python_distribution(wheel.distribution) == expected_distribution
            && wheel.version == spec.version
    });
    if !has_expected_distribution {
        bail!(
            "embedded {} runtime must include {} in its requirements or verified wheels",
            spec.backend,
            expected
        );
    }
    verify_embedded_python_module(spec)?;
    Ok(())
}

fn embedded_python_wheels(spec: &PythonRuntimeSpec) -> &'static [EmbeddedPythonWheel] {
    match spec.backend {
        "sulphur-mlx" => SULPHUR_MLX_WHEELS,
        _ => &[],
    }
}

fn normalize_python_distribution(distribution: &str) -> String {
    distribution.to_ascii_lowercase().replace('_', "-")
}

fn verify_embedded_python_wheels(spec: &PythonRuntimeSpec) -> Result<()> {
    let mut filenames = BTreeSet::new();
    let mut distributions = BTreeSet::new();
    for wheel in embedded_python_wheels(spec) {
        ensure!(
            !wheel.filename.is_empty()
                && wheel.filename.ends_with(".whl")
                && wheel.filename.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                }),
            "embedded {} wheel filename is unsafe: {}",
            spec.backend,
            wheel.filename
        );
        ensure!(
            !wheel.distribution.is_empty()
                && wheel.distribution.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                })
                && !wheel.version.is_empty()
                && wheel.version.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
                }),
            "embedded {} wheel identity is unsafe: {}=={}",
            spec.backend,
            wheel.distribution,
            wheel.version
        );
        ensure!(
            filenames.insert(wheel.filename),
            "duplicate embedded {} wheel filename {}",
            spec.backend,
            wheel.filename
        );
        ensure!(
            distributions.insert(normalize_python_distribution(wheel.distribution)),
            "duplicate embedded {} wheel distribution {}",
            spec.backend,
            wheel.distribution
        );
        let actual = format!("{:x}", Sha256::digest(wheel.source));
        ensure!(
            actual == wheel.source_sha256,
            "embedded {} wheel checksum mismatch for {}: expected {}, got {}",
            spec.backend,
            wheel.filename,
            wheel.source_sha256,
            actual
        );
    }
    Ok(())
}

fn verify_embedded_python_module(spec: &PythonRuntimeSpec) -> Result<()> {
    let Some(module) = spec.embedded_module else {
        return Ok(());
    };
    ensure!(
        !module.name.is_empty()
            && module.name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_'
                    || byte.is_ascii_alphanumeric() && (index > 0 || !byte.is_ascii_digit())
            }),
        "embedded {} module name is not a safe Python identifier",
        spec.backend
    );
    let actual = format!("{:x}", Sha256::digest(module.source));
    ensure!(
        actual == module.source_sha256,
        "embedded {} module checksum mismatch: expected {}, got {}",
        spec.backend,
        module.source_sha256,
        actual
    );
    Ok(())
}

fn install_embedded_python_wheels(
    python: &Path,
    venv: &Path,
    spec: &PythonRuntimeSpec,
) -> Result<()> {
    let wheels = embedded_python_wheels(spec);
    if wheels.is_empty() {
        return Ok(());
    }
    verify_embedded_python_wheels(spec)?;
    let wheel_dir = venv.join("mayhem-wheels");
    match fs::symlink_metadata(&wheel_dir) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "managed wheel directory {} is not a real directory",
            wheel_dir.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&wheel_dir).with_context(|| {
                format!("creating managed wheel directory {}", wheel_dir.display())
            })?;
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("reading managed wheel directory {}", wheel_dir.display())
            });
        }
    }
    let mut paths = Vec::with_capacity(wheels.len());
    for wheel in wheels {
        let path = wheel_dir.join(wheel.filename);
        materialize_embedded_regular_file(
            &path,
            wheel.source,
            wheel.source_sha256,
            "Python wheel",
        )?;
        paths.push(path);
    }
    let mut command = Command::new(python);
    command
        .arg("-m")
        .arg("pip")
        .arg("install")
        .arg("--disable-pip-version-check")
        .arg("--no-input")
        .arg("--no-deps")
        .arg("--force-reinstall");
    command.args(&paths);
    let install = command
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .env("PIP_NO_INPUT", "1")
        .output()
        .with_context(|| format!("starting pip for embedded {} runtime wheels", spec.backend))?;
    if !install.status.success() {
        let detail = command_output_detail(&install);
        bail!(
            "installing embedded {} runtime wheels failed with {}{}",
            spec.backend,
            install.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(())
}

fn install_embedded_python_module(
    python: &Path,
    venv: &Path,
    spec: &PythonRuntimeSpec,
) -> Result<()> {
    let Some(module) = spec.embedded_module else {
        return Ok(());
    };
    verify_embedded_python_module(spec)?;
    const PURELIB_MARKER: &str = "__MAYHEM_PURELIB__=";
    let output = Command::new(python)
        .arg("-I")
        .arg("-c")
        .arg(format!(
            "import sysconfig; print({PURELIB_MARKER:?} + sysconfig.get_path('purelib'))"
        ))
        .output()
        .with_context(|| format!("locating {} site-packages", python.display()))?;
    if !output.status.success() {
        bail!(
            "{} could not locate its managed site-packages{}",
            python.display(),
            command_output_detail(&output)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let purelib = stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(PURELIB_MARKER))
        .map(PathBuf::from)
        .context("managed Python did not report its site-packages path")?;
    let venv = fs::canonicalize(venv)
        .with_context(|| format!("canonicalizing managed venv {}", venv.display()))?;
    let purelib = fs::canonicalize(&purelib)
        .with_context(|| format!("canonicalizing managed site-packages {}", purelib.display()))?;
    ensure!(
        purelib.starts_with(&venv),
        "managed {} site-packages {} escaped venv {}",
        spec.backend,
        purelib.display(),
        venv.display()
    );
    materialize_embedded_python_module(&purelib, module)
}

fn materialize_embedded_python_module(purelib: &Path, module: EmbeddedPythonModule) -> Result<()> {
    let destination = purelib.join(format!("{}.py", module.name));
    materialize_embedded_regular_file(
        &destination,
        module.source,
        module.source_sha256,
        "Python module",
    )
}

fn materialize_embedded_regular_file(
    destination: &Path,
    source: &[u8],
    source_sha256: &str,
    label: &str,
) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        ensure!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "embedded {label} destination {} is not a regular file",
            destination.display()
        );
        if file_sha256(destination)? == source_sha256 {
            return Ok(());
        }
        fs::remove_file(destination)
            .with_context(|| format!("removing stale {label} {}", destination.display()))?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .context("embedded destination has no UTF-8 filename")?;
    let temporary =
        destination.with_file_name(format!(".{filename}.{}.{nonce}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("creating temporary {label} {}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(source)
        .with_context(|| format!("writing temporary {label} {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing temporary {label} {}", temporary.display()))?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error)
            .with_context(|| format!("installing embedded {label} {}", destination.display()));
    }
    ensure!(
        file_sha256(destination)? == source_sha256,
        "installed embedded {label} {} failed checksum verification",
        destination.display()
    );
    Ok(())
}

fn canonical_requirements<'a>(requirements: &'a [u8], backend: &str) -> Result<Cow<'a, [u8]>> {
    let text = std::str::from_utf8(requirements)
        .with_context(|| format!("{backend} requirements are not UTF-8"))?;
    if !text.contains('\r') {
        return Ok(Cow::Borrowed(requirements));
    }
    let normalized = text.replace("\r\n", "\n");
    if normalized.contains('\r') {
        bail!("{backend} requirements contain an unsupported bare carriage return");
    }
    Ok(Cow::Owned(normalized.into_bytes()))
}

fn exact_requirement_pairs(text: &str) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut seen = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (distribution, version) = line
            .split_once("==")
            .with_context(|| format!("requirement {line:?} is not exactly pinned"))?;
        if distribution.is_empty()
            || version.is_empty()
            || version.contains("==")
            || distribution.contains(char::is_whitespace)
            || version.contains(char::is_whitespace)
        {
            bail!("requirement {line:?} is not a plain name==version pin");
        }
        let normalized = normalize_python_distribution(distribution);
        if !seen.insert(normalized) {
            bail!("duplicate requirement distribution {distribution}");
        }
        pairs.push((distribution.to_owned(), version.to_owned()));
    }
    if pairs.is_empty() {
        bail!("requirements manifest is empty");
    }
    Ok(pairs)
}

fn validate_python(python: &Path, spec: &PythonRuntimeSpec, cache_root: &Path) -> Result<()> {
    let text = std::str::from_utf8(spec.requirements)
        .with_context(|| format!("{} requirements are not UTF-8", spec.backend))?;
    let mut expected_pairs = exact_requirement_pairs(text)?;
    for wheel in embedded_python_wheels(spec) {
        let normalized = normalize_python_distribution(wheel.distribution);
        ensure!(
            !expected_pairs.iter().any(|(distribution, _)| {
                normalize_python_distribution(distribution) == normalized
            }),
            "embedded {} wheel duplicates requirements distribution {}",
            spec.backend,
            wheel.distribution
        );
        expected_pairs.push((wheel.distribution.to_owned(), wheel.version.to_owned()));
    }
    let expected_versions = serde_json::to_string(&expected_pairs)?;
    let required_imports = serde_json::to_string(spec.required_imports)?;
    let embedded_module = serde_json::to_string(
        &spec
            .embedded_module
            .map(|module| (module.name, module.source_sha256)),
    )?;
    const VERSION_MARKER: &str = "__MAYHEM_RUNTIME_VERSION__=";
    let script = format!(
        "import hashlib,importlib,importlib.metadata as m,pathlib; expected=dict({expected_versions}); mismatched=[f'{{name}}={{m.version(name)}} (expected {{version}})' for name,version in expected.items() if not (m.version(name) == version or ('+' not in version and m.version(name).partition('+')[0] == version))]; assert not mismatched, '; '.join(mismatched); modules={{name:importlib.import_module(name) for name in {required_imports}}}; embedded={embedded_module}; embedded_ok=embedded is None or (pathlib.Path(modules[embedded[0]].__file__).is_file() and hashlib.sha256(pathlib.Path(modules[embedded[0]].__file__).read_bytes()).hexdigest()==embedded[1]); assert embedded_ok, 'embedded runtime module checksum mismatch'; print({:?} + m.version({:?}))",
        VERSION_MARKER, spec.distribution
    );
    let mut command = Command::new(python);
    configure_validation_cache(&mut command, cache_root)?;
    let output = command
        .arg("-c")
        .arg(&script)
        .output()
        .with_context(|| format!("starting {}", python.display()))?;
    if !output.status.success() {
        let detail = command_output_detail(&output);
        bail!(
            "{} could not validate {}=={}{}",
            python.display(),
            spec.distribution,
            spec.version,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(VERSION_MARKER))
        .with_context(|| {
            format!(
                "{} validated {} but did not emit its version marker",
                python.display(),
                spec.distribution
            )
        })?;
    if !python_distribution_version_matches(actual, spec.version) {
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

fn python_distribution_version_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || (!expected.contains('+')
            && actual
                .split_once('+')
                .is_some_and(|(public, local)| public == expected && !local.is_empty()))
}

fn configure_validation_cache(command: &mut Command, cache_root: &Path) -> Result<()> {
    for (name, default_path) in [
        ("XDG_CACHE_HOME", cache_root.join("xdg")),
        ("HF_HOME", cache_root.join("huggingface")),
        ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
        ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ("TORCH_HOME", cache_root.join("torch")),
    ] {
        let path = env::var_os(name).map(PathBuf::from).unwrap_or(default_path);
        fs::create_dir_all(&path)
            .with_context(|| format!("creating managed Python cache {}", path.display()))?;
        command.env(name, path);
    }
    Ok(())
}

fn command_output_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let mut chars = detail.chars().rev().take(2_000).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
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
    venv_executable(
        venv,
        if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        },
    )
}

fn venv_executable(venv: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join(name)
    } else {
        venv.join("bin").join(name)
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
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
        for backend in [
            "vllm",
            "trt-llm",
            "mlx",
            "llama-media",
            "transformers-asr",
            "sulphur",
            "sulphur-mlx",
        ] {
            let spec = python_runtime_spec(backend).expect("known backend");
            verify_requirements(&spec).expect("requirements verify");
            let pairs = exact_requirement_pairs(std::str::from_utf8(spec.requirements).unwrap())
                .expect("all requirements are exact");
            assert!(!pairs.is_empty());
        }
        let vllm = python_runtime_spec("vllm").expect("vLLM runtime");
        assert!(vllm.required_imports.contains(&"av"));
        assert!(
            exact_requirement_pairs(std::str::from_utf8(vllm.requirements).unwrap())
                .unwrap()
                .contains(&("av".to_owned(), "18.0.0".to_owned()))
        );
        let mlx = python_runtime_spec("mlx").expect("MLX runtime");
        assert!(mlx.required_imports.contains(&"mlx_vlm"));
        assert!(
            exact_requirement_pairs(std::str::from_utf8(mlx.requirements).unwrap())
                .unwrap()
                .contains(&("mlx-vlm".to_owned(), "0.6.3".to_owned()))
        );
        assert!(mlx.required_imports.contains(&"llguidance"));
        assert!(
            exact_requirement_pairs(std::str::from_utf8(mlx.requirements).unwrap())
                .unwrap()
                .contains(&("llguidance".to_owned(), "1.7.6".to_owned()))
        );

        let llama_media = python_runtime_spec("llama-media").expect("llama media runtime");
        assert_eq!(llama_media.required_imports, &["av"]);
        assert_eq!(
            exact_requirement_pairs(std::str::from_utf8(llama_media.requirements).unwrap())
                .unwrap(),
            vec![("av".to_owned(), "18.0.0".to_owned())]
        );

        let transformers_asr =
            python_runtime_spec("transformers-asr").expect("Transformers ASR runtime");
        assert!(transformers_asr
            .required_imports
            .contains(&"transformers.models.parakeet.modeling_parakeet"));
        assert!(transformers_asr.required_imports.contains(&"soundfile"));
        assert!(transformers_asr.required_imports.contains(&"soxr"));
        assert!(transformers_asr.required_imports.contains(&"librosa"));
        assert_eq!(
            exact_requirement_pairs(std::str::from_utf8(transformers_asr.requirements).unwrap())
                .unwrap(),
            vec![
                ("transformers".to_owned(), "5.14.1".to_owned()),
                ("torch".to_owned(), "2.13.0".to_owned()),
                ("tokenizers".to_owned(), "0.22.2".to_owned()),
                ("safetensors".to_owned(), "0.8.0".to_owned()),
                ("numpy".to_owned(), "2.4.6".to_owned()),
                ("soundfile".to_owned(), "0.14.0".to_owned()),
                ("soxr".to_owned(), "1.1.0".to_owned()),
                ("librosa".to_owned(), "0.11.0".to_owned()),
            ]
        );
        let sulphur = python_runtime_spec("sulphur").expect("Sulphur runtime");
        assert!(sulphur.required_imports.contains(&"mayhem_sulphur_runtime"));
        assert_eq!(
            sulphur.embedded_module.map(|module| module.name),
            Some("mayhem_sulphur_runtime")
        );
        verify_embedded_python_module(&sulphur).expect("embedded Sulphur adapter");
        let sulphur_mlx = python_runtime_spec("sulphur-mlx").expect("Sulphur MLX runtime");
        assert!(sulphur_mlx
            .required_imports
            .contains(&"mayhem_sulphur_runtime"));
        assert!(sulphur_mlx.required_imports.contains(&"ltx_pipelines_mlx"));
        assert!(sulphur_mlx.required_imports.contains(&"ltx_core_mlx"));
        assert_eq!(
            sulphur_mlx.embedded_module.map(|module| module.name),
            Some("mayhem_sulphur_runtime")
        );
        verify_embedded_python_module(&sulphur_mlx).expect("embedded Sulphur MLX adapter");
        verify_embedded_python_wheels(&sulphur_mlx).expect("embedded Sulphur MLX wheels");
        assert_eq!(
            embedded_python_wheels(&sulphur_mlx)
                .iter()
                .map(|wheel| (wheel.distribution, wheel.version))
                .collect::<Vec<_>>(),
            vec![
                ("ltx-core-mlx", "0.14.19"),
                ("ltx-pipelines-mlx", "0.14.19"),
            ]
        );
        let sulphur_mlx_requirements =
            exact_requirement_pairs(std::str::from_utf8(sulphur_mlx.requirements).unwrap())
                .unwrap();
        assert!(!sulphur_mlx_requirements.iter().any(|(distribution, _)| {
            matches!(
                normalize_python_distribution(distribution).as_str(),
                "ltx-2-mlx" | "ltx-core-mlx" | "ltx-pipelines-mlx"
            )
        }));
        verify_ace_step_supplemental_requirements()
            .expect("ACE-Step supplemental runtime requirements");
    }

    #[test]
    fn requirements_reject_ranges_options_and_duplicates() {
        assert!(exact_requirement_pairs("mlx-lm>=0.31.3\n").is_err());
        assert!(exact_requirement_pairs("--extra-index-url https://example.invalid\n").is_err());
        assert!(exact_requirement_pairs("mlx-lm==0.31.3\nmlx_lm==0.31.3\n").is_err());
    }

    #[test]
    fn requirements_hash_is_stable_across_windows_line_endings() {
        let mut spec = python_runtime_spec("transformers-asr").expect("Transformers ASR runtime");
        let crlf = std::str::from_utf8(spec.requirements)
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes()
            .into_boxed_slice();
        spec.requirements = Box::leak(crlf);
        verify_requirements(&spec).expect("CRLF requirements verify canonically");
        assert!(canonical_requirements(b"a==1\rb==2\n", "test").is_err());
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
        assert_eq!(
            python_runtime_spec("transformers-asr").map(|spec| spec.override_env),
            Some("MAYHEM_TRANSFORMERS_ASR_PYTHON")
        );
        assert_eq!(
            python_runtime_spec("llama-media").map(|spec| spec.override_env),
            Some("MAYHEM_LLAMA_MEDIA_PYTHON")
        );
        assert_eq!(
            python_runtime_spec("sulphur").map(|spec| spec.override_env),
            Some("MAYHEM_SULPHUR_PYTHON")
        );
        assert_eq!(
            python_runtime_spec("sulphur-mlx").map(|spec| spec.override_env),
            Some("MAYHEM_SULPHUR_PYTHON")
        );
        assert!(python_runtime_spec("llama.cpp").is_none());
    }

    #[test]
    fn cuda_runtime_indexes_stay_backend_specific() {
        assert!(python_runtime_spec("vllm")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert!(python_runtime_spec("mlx")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert!(python_runtime_spec("transformers-asr")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert!(python_runtime_spec("llama-media")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert!(python_runtime_spec("sulphur-mlx")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert_eq!(
            python_runtime_spec("trt-llm").unwrap().extra_index_urls,
            &[
                "https://pypi.nvidia.com",
                "https://download.pytorch.org/whl/cu130"
            ]
        );
        assert_eq!(
            python_runtime_spec("sulphur").unwrap().extra_index_urls,
            &["https://download.pytorch.org/whl/cu130"]
        );
    }

    #[test]
    fn embedded_sulphur_module_repairs_only_its_managed_regular_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let purelib = env::temp_dir().join(format!(
            "mayhem-sulphur-module-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&purelib).unwrap();
        let module = python_runtime_spec("sulphur")
            .unwrap()
            .embedded_module
            .unwrap();
        let destination = purelib.join("mayhem_sulphur_runtime.py");
        fs::write(&destination, b"stale").unwrap();

        materialize_embedded_python_module(&purelib, module).unwrap();
        assert_eq!(file_sha256(&destination).unwrap(), module.source_sha256);
        materialize_embedded_python_module(&purelib, module).unwrap();

        fs::remove_dir_all(purelib).unwrap();
    }

    #[test]
    fn embedded_sulphur_mlx_wheels_materialize_as_regular_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let wheel_dir = env::temp_dir().join(format!(
            "mayhem-sulphur-mlx-wheel-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&wheel_dir).unwrap();
        let spec = python_runtime_spec("sulphur-mlx").unwrap();
        for wheel in embedded_python_wheels(&spec) {
            let destination = wheel_dir.join(wheel.filename);
            materialize_embedded_regular_file(
                &destination,
                wheel.source,
                wheel.source_sha256,
                "Python wheel",
            )
            .unwrap();
            assert_eq!(file_sha256(&destination).unwrap(), wheel.source_sha256);
            materialize_embedded_regular_file(
                &destination,
                wheel.source,
                wheel.source_sha256,
                "Python wheel",
            )
            .unwrap();
        }
        fs::remove_dir_all(wheel_dir).unwrap();
    }

    #[test]
    fn embedded_sulphur_mlx_module_repairs_only_its_managed_regular_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let purelib = env::temp_dir().join(format!(
            "mayhem-sulphur-mlx-module-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&purelib).unwrap();
        let module = python_runtime_spec("sulphur-mlx")
            .unwrap()
            .embedded_module
            .unwrap();
        let destination = purelib.join("mayhem_sulphur_runtime.py");
        fs::write(&destination, b"stale").unwrap();

        materialize_embedded_python_module(&purelib, module).unwrap();
        assert_eq!(file_sha256(&destination).unwrap(), module.source_sha256);
        materialize_embedded_python_module(&purelib, module).unwrap();

        fs::remove_dir_all(purelib).unwrap();
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

    #[test]
    fn pinned_uv_accepts_informational_build_metadata_only() {
        assert!(uv_version_output_matches("uv 0.11.29\n"));
        assert!(uv_version_output_matches(
            "uv 0.11.29 (901092ee1 2026-07-15 x86_64-pc-windows-msvc)\n"
        ));
        assert!(!uv_version_output_matches("uv 0.11.28\n"));
        assert!(!uv_version_output_matches("uv 0.11.290\n"));
        assert!(!uv_version_output_matches("other 0.11.29\n"));
    }

    #[test]
    fn managed_runtime_versions_follow_pep440_local_version_matching() {
        assert!(python_distribution_version_matches("2.9.1", "2.9.1"));
        assert!(python_distribution_version_matches("2.9.1+cu130", "2.9.1"));
        assert!(!python_distribution_version_matches("2.9.2+cu130", "2.9.1"));
        assert!(!python_distribution_version_matches(
            "2.9.1+cu128",
            "2.9.1+cu130"
        ));
        assert!(!python_distribution_version_matches("2.9.1+", "2.9.1"));
    }

    #[test]
    fn ace_step_uv_builds_in_a_disposable_source_copy() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-ace-step-uv-project-test-{}-{nonce}",
            std::process::id()
        ));
        let source = root.join("verified");
        let cache = root.join("cache");
        fs::create_dir_all(source.join("acestep/third_parts/nano-vllm")).unwrap();
        fs::write(source.join("uv.lock"), b"locked").unwrap();
        fs::write(
            source.join("acestep/third_parts/nano-vllm/pyproject.toml"),
            b"[build-system]\n",
        )
        .unwrap();

        let project = temporary_ace_step_install_project(&source, &cache).unwrap();
        assert_eq!(fs::read(project.path().join("uv.lock")).unwrap(), b"locked");
        fs::create_dir_all(project.path().join("acestep/third_parts/nano-vllm/build")).unwrap();
        assert!(!source.join("acestep/third_parts/nano-vllm/build").exists());
        let project_path = project.path().to_path_buf();
        drop(project);
        assert!(!project_path.exists());
        assert!(source.join("uv.lock").is_file());

        fs::remove_dir_all(root).unwrap();
    }
}
