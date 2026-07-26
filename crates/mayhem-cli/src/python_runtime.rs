use std::borrow::Cow;
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use flate2::read::GzDecoder;
use fs2::FileExt;
use sha2::{Digest, Sha256};

const GIB: u64 = 1024 * 1024 * 1024;

const VLLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/vllm.txt");
const TRT_LLM_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/trt-llm.txt");
const MLX_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/mlx.txt");
const LLAMA_MEDIA_REQUIREMENTS: &[u8] = include_bytes!("../resources/python/llama-media.txt");
const TRANSFORMERS_ASR_REQUIREMENTS: &[u8] =
    include_bytes!("../resources/python/transformers-asr.txt");
const CHATTERBOX_CPU_PROJECT: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cpu/pyproject.toml");
const CHATTERBOX_CPU_LOCK: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cpu/uv.lock");
const CHATTERBOX_CPU_X86_PROJECT: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cpu-x86/pyproject.toml");
const CHATTERBOX_CPU_X86_LOCK: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cpu-x86/uv.lock");
const CHATTERBOX_CUDA124_PROJECT: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cuda124/pyproject.toml");
const CHATTERBOX_CUDA124_LOCK: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cuda124/uv.lock");
const CHATTERBOX_CUDA130_ARM64_PROJECT: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cuda130-arm64/pyproject.toml");
const CHATTERBOX_CUDA130_ARM64_LOCK: &[u8] =
    include_bytes!("../resources/python/chatterbox-runtime-cuda130-arm64/uv.lock");
const NEEDLE_CPU_ARM64_PROJECT: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cpu-arm64/pyproject.toml");
const NEEDLE_CPU_ARM64_LOCK: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cpu-arm64/uv.lock");
const NEEDLE_CPU_X86_PROJECT: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cpu-x86/pyproject.toml");
const NEEDLE_CPU_X86_LOCK: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cpu-x86/uv.lock");
const NEEDLE_CUDA130_ARM64_PROJECT: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cuda130-arm64/pyproject.toml");
const NEEDLE_CUDA130_ARM64_LOCK: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cuda130-arm64/uv.lock");
const NEEDLE_CUDA130_X86_PROJECT: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cuda130-x86/pyproject.toml");
const NEEDLE_CUDA130_X86_LOCK: &[u8] =
    include_bytes!("../resources/python/needle-runtime-cuda130-x86/uv.lock");
const SULPHUR_REQUIREMENTS: &[u8] =
    include_bytes!("../resources/python/sulphur-runtime-requirements.txt");
const SULPHUR_RUNTIME_ADAPTER: &[u8] = include_bytes!("../resources/python/sulphur_runtime.py");
const SULPHUR_RUNTIME_ADAPTER_SHA256: &str =
    "5ce2bf8bfe6143aa7eb6d91297600cfc507a0f57727f96547594afad7e1fb5ff";
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
const MANAGED_UV_MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MANAGED_UV_MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const MANAGED_UV_MAX_ZIP_ENTRIES: usize = 64;
const MANAGED_PYTHON_VERSION: &str = "3.12";
const CHATTERBOX_PYTHON_VERSION: &str = "3.11";
const CHATTERBOX_SOURCE_TREE_SHA256: &str =
    "0558df33c643af95371dc8dee5e811a1bde26974b08bd6c936fa5562e49aa720";
const CHATTERBOX_CPU_PROJECT_SHA256: &str =
    "1d366015abe9f3c3a89ac58030743758908bbe4c85bf29922fce5d438aea8e72";
const CHATTERBOX_CPU_LOCK_SHA256: &str =
    "7a4483fba5368339db0b555ce723be250180ca11faac6ff07156d874b2d5e944";
const CHATTERBOX_CPU_X86_PROJECT_SHA256: &str =
    "b9ee1a0046780bcfc2d332a53877a09a2bb6ceb45b1f09cc76be6eb8a4863cc4";
const CHATTERBOX_CPU_X86_LOCK_SHA256: &str =
    "b91d41bbe9ec47a1a28a64c37cf23eb788fed4b4c2cab7a857e3a3c3b479672c";
const CHATTERBOX_CUDA124_PROJECT_SHA256: &str =
    "0e46147713a73d3f7d771b03fdf83e5e03c6bdb96314a2700c4faec933e438e1";
const CHATTERBOX_CUDA124_LOCK_SHA256: &str =
    "2080d1e677da4dd0095f5afaf26d972a1d2fb5fba5ff8f83f19b8d17df378fa5";
const CHATTERBOX_CUDA130_ARM64_PROJECT_SHA256: &str =
    "17df5a7ca5b71872685ef23f39656395326c9814be05c88f25e4df986ea03518";
const CHATTERBOX_CUDA130_ARM64_LOCK_SHA256: &str =
    "afef8c6e5a717784ef477e7dbd9091b1bb08870f02d4afec1d36e89935191ad6";
const CHATTERBOX_MIN_FREE_BYTES: u64 = 16 * GIB;
const NEEDLE_PYTHON_VERSION: &str = "3.11";
const NEEDLE_CPU_ARM64_PROJECT_SHA256: &str =
    "f7ed2e8f740ad3de1e9e5f5585da21f8ae35b6aa9556be0c61ec8ea52d8eb7a6";
const NEEDLE_CPU_ARM64_LOCK_SHA256: &str =
    "91a168f4a3ab97699627621a2fd833ace13a1f5710bbfadb4e010265f924853c";
const NEEDLE_CPU_X86_PROJECT_SHA256: &str =
    "605fb6c116a360227f236c890efdd9af924cb1ae2831eb11a9f569072920cda8";
const NEEDLE_CPU_X86_LOCK_SHA256: &str =
    "52fd580a67bfff360184976ad195fdf76abc4aa35e41d3fdfdbe091242e00bd6";
const NEEDLE_CUDA130_ARM64_PROJECT_SHA256: &str =
    "acf40cc505e8ed705afe926b53adc9e8270f59c12a749738cb6c4c1254618638";
const NEEDLE_CUDA130_ARM64_LOCK_SHA256: &str =
    "5ef6768852fd83823c05457a6c058e2edb9902698f42958c4027e3fd1a6797db";
const NEEDLE_CUDA130_X86_PROJECT_SHA256: &str =
    "dba0ea384d584c2a5cfb640366f91a111e634ec711ff1871c595a49312253f1e";
const NEEDLE_CUDA130_X86_LOCK_SHA256: &str =
    "fa136bc03b6a843957fba29db06d0d66ee64b45ca0b1aaa344e6749e96d3523f";
const NEEDLE_CPU_MIN_FREE_BYTES: u64 = 4 * GIB;
const NEEDLE_CUDA_MIN_FREE_BYTES: u64 = 12 * GIB;
const NEEDLE_CUDA130_MIN_DRIVER_MAJOR: u32 = 580;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChatterboxRuntimeFlavor {
    CpuArm64,
    CpuX86,
    Cuda124,
    Cuda130Arm64,
}

#[derive(Clone, Copy, Debug)]
struct ChatterboxUvProject {
    name: &'static str,
    project: &'static [u8],
    project_sha256: &'static str,
    lock: &'static [u8],
    lock_sha256: &'static str,
    torch_version: &'static str,
    torchaudio_version: &'static str,
    cuda_version: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NeedleRuntimeFlavor {
    CpuArm64,
    CpuX86,
    Cuda130Arm64,
    Cuda130X86,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NeedleDevice {
    Cpu,
    Cuda,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NeedleRuntimeSelection {
    flavor: NeedleRuntimeFlavor,
    device: NeedleDevice,
}

#[derive(Clone, Copy, Debug)]
struct NeedleUvProject {
    name: &'static str,
    project: &'static [u8],
    project_sha256: &'static str,
    lock: &'static [u8],
    lock_sha256: &'static str,
    torch_version: &'static str,
    cuda_version: Option<&'static str>,
    min_cuda_compute_capability: Option<(u8, u8)>,
    min_free_bytes: u64,
}

pub(crate) fn ensure_backend_python(home: &Path, backend: &str) -> Result<PythonRuntime> {
    if backend == "ace-step" {
        return ensure_ace_step_python(home);
    }
    if backend == "chatterbox" {
        return ensure_chatterbox_python(home);
    }
    if backend == "needle" {
        return ensure_needle_python(home);
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

fn ensure_chatterbox_python(home: &Path) -> Result<PythonRuntime> {
    let flavor = select_chatterbox_runtime_flavor(
        env::var("MAYHEM_CHATTERBOX_DEVICE").ok().as_deref(),
        chatterbox_cuda_available(),
        env::consts::OS,
        env::consts::ARCH,
    )?;
    let project = chatterbox_uv_project(flavor);
    let runtime_sha256 = verify_chatterbox_uv_project(&project)?;
    let cache_root = home.join("cache").join("chatterbox").join(project.name);

    if let Some(explicit) = env::var_os("MAYHEM_CHATTERBOX_PYTHON") {
        let python = validate_explicit_chatterbox_python_path(home, Path::new(&explicit))?;
        let venv = explicit_chatterbox_venv_root(&python)?;
        check_chatterbox_environment(home, &venv, &project, &cache_root)?;
        validate_chatterbox_python(&python, &project, &cache_root).with_context(|| {
            "MAYHEM_CHATTERBOX_PYTHON points to an unusable frozen Chatterbox runtime; use a user-owned Python 3.11 environment matching the embedded lock or unset the override"
        })?;
        return Ok(PythonRuntime {
            python,
            source: "explicit MAYHEM_CHATTERBOX_PYTHON".to_owned(),
            requirements_sha256: runtime_sha256,
        });
    }

    let venvs = home.join("venvs");
    fs::create_dir_all(&venvs)
        .with_context(|| format!("creating managed Python directory {}", venvs.display()))?;
    let venv = venvs.join(format!("chatterbox-{}", project.name));
    let python = venv_python(&venv);
    if validate_managed_chatterbox_python(
        &python,
        &venv,
        &project,
        &cache_root,
        &runtime_sha256,
        home,
    )
    .is_ok()
    {
        return Ok(PythonRuntime {
            python,
            source: format!("managed existing frozen uv runtime ({})", project.name),
            requirements_sha256: runtime_sha256,
        });
    }

    let bootstrap_lock_path = venvs.join(format!(".chatterbox-{}.bootstrap.lock", project.name));
    let bootstrap_lock = open_lock_file(&bootstrap_lock_path)?;
    bootstrap_lock.lock_exclusive().with_context(|| {
        format!(
            "locking managed Chatterbox bootstrap {}",
            bootstrap_lock_path.display()
        )
    })?;

    let result = (|| {
        if validate_managed_chatterbox_python(
            &python,
            &venv,
            &project,
            &cache_root,
            &runtime_sha256,
            home,
        )
        .is_ok()
        {
            return Ok(PythonRuntime {
                python,
                source: format!("managed existing frozen uv runtime ({})", project.name),
                requirements_sha256: runtime_sha256.clone(),
            });
        }

        let free_bytes = fs2::available_space(&venvs).with_context(|| {
            format!(
                "reading free space before Chatterbox {} Python bootstrap under {}",
                project.name,
                venvs.display()
            )
        })?;
        if free_bytes < CHATTERBOX_MIN_FREE_BYTES {
            bail!(
                "Chatterbox {} Python bootstrap needs at least {} GiB free under {}; only {} GiB is available",
                project.name,
                CHATTERBOX_MIN_FREE_BYTES / GIB,
                venvs.display(),
                free_bytes / GIB
            );
        }
        if venv.exists() {
            cleanup_incomplete_chatterbox_venv(&venv)?;
        }

        let install_result = (|| {
            let uv = ensure_managed_uv(home)?;
            let install_project = temporary_chatterbox_install_project(&project, &cache_root)?;
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
                .arg(CHATTERBOX_PYTHON_VERSION)
                .arg("--project")
                .arg(install_project.path())
                .env("UV_PROJECT_ENVIRONMENT", &venv)
                .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
                .env("UV_CACHE_DIR", &uv_cache)
                .env("UV_NO_PROGRESS", "1")
                .output()
                .with_context(|| {
                    format!(
                        "starting {} frozen sync for Chatterbox {}",
                        uv.display(),
                        project.name
                    )
                })?;
            if !install.status.success() {
                let detail = command_output_detail(&install);
                bail!(
                    "installing the frozen Chatterbox {} runtime failed with {}{}",
                    project.name,
                    install.status,
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                );
            }
            let managed_python = venv_python(&venv);
            validate_chatterbox_python(&managed_python, &project, &cache_root).with_context(
                || {
                    format!(
                        "managed Chatterbox {} install completed but runtime validation failed",
                        project.name
                    )
                },
            )?;
            write_chatterbox_runtime_marker(&venv, &runtime_sha256)?;
            validate_managed_chatterbox_python(
                &managed_python,
                &venv,
                &project,
                &cache_root,
                &runtime_sha256,
                home,
            )?;
            Ok(managed_python)
        })();

        let managed_python = match install_result {
            Ok(python) => python,
            Err(error) => {
                cleanup_incomplete_chatterbox_venv(&venv).with_context(|| {
                    format!(
                        "cleaning incomplete Chatterbox {} environment {} after: {error:#}",
                        project.name,
                        venv.display()
                    )
                })?;
                return Err(error);
            }
        };
        Ok(PythonRuntime {
            python: managed_python,
            source: format!("managed bootstrapped frozen uv runtime ({})", project.name),
            requirements_sha256: runtime_sha256.clone(),
        })
    })();

    let _ = FileExt::unlock(&bootstrap_lock);
    result
}

fn ensure_needle_python(home: &Path) -> Result<PythonRuntime> {
    let requested = env::var("MAYHEM_NEEDLE_DEVICE").ok();
    ensure_needle_python_requested(home, requested.as_deref())
}

pub(crate) fn ensure_needle_python_for_device(home: &Path, device: &str) -> Result<PythonRuntime> {
    let resolved = resolve_needle_device_for_request(device)?;
    ensure_needle_python_requested(home, Some(resolved))
}

pub(crate) fn resolve_needle_device_for_request(device: &str) -> Result<&'static str> {
    let requested = normalize_explicit_needle_device(device)?;
    let selection = select_needle_runtime(
        Some(&requested),
        needle_cuda_available(),
        env::consts::OS,
        env::consts::ARCH,
    )?;
    Ok(needle_device_name(selection.device))
}

fn normalize_explicit_needle_device(device: &str) -> Result<String> {
    let requested = device.trim().to_ascii_lowercase();
    ensure!(
        matches!(requested.as_str(), "cpu" | "gpu" | "cuda"),
        "Needle device must be cpu, gpu, or cuda"
    );
    Ok(requested)
}

fn ensure_needle_python_requested(home: &Path, requested: Option<&str>) -> Result<PythonRuntime> {
    let selection = select_needle_runtime(
        requested,
        needle_cuda_available(),
        env::consts::OS,
        env::consts::ARCH,
    )?;
    let project = needle_uv_project(selection.flavor);
    let runtime_sha256 = verify_needle_uv_project(&project)?;
    let cache_root = home.join("cache").join("needle").join(project.name);
    let venvs = home.join("venvs");
    fs::create_dir_all(&venvs)
        .with_context(|| format!("creating managed Python directory {}", venvs.display()))?;
    let venv = venvs.join(format!("needle-{}", project.name));
    let python = venv_python(&venv);

    if validate_existing_needle_runtime(&venv, &runtime_sha256, || {
        validate_managed_needle_python(
            &python,
            &venv,
            &project,
            selection.device,
            &cache_root,
            &runtime_sha256,
            home,
        )
    })? {
        return Ok(PythonRuntime {
            python,
            source: format!(
                "managed existing frozen Needle uv runtime ({}/{})",
                project.name,
                needle_device_name(selection.device)
            ),
            requirements_sha256: runtime_sha256,
        });
    }

    let bootstrap_lock_path = venvs.join(format!(".needle-{}.bootstrap.lock", project.name));
    let bootstrap_lock = open_lock_file(&bootstrap_lock_path)?;
    bootstrap_lock.lock_exclusive().with_context(|| {
        format!(
            "locking managed Needle bootstrap {}",
            bootstrap_lock_path.display()
        )
    })?;

    let result = (|| {
        if validate_existing_needle_runtime(&venv, &runtime_sha256, || {
            validate_managed_needle_python(
                &python,
                &venv,
                &project,
                selection.device,
                &cache_root,
                &runtime_sha256,
                home,
            )
        })? {
            return Ok(PythonRuntime {
                python,
                source: format!(
                    "managed existing frozen Needle uv runtime ({}/{})",
                    project.name,
                    needle_device_name(selection.device)
                ),
                requirements_sha256: runtime_sha256.clone(),
            });
        }

        let free_bytes = fs2::available_space(&venvs).with_context(|| {
            format!(
                "reading free space before Needle {} Python bootstrap under {}",
                project.name,
                venvs.display()
            )
        })?;
        if free_bytes < project.min_free_bytes {
            bail!(
                "Needle {} Python bootstrap needs at least {} GiB free under {}; only {} GiB is available",
                project.name,
                project.min_free_bytes / GIB,
                venvs.display(),
                free_bytes / GIB
            );
        }
        if venv.exists() {
            cleanup_incomplete_needle_venv(&venv)?;
        }

        let install_result = (|| {
            let uv = ensure_managed_uv(home)?;
            let install_project = temporary_needle_install_project(&project, &cache_root)?;
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
                .arg(NEEDLE_PYTHON_VERSION)
                .arg("--project")
                .arg(install_project.path())
                .env("UV_PROJECT_ENVIRONMENT", &venv)
                .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
                .env("UV_CACHE_DIR", &uv_cache)
                .env("UV_NO_PROGRESS", "1")
                .output()
                .with_context(|| {
                    format!(
                        "starting {} frozen sync for Needle {}",
                        uv.display(),
                        project.name
                    )
                })?;
            if !install.status.success() {
                let detail = command_output_detail(&install);
                bail!(
                    "installing the frozen Needle {} runtime failed with {}{}",
                    project.name,
                    install.status,
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                );
            }
            let managed_python = venv_python(&venv);
            check_needle_environment(home, &venv, &project, &cache_root).with_context(|| {
                format!(
                    "managed Needle {} install completed but its frozen environment check failed",
                    project.name
                )
            })?;
            validate_needle_python(&managed_python, &project, selection.device, &cache_root)
                .with_context(|| {
                    format!(
                        "managed Needle {} install completed but runtime validation failed",
                        project.name
                    )
                })?;
            write_needle_runtime_marker(&venv, &runtime_sha256)?;
            Ok(managed_python)
        })();

        let managed_python = match install_result {
            Ok(python) => python,
            Err(error) => {
                cleanup_incomplete_needle_venv(&venv).with_context(|| {
                    format!(
                        "cleaning incomplete Needle {} environment {} after: {error:#}",
                        project.name,
                        venv.display()
                    )
                })?;
                return Err(error);
            }
        };
        Ok(PythonRuntime {
            python: managed_python,
            source: format!(
                "managed bootstrapped frozen Needle uv runtime ({}/{})",
                project.name,
                needle_device_name(selection.device)
            ),
            requirements_sha256: runtime_sha256.clone(),
        })
    })();

    let _ = FileExt::unlock(&bootstrap_lock);
    result
}

fn needle_uv_project(flavor: NeedleRuntimeFlavor) -> NeedleUvProject {
    match flavor {
        NeedleRuntimeFlavor::CpuArm64 => NeedleUvProject {
            name: "cpu-arm64",
            project: NEEDLE_CPU_ARM64_PROJECT,
            project_sha256: NEEDLE_CPU_ARM64_PROJECT_SHA256,
            lock: NEEDLE_CPU_ARM64_LOCK,
            lock_sha256: NEEDLE_CPU_ARM64_LOCK_SHA256,
            torch_version: "2.9.1",
            cuda_version: None,
            min_cuda_compute_capability: None,
            min_free_bytes: NEEDLE_CPU_MIN_FREE_BYTES,
        },
        NeedleRuntimeFlavor::CpuX86 => NeedleUvProject {
            name: "cpu-x86",
            project: NEEDLE_CPU_X86_PROJECT,
            project_sha256: NEEDLE_CPU_X86_PROJECT_SHA256,
            lock: NEEDLE_CPU_X86_LOCK,
            lock_sha256: NEEDLE_CPU_X86_LOCK_SHA256,
            torch_version: "2.9.1+cpu",
            cuda_version: None,
            min_cuda_compute_capability: None,
            min_free_bytes: NEEDLE_CPU_MIN_FREE_BYTES,
        },
        NeedleRuntimeFlavor::Cuda130Arm64 => NeedleUvProject {
            name: "cuda130-arm64",
            project: NEEDLE_CUDA130_ARM64_PROJECT,
            project_sha256: NEEDLE_CUDA130_ARM64_PROJECT_SHA256,
            lock: NEEDLE_CUDA130_ARM64_LOCK,
            lock_sha256: NEEDLE_CUDA130_ARM64_LOCK_SHA256,
            torch_version: "2.9.1+cu130",
            cuda_version: Some("13.0"),
            min_cuda_compute_capability: Some((7, 5)),
            min_free_bytes: NEEDLE_CUDA_MIN_FREE_BYTES,
        },
        NeedleRuntimeFlavor::Cuda130X86 => NeedleUvProject {
            name: "cuda130-x86",
            project: NEEDLE_CUDA130_X86_PROJECT,
            project_sha256: NEEDLE_CUDA130_X86_PROJECT_SHA256,
            lock: NEEDLE_CUDA130_X86_LOCK,
            lock_sha256: NEEDLE_CUDA130_X86_LOCK_SHA256,
            torch_version: "2.9.1+cu130",
            cuda_version: Some("13.0"),
            min_cuda_compute_capability: Some((7, 5)),
            min_free_bytes: NEEDLE_CUDA_MIN_FREE_BYTES,
        },
    }
}

fn select_needle_runtime(
    requested: Option<&str>,
    cuda_available: bool,
    target_os: &str,
    target_arch: &str,
) -> Result<NeedleRuntimeSelection> {
    let requested = requested.unwrap_or("auto").trim().to_ascii_lowercase();
    ensure!(
        matches!(requested.as_str(), "auto" | "cpu" | "gpu" | "cuda"),
        "MAYHEM_NEEDLE_DEVICE must be auto, cpu, gpu, or cuda"
    );
    match (target_os, target_arch) {
        ("macos", "aarch64") => match requested.as_str() {
            "gpu" | "cuda" => {
                bail!("Needle GPU execution is CUDA-only; calibrated Apple hosts use needle-cpu")
            }
            "auto" | "cpu" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }),
            _ => unreachable!(),
        },
        ("linux", "aarch64") => match requested.as_str() {
            "gpu" | "cuda" => {
                ensure!(
                    cuda_available,
                    "Needle GPU execution requires a usable NVIDIA device with driver r580 or newer for CUDA 13"
                );
                Ok(NeedleRuntimeSelection {
                    flavor: NeedleRuntimeFlavor::Cuda130Arm64,
                    device: NeedleDevice::Cuda,
                })
            }
            "cpu" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }),
            "auto" if cuda_available => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130Arm64,
                device: NeedleDevice::Cuda,
            }),
            "auto" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }),
            _ => unreachable!(),
        },
        ("linux", "x86_64") => match requested.as_str() {
            "cpu" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }),
            "gpu" | "cuda" if cuda_available => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }),
            "gpu" | "cuda" => {
                bail!("Needle GPU execution requires a usable NVIDIA device with driver r580 or newer for CUDA 13")
            }
            "auto" if cuda_available => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }),
            "auto" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }),
            _ => unreachable!(),
        },
        ("windows", "x86_64") => match requested.as_str() {
            "cpu" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }),
            "gpu" | "cuda" if cuda_available => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }),
            "gpu" | "cuda" => {
                bail!("Needle GPU execution requires a usable NVIDIA device with driver r580 or newer for CUDA 13")
            }
            "auto" if cuda_available => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }),
            "auto" => Ok(NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }),
            _ => unreachable!(),
        },
        _ => bail!("the frozen Needle runtime does not support {target_os}/{target_arch}"),
    }
}

fn needle_cuda_available() -> bool {
    if !matches!(
        (env::consts::OS, env::consts::ARCH),
        ("linux", "aarch64") | ("linux", "x86_64") | ("windows", "x86_64")
    ) {
        return false;
    }
    Command::new("nvidia-smi")
        .args([
            "--query-gpu=driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && needle_cuda_driver_supported(&String::from_utf8_lossy(&output.stdout))
        })
}

fn needle_cuda_driver_supported(output: &str) -> bool {
    output.lines().any(|line| {
        line.trim()
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= NEEDLE_CUDA130_MIN_DRIVER_MAJOR)
    })
}

fn needle_device_name(device: NeedleDevice) -> &'static str {
    match device {
        NeedleDevice::Cpu => "cpu",
        NeedleDevice::Cuda => "cuda",
    }
}

fn verify_needle_uv_project(project: &NeedleUvProject) -> Result<String> {
    let project_actual = format!("{:x}", Sha256::digest(project.project));
    ensure!(
        project_actual == project.project_sha256,
        "embedded Needle {} pyproject checksum mismatch: expected {}, got {}",
        project.name,
        project.project_sha256,
        project_actual
    );
    let lock_actual = format!("{:x}", Sha256::digest(project.lock));
    ensure!(
        lock_actual == project.lock_sha256,
        "embedded Needle {} uv.lock checksum mismatch: expected {}, got {}",
        project.name,
        project.lock_sha256,
        lock_actual
    );

    let project_text = std::str::from_utf8(project.project)
        .with_context(|| format!("embedded Needle {} pyproject is not UTF-8", project.name))?;
    let project_toml: toml::Value = toml::from_str(project_text)
        .with_context(|| format!("parsing embedded Needle {} pyproject", project.name))?;
    let dependencies = project_toml
        .get("project")
        .and_then(|value| value.get("dependencies"))
        .and_then(toml::Value::as_array)
        .context("embedded Needle pyproject has no dependency array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .context("embedded Needle pyproject dependency is not a string")
        })
        .collect::<Result<Vec<_>>>()?;
    let expected_dependencies = vec![
        "numpy==1.26.4",
        "safetensors==0.5.3",
        "sentencepiece==0.2.1",
        match project.cuda_version {
            Some(_) => "torch==2.9.1+cu130",
            None if project.name == "cpu-x86" => "torch==2.9.1+cpu",
            None => "torch==2.9.1",
        },
        "transformers==5.2.0",
    ];
    ensure!(
        dependencies == expected_dependencies,
        "embedded Needle {} pyproject dependency topology changed",
        project.name
    );

    let lock_text = std::str::from_utf8(project.lock)
        .with_context(|| format!("embedded Needle {} uv.lock is not UTF-8", project.name))?;
    let lock: toml::Value = toml::from_str(lock_text)
        .with_context(|| format!("parsing embedded Needle {} uv.lock", project.name))?;
    ensure!(
        lock.get("requires-python").and_then(toml::Value::as_str) == Some("==3.11.*"),
        "embedded Needle {} uv.lock is not pinned to Python 3.11",
        project.name
    );
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .context("embedded Needle uv.lock has no package array")?;
    let mut direct_versions = std::collections::BTreeMap::new();
    for package in packages {
        let table = package
            .as_table()
            .context("embedded Needle uv.lock package is not a table")?;
        let name = table
            .get("name")
            .and_then(toml::Value::as_str)
            .context("embedded Needle uv.lock package has no name")?;
        ensure!(
            !matches!(
                name,
                "accelerate"
                    | "cactus"
                    | "datasets"
                    | "jax"
                    | "jaxlib"
                    | "peft"
                    | "tensorboard"
                    | "trl"
                    | "wandb"
            ),
            "embedded Needle {} lock contains forbidden runtime package {}",
            project.name,
            name
        );
        if matches!(
            name,
            "numpy" | "safetensors" | "sentencepiece" | "torch" | "transformers"
        ) {
            direct_versions.insert(
                name,
                table
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .context("embedded Needle lock package has no version")?,
            );
        }
        let source = table
            .get("source")
            .and_then(toml::Value::as_table)
            .context("embedded Needle uv.lock package has no source")?;
        if let Some(registry) = source.get("registry").and_then(toml::Value::as_str) {
            ensure!(
                registry.starts_with("https://"),
                "embedded Needle {} lock uses a non-HTTPS registry for {}",
                project.name,
                name
            );
            verify_locked_registry_artifacts(project.name, name, table)?;
        } else {
            ensure!(
                source.get("virtual").is_some(),
                "embedded Needle {} lock contains a non-registry dependency for {}",
                project.name,
                name
            );
        }
    }
    for (name, version) in [
        ("numpy", "1.26.4"),
        ("safetensors", "0.5.3"),
        ("sentencepiece", "0.2.1"),
        ("torch", project.torch_version),
        ("transformers", "5.2.0"),
    ] {
        ensure!(
            direct_versions.get(name).copied() == Some(version),
            "embedded Needle {} lock does not contain {}=={}",
            project.name,
            name,
            version
        );
    }

    let mut runtime = Sha256::new();
    runtime.update(b"mayhem/needle/runtime/v1\0");
    runtime.update(project.name.as_bytes());
    runtime.update(b"\0");
    runtime.update(project.project_sha256.as_bytes());
    runtime.update(b"\0");
    runtime.update(project.lock_sha256.as_bytes());
    Ok(format!("{:x}", runtime.finalize()))
}

struct TemporaryNeedleProject {
    path: PathBuf,
}

impl TemporaryNeedleProject {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryNeedleProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn validate_existing_needle_runtime<F>(
    venv: &Path,
    runtime_sha256: &str,
    validate_health: F,
) -> Result<bool>
where
    F: FnOnce() -> Result<()>,
{
    let metadata = match fs::symlink_metadata(venv) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("reading existing Needle environment {}", venv.display())
            });
        }
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "existing Needle environment {} is not a regular directory",
        venv.display()
    );

    let marker = venv.join(".mayhem-needle-runtime-sha256");
    let marker_metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading Needle runtime marker {}", marker.display()));
        }
    };
    ensure!(
        marker_metadata.is_file() && !marker_metadata.file_type().is_symlink(),
        "Needle runtime marker {} is not a regular file",
        marker.display()
    );
    let actual = fs::read_to_string(&marker)
        .with_context(|| format!("reading Needle runtime marker {}", marker.display()))?;
    if actual.trim() != runtime_sha256 {
        return Ok(false);
    }

    validate_health().with_context(|| {
        format!(
            "existing complete Needle environment {} is temporarily unhealthy; preserving it without reinstall",
            venv.display()
        )
    })?;
    Ok(true)
}

fn temporary_needle_install_project(
    project: &NeedleUvProject,
    cache_root: &Path,
) -> Result<TemporaryNeedleProject> {
    let projects = cache_root.join("uv-projects");
    fs::create_dir_all(&projects)
        .with_context(|| format!("creating Needle uv project root {}", projects.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = projects.join(format!(
        ".needle-{}.{}.{}.tmp",
        project.name,
        std::process::id(),
        nonce
    ));
    fs::create_dir(&path)
        .with_context(|| format!("creating disposable Needle project {}", path.display()))?;
    let result = (|| {
        materialize_embedded_regular_file(
            &path.join("pyproject.toml"),
            project.project,
            project.project_sha256,
            "Needle pyproject",
        )?;
        materialize_embedded_regular_file(
            &path.join("uv.lock"),
            project.lock,
            project.lock_sha256,
            "Needle uv.lock",
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(TemporaryNeedleProject { path })
}

fn cleanup_incomplete_needle_venv(venv: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(venv) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading incomplete Needle venv {}", venv.display()));
        }
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "refusing to remove non-directory or symlink Needle venv {}",
        venv.display()
    );
    fs::remove_dir_all(venv)
        .with_context(|| format!("removing incomplete Needle venv {}", venv.display()))
}

fn validate_managed_needle_python(
    python: &Path,
    venv: &Path,
    project: &NeedleUvProject,
    device: NeedleDevice,
    cache_root: &Path,
    runtime_sha256: &str,
    home: &Path,
) -> Result<()> {
    let marker = venv.join(".mayhem-needle-runtime-sha256");
    let metadata = fs::symlink_metadata(&marker)
        .with_context(|| format!("reading Needle runtime marker {}", marker.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Needle runtime marker {} is not a regular file",
        marker.display()
    );
    let actual = fs::read_to_string(&marker)
        .with_context(|| format!("reading Needle runtime marker {}", marker.display()))?;
    ensure!(
        actual.trim() == runtime_sha256,
        "managed Needle {} runtime identity changed",
        project.name
    );
    check_needle_environment(home, venv, project, cache_root)?;
    validate_needle_python(python, project, device, cache_root)
}

fn check_needle_environment(
    home: &Path,
    venv: &Path,
    project: &NeedleUvProject,
    cache_root: &Path,
) -> Result<()> {
    let uv = ensure_managed_uv(home)?;
    let install_project = temporary_needle_install_project(project, cache_root)?;
    let uv_cache = cache_root.join("uv");
    fs::create_dir_all(&uv_cache)
        .with_context(|| format!("creating uv cache directory {}", uv_cache.display()))?;
    let check = Command::new(&uv)
        .arg("sync")
        .arg("--check")
        .arg("--frozen")
        .arg("--no-dev")
        .arg("--no-install-project")
        .arg("--project")
        .arg(install_project.path())
        .env("UV_PROJECT_ENVIRONMENT", venv)
        .env("UV_CACHE_DIR", &uv_cache)
        .env("UV_NO_PROGRESS", "1")
        .env("UV_OFFLINE", "1")
        .output()
        .with_context(|| {
            format!(
                "checking Needle {} environment with {}",
                project.name,
                uv.display()
            )
        })?;
    if check.status.success() {
        return Ok(());
    }
    let detail = command_output_detail(&check);
    if project.name == "cuda130-arm64" {
        return check_needle_cuda_environment_with_malformed_cusparselt(
            &uv,
            venv,
            project,
            &install_project,
            &uv_cache,
        )
        .with_context(|| {
            format!(
                "Needle {} environment failed its frozen uv check{}",
                project.name,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )
        });
    }
    bail!(
        "Needle {} environment is not synchronized with its frozen uv.lock{}",
        project.name,
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )
}

fn check_needle_cuda_environment_with_malformed_cusparselt(
    uv: &Path,
    venv: &Path,
    project: &NeedleUvProject,
    install_project: &TemporaryNeedleProject,
    uv_cache: &Path,
) -> Result<()> {
    let check = Command::new(uv)
        .arg("sync")
        .arg("--check")
        .arg("--frozen")
        .arg("--no-dev")
        .arg("--no-install-project")
        .arg("--no-install-package")
        .arg("nvidia-cusparselt-cu13")
        .arg("--inexact")
        .arg("--project")
        .arg(install_project.path())
        .env("UV_PROJECT_ENVIRONMENT", venv)
        .env("UV_CACHE_DIR", uv_cache)
        .env("UV_NO_PROGRESS", "1")
        .env("UV_OFFLINE", "1")
        .output()
        .context("checking the frozen Needle CUDA environment around NVIDIA's malformed wheel")?;
    ensure!(
        check.status.success(),
        "Needle CUDA environment differs from its lock beyond nvidia-cusparselt-cu13: {}",
        command_output_detail(&check)
    );

    let listed = Command::new(uv)
        .arg("pip")
        .arg("list")
        .arg("--python")
        .arg(venv_python(venv))
        .arg("--format")
        .arg("json")
        .env("UV_CACHE_DIR", uv_cache)
        .env("UV_OFFLINE", "1")
        .output()
        .context("listing installed Needle CUDA distributions")?;
    ensure!(
        listed.status.success(),
        "listing installed Needle CUDA distributions failed: {}",
        command_output_detail(&listed)
    );
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&listed.stdout).context("parsing Needle CUDA distribution list")?;
    let mut actual = std::collections::BTreeMap::new();
    for value in values {
        let name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("installed Needle CUDA distribution has no name")?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .context("installed Needle CUDA distribution has no version")?;
        ensure!(
            actual
                .insert(normalize_python_distribution_name(name), version.to_owned())
                .is_none(),
            "Needle CUDA environment contains a duplicate distribution named {name}"
        );
    }
    let expected = needle_cuda_expected_distributions(project)?;
    ensure!(
        actual == expected,
        "Needle CUDA environment distribution set differs from its frozen lock"
    );

    let version = expected
        .get("nvidia-cusparselt-cu13")
        .context("Needle CUDA lock has no nvidia-cusparselt-cu13 distribution")?;
    let wheel = venv
        .join("lib/python3.11/site-packages")
        .join(format!("nvidia_cusparselt_cu13-{version}.dist-info/WHEEL"));
    let metadata = fs::symlink_metadata(&wheel).with_context(|| {
        format!(
            "inspecting malformed NVIDIA wheel metadata {}",
            wheel.display()
        )
    })?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "malformed NVIDIA wheel metadata must be a regular non-symlink file"
    );
    let wheel =
        fs::read_to_string(&wheel).context("reading malformed NVIDIA cuSPARSELt wheel metadata")?;
    let tags = wheel
        .lines()
        .filter_map(|line| line.strip_prefix("Tag: "))
        .collect::<Vec<_>>();
    ensure!(
        tags == ["py3-none-manylinux2014_sbsa"],
        "nvidia-cusparselt-cu13 no longer has the one publisher tag defect covered by this verifier"
    );
    Ok(())
}

fn needle_cuda_expected_distributions(
    project: &NeedleUvProject,
) -> Result<std::collections::BTreeMap<String, String>> {
    let lock_text =
        std::str::from_utf8(project.lock).context("embedded Needle CUDA uv.lock is not UTF-8")?;
    let lock: toml::Value =
        toml::from_str(lock_text).context("parsing embedded Needle CUDA uv.lock")?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .context("embedded Needle CUDA uv.lock has no package array")?;
    let mut expected = std::collections::BTreeMap::new();
    for package in packages {
        let table = package
            .as_table()
            .context("embedded Needle CUDA package is not a table")?;
        let source = table
            .get("source")
            .and_then(toml::Value::as_table)
            .context("embedded Needle CUDA package has no source")?;
        if source.get("virtual").is_some() {
            continue;
        }
        let name = table
            .get("name")
            .and_then(toml::Value::as_str)
            .context("embedded Needle CUDA package has no name")?;
        // The CUDA runtime is Linux ARM64; this is the lock's sole Windows-only package.
        if name == "colorama" {
            continue;
        }
        let version = table
            .get("version")
            .and_then(toml::Value::as_str)
            .context("embedded Needle CUDA package has no version")?;
        ensure!(
            expected
                .insert(normalize_python_distribution_name(name), version.to_owned())
                .is_none(),
            "embedded Needle CUDA lock has duplicate package {name}"
        );
    }
    Ok(expected)
}

fn normalize_python_distribution_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    normalized
}

fn needle_validation_script(project: &NeedleUvProject, device: NeedleDevice) -> Result<String> {
    let expected_versions = serde_json::to_string(&[
        ("numpy", "1.26.4"),
        ("safetensors", "0.5.3"),
        ("sentencepiece", "0.2.1"),
        ("torch", project.torch_version),
        ("transformers", "5.2.0"),
    ])?;
    let device_check = match device {
        NeedleDevice::Cpu => "assert device == 'cpu'".to_owned(),
        NeedleDevice::Cuda => {
            let cuda_version = project
                .cuda_version
                .context("Needle CUDA device selected with a non-CUDA runtime")?;
            let min_capability = project
                .min_cuda_compute_capability
                .context("Needle CUDA runtime has no minimum compute capability")?;
            format!(
                "assert device == 'cuda'; assert torch.cuda.is_available(), 'frozen Needle CUDA runtime cannot access a CUDA device'; assert torch.version.cuda == {cuda_version:?}, f'expected CUDA {cuda_version}, got {{torch.version.cuda}}'; capability=torch.cuda.get_device_capability(0); assert capability >= {min_capability:?}, f'Needle CUDA runtime requires compute capability >= {major}.{minor}, got {{capability}}'",
                major = min_capability.0,
                minor = min_capability.1,
            )
        }
    };
    Ok(format!(
        "import importlib,importlib.metadata as m,os,sys; assert sys.version_info[:2] == (3,11), sys.version; expected=dict({expected_versions}); mismatched=[f'{{name}}={{m.version(name)}} (expected {{version}})' for name,version in expected.items() if m.version(name) != version]; assert not mismatched, '; '.join(mismatched); modules={{name:importlib.import_module(name) for name in ['numpy','safetensors','sentencepiece','torch','transformers']}}; torch=modules['torch']; device={:?}; {device_check}; assert os.environ.get('HF_HUB_OFFLINE') == '1'; assert os.environ.get('TRANSFORMERS_OFFLINE') == '1'; print('__MAYHEM_NEEDLE_RUNTIME__=' + m.version('torch') + '/' + device)",
        needle_device_name(device)
    ))
}

fn validate_needle_python(
    python: &Path,
    project: &NeedleUvProject,
    device: NeedleDevice,
    cache_root: &Path,
) -> Result<()> {
    let script = needle_validation_script(project, device)?;
    const MARKER: &str = "__MAYHEM_NEEDLE_RUNTIME__=";
    let mut command = Command::new(python);
    configure_validation_cache(&mut command, cache_root)?;
    configure_offline_validation(&mut command);
    let output = command
        .arg("-c")
        .arg(&script)
        .output()
        .with_context(|| format!("starting {}", python.display()))?;
    if !output.status.success() {
        let detail = command_output_detail(&output);
        bail!(
            "{} could not validate the frozen Needle {}/{} runtime{}",
            python.display(),
            project.name,
            needle_device_name(device),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    ensure!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim().starts_with(MARKER)),
        "{} validated Needle but did not emit its runtime marker",
        python.display()
    );
    Ok(())
}

fn write_needle_runtime_marker(venv: &Path, runtime_sha256: &str) -> Result<()> {
    let marker = venv.join(".mayhem-needle-runtime-sha256");
    let bytes = format!("{runtime_sha256}\n");
    let expected = format!("{:x}", Sha256::digest(bytes.as_bytes()));
    materialize_embedded_regular_file(
        &marker,
        bytes.as_bytes(),
        &expected,
        "Needle runtime marker",
    )
}

fn cleanup_incomplete_chatterbox_venv(venv: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(venv) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading incomplete Chatterbox venv {}", venv.display()));
        }
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "refusing to remove non-directory or symlink Chatterbox venv {}",
        venv.display()
    );
    fs::remove_dir_all(venv)
        .with_context(|| format!("removing incomplete Chatterbox venv {}", venv.display()))
}

fn chatterbox_uv_project(flavor: ChatterboxRuntimeFlavor) -> ChatterboxUvProject {
    match flavor {
        ChatterboxRuntimeFlavor::CpuArm64 => ChatterboxUvProject {
            name: "cpu-arm64",
            project: CHATTERBOX_CPU_PROJECT,
            project_sha256: CHATTERBOX_CPU_PROJECT_SHA256,
            lock: CHATTERBOX_CPU_LOCK,
            lock_sha256: CHATTERBOX_CPU_LOCK_SHA256,
            torch_version: "2.6.0",
            torchaudio_version: "2.6.0",
            cuda_version: None,
        },
        ChatterboxRuntimeFlavor::CpuX86 => ChatterboxUvProject {
            name: "cpu-x86",
            project: CHATTERBOX_CPU_X86_PROJECT,
            project_sha256: CHATTERBOX_CPU_X86_PROJECT_SHA256,
            lock: CHATTERBOX_CPU_X86_LOCK,
            lock_sha256: CHATTERBOX_CPU_X86_LOCK_SHA256,
            torch_version: "2.6.0+cpu",
            torchaudio_version: "2.6.0+cpu",
            cuda_version: None,
        },
        ChatterboxRuntimeFlavor::Cuda124 => ChatterboxUvProject {
            name: "cuda124",
            project: CHATTERBOX_CUDA124_PROJECT,
            project_sha256: CHATTERBOX_CUDA124_PROJECT_SHA256,
            lock: CHATTERBOX_CUDA124_LOCK,
            lock_sha256: CHATTERBOX_CUDA124_LOCK_SHA256,
            torch_version: "2.6.0+cu124",
            torchaudio_version: "2.6.0+cu124",
            cuda_version: Some("12.4"),
        },
        ChatterboxRuntimeFlavor::Cuda130Arm64 => ChatterboxUvProject {
            name: "cuda130-arm64",
            project: CHATTERBOX_CUDA130_ARM64_PROJECT,
            project_sha256: CHATTERBOX_CUDA130_ARM64_PROJECT_SHA256,
            lock: CHATTERBOX_CUDA130_ARM64_LOCK,
            lock_sha256: CHATTERBOX_CUDA130_ARM64_LOCK_SHA256,
            torch_version: "2.9.1+cu130",
            torchaudio_version: "2.9.1",
            cuda_version: Some("13.0"),
        },
    }
}

fn select_chatterbox_runtime_flavor(
    requested: Option<&str>,
    cuda_available: bool,
    target_os: &str,
    target_arch: &str,
) -> Result<ChatterboxRuntimeFlavor> {
    let requested = requested.unwrap_or("auto").trim().to_ascii_lowercase();
    ensure!(
        matches!(requested.as_str(), "auto" | "cpu" | "cuda" | "mps"),
        "MAYHEM_CHATTERBOX_DEVICE must be auto, cpu, cuda, or mps"
    );
    match (target_os, target_arch) {
        ("macos", "aarch64") => {
            ensure!(
                requested != "cuda",
                "Chatterbox CUDA 12.4 is not supported on macOS; use auto, mps, or cpu"
            );
            Ok(ChatterboxRuntimeFlavor::CpuArm64)
        }
        ("linux", "aarch64") => match requested.as_str() {
            "mps" => bail!("Chatterbox MPS is supported only on Apple Silicon"),
            "cuda" => {
                ensure!(
                    cuda_available,
                    "MAYHEM_CHATTERBOX_DEVICE=cuda was requested but no usable NVIDIA CUDA device was detected"
                );
                Ok(ChatterboxRuntimeFlavor::Cuda130Arm64)
            }
            "cpu" => Ok(ChatterboxRuntimeFlavor::CpuArm64),
            "auto" if cuda_available => Ok(ChatterboxRuntimeFlavor::Cuda130Arm64),
            "auto" => Ok(ChatterboxRuntimeFlavor::CpuArm64),
            _ => unreachable!(),
        },
        ("linux" | "windows", "x86_64") => match requested.as_str() {
            "mps" => bail!("Chatterbox MPS is supported only on Apple Silicon"),
            "cuda" => {
                ensure!(
                    cuda_available,
                    "MAYHEM_CHATTERBOX_DEVICE=cuda was requested but no usable NVIDIA CUDA device was detected"
                );
                Ok(ChatterboxRuntimeFlavor::Cuda124)
            }
            "cpu" => Ok(ChatterboxRuntimeFlavor::CpuX86),
            "auto" if cuda_available => Ok(ChatterboxRuntimeFlavor::Cuda124),
            "auto" => Ok(ChatterboxRuntimeFlavor::CpuX86),
            _ => unreachable!(),
        },
        ("macos", "x86_64") => bail!(
            "Chatterbox 0.1.7 requires Torch 2.6.0, which has no Python 3.11 Intel-macOS wheel"
        ),
        _ => bail!("the frozen Chatterbox runtime does not support {target_os}/{target_arch}"),
    }
}

fn chatterbox_cuda_available() -> bool {
    if !matches!(
        (env::consts::OS, env::consts::ARCH),
        ("linux", "aarch64" | "x86_64") | ("windows", "x86_64")
    ) {
        return false;
    }
    Command::new("nvidia-smi")
        .args([
            "--query-gpu=driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .is_ok_and(|output| {
            output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        })
}

fn verify_chatterbox_uv_project(project: &ChatterboxUvProject) -> Result<String> {
    let project_actual = format!("{:x}", Sha256::digest(project.project));
    ensure!(
        project_actual == project.project_sha256,
        "embedded Chatterbox {} pyproject checksum mismatch: expected {}, got {}",
        project.name,
        project.project_sha256,
        project_actual
    );
    let lock_actual = format!("{:x}", Sha256::digest(project.lock));
    ensure!(
        lock_actual == project.lock_sha256,
        "embedded Chatterbox {} uv.lock checksum mismatch: expected {}, got {}",
        project.name,
        project.lock_sha256,
        lock_actual
    );

    let lock_text = std::str::from_utf8(project.lock)
        .with_context(|| format!("embedded Chatterbox {} uv.lock is not UTF-8", project.name))?;
    let lock: toml::Value = toml::from_str(lock_text)
        .with_context(|| format!("parsing embedded Chatterbox {} uv.lock", project.name))?;
    ensure!(
        lock.get("requires-python").and_then(toml::Value::as_str) == Some("==3.11.*"),
        "embedded Chatterbox {} uv.lock is not pinned to Python 3.11",
        project.name
    );
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .context("embedded Chatterbox uv.lock has no package array")?;
    let mut chatterbox_ok = false;
    let mut perth_ok = false;
    let expected_perth_source = format!(
        "https://github.com/resemble-ai/Perth.git?rev={}#{}",
        mayhem_engine::CHATTERBOX_PERTH_COMMIT,
        mayhem_engine::CHATTERBOX_PERTH_COMMIT
    );
    for package in packages {
        let table = package
            .as_table()
            .context("embedded Chatterbox uv.lock package is not a table")?;
        let name = table
            .get("name")
            .and_then(toml::Value::as_str)
            .context("embedded Chatterbox uv.lock package has no name")?;
        let version = table.get("version").and_then(toml::Value::as_str);
        if name == "chatterbox-tts" {
            chatterbox_ok = version == Some("0.1.7");
        }
        let source = table
            .get("source")
            .and_then(toml::Value::as_table)
            .context("embedded Chatterbox uv.lock package has no source")?;
        if let Some(registry) = source.get("registry").and_then(toml::Value::as_str) {
            ensure!(
                registry.starts_with("https://"),
                "embedded Chatterbox {} lock uses a non-HTTPS registry for {}",
                project.name,
                name
            );
            verify_locked_registry_artifacts(project.name, name, table)?;
        } else if let Some(git) = source.get("git").and_then(toml::Value::as_str) {
            ensure!(
                name == "resemble-perth" && git == expected_perth_source,
                "embedded Chatterbox {} lock contains unexpected Git source {} for {}",
                project.name,
                git,
                name
            );
            perth_ok = version == Some("1.0.1");
        } else {
            ensure!(
                source.get("virtual").is_some(),
                "embedded Chatterbox {} lock contains unsupported source for {}",
                project.name,
                name
            );
        }
    }
    ensure!(
        chatterbox_ok,
        "embedded Chatterbox {} lock does not contain chatterbox-tts==0.1.7",
        project.name
    );
    ensure!(
        perth_ok,
        "embedded Chatterbox {} lock does not contain Perth 1.0.1 at commit {}",
        project.name,
        mayhem_engine::CHATTERBOX_PERTH_COMMIT
    );

    let mut runtime = Sha256::new();
    runtime.update(b"mayhem/chatterbox/runtime/v1\0");
    runtime.update(project.name.as_bytes());
    runtime.update(b"\0");
    runtime.update(project.project_sha256.as_bytes());
    runtime.update(b"\0");
    runtime.update(project.lock_sha256.as_bytes());
    runtime.update(b"\0");
    runtime.update(mayhem_engine::CHATTERBOX_SOURCE_COMMIT.as_bytes());
    runtime.update(b"\0");
    runtime.update(mayhem_engine::CHATTERBOX_PERTH_COMMIT.as_bytes());
    runtime.update(b"\0");
    runtime.update(CHATTERBOX_SOURCE_TREE_SHA256.as_bytes());
    Ok(format!("{:x}", runtime.finalize()))
}

fn verify_locked_registry_artifacts(
    project_name: &str,
    package_name: &str,
    package: &toml::map::Map<String, toml::Value>,
) -> Result<()> {
    let mut count = 0_usize;
    if let Some(sdist) = package.get("sdist") {
        verify_locked_artifact_hash(project_name, package_name, sdist)?;
        count += 1;
    }
    if let Some(wheels) = package.get("wheels").and_then(toml::Value::as_array) {
        for wheel in wheels {
            verify_locked_artifact_hash(project_name, package_name, wheel)?;
            count += 1;
        }
    }
    ensure!(
        count > 0,
        "embedded Chatterbox {} lock has no hashed artifacts for registry package {}",
        project_name,
        package_name
    );
    Ok(())
}

fn verify_locked_artifact_hash(
    project_name: &str,
    package_name: &str,
    artifact: &toml::Value,
) -> Result<()> {
    let hash = artifact
        .get("hash")
        .and_then(toml::Value::as_str)
        .with_context(|| {
            format!(
                "embedded Chatterbox {} lock has an unhashed artifact for {}",
                project_name, package_name
            )
        })?;
    let digest = hash
        .strip_prefix("sha256:")
        .context("locked artifact hash is not SHA-256")?;
    ensure!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "embedded Chatterbox {} lock has an invalid artifact hash for {}",
        project_name,
        package_name
    );
    Ok(())
}

struct TemporaryChatterboxProject {
    path: PathBuf,
}

impl TemporaryChatterboxProject {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryChatterboxProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn temporary_chatterbox_install_project(
    project: &ChatterboxUvProject,
    cache_root: &Path,
) -> Result<TemporaryChatterboxProject> {
    let projects = cache_root.join("uv-projects");
    fs::create_dir_all(&projects)
        .with_context(|| format!("creating Chatterbox uv project root {}", projects.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = projects.join(format!(
        ".chatterbox-{}.{}.{}.tmp",
        project.name,
        std::process::id(),
        nonce
    ));
    fs::create_dir(&path)
        .with_context(|| format!("creating disposable Chatterbox project {}", path.display()))?;
    let result = (|| {
        materialize_embedded_regular_file(
            &path.join("pyproject.toml"),
            project.project,
            project.project_sha256,
            "Chatterbox pyproject",
        )?;
        materialize_embedded_regular_file(
            &path.join("uv.lock"),
            project.lock,
            project.lock_sha256,
            "Chatterbox uv.lock",
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(TemporaryChatterboxProject { path })
}

fn validate_managed_chatterbox_python(
    python: &Path,
    venv: &Path,
    project: &ChatterboxUvProject,
    cache_root: &Path,
    runtime_sha256: &str,
    home: &Path,
) -> Result<()> {
    let marker = venv.join(".mayhem-chatterbox-runtime-sha256");
    let metadata = fs::symlink_metadata(&marker)
        .with_context(|| format!("reading Chatterbox runtime marker {}", marker.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Chatterbox runtime marker {} is not a regular file",
        marker.display()
    );
    let actual = fs::read_to_string(&marker)
        .with_context(|| format!("reading Chatterbox runtime marker {}", marker.display()))?;
    ensure!(
        actual.trim() == runtime_sha256,
        "managed Chatterbox {} runtime identity changed",
        project.name
    );
    check_chatterbox_environment(home, venv, project, cache_root)?;
    validate_chatterbox_python(python, project, cache_root)
}

fn check_chatterbox_environment(
    home: &Path,
    venv: &Path,
    project: &ChatterboxUvProject,
    cache_root: &Path,
) -> Result<()> {
    let uv = ensure_managed_uv(home)?;
    let install_project = temporary_chatterbox_install_project(project, cache_root)?;
    let uv_cache = cache_root.join("uv");
    fs::create_dir_all(&uv_cache)
        .with_context(|| format!("creating uv cache directory {}", uv_cache.display()))?;
    let check = Command::new(&uv)
        .arg("sync")
        .arg("--check")
        .arg("--frozen")
        .arg("--no-dev")
        .arg("--no-install-project")
        .arg("--project")
        .arg(install_project.path())
        .env("UV_PROJECT_ENVIRONMENT", venv)
        .env("UV_CACHE_DIR", &uv_cache)
        .env("UV_NO_PROGRESS", "1")
        .env("UV_OFFLINE", "1")
        .output()
        .with_context(|| {
            format!(
                "checking Chatterbox {} environment with {}",
                project.name,
                uv.display()
            )
        })?;
    if !check.status.success() {
        let detail = command_output_detail(&check);
        bail!(
            "Chatterbox {} environment is not synchronized with its frozen uv.lock{}",
            project.name,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    Ok(())
}

fn validate_chatterbox_python(
    python: &Path,
    project: &ChatterboxUvProject,
    cache_root: &Path,
) -> Result<()> {
    let expected_versions = serde_json::to_string(&[
        ("chatterbox-tts", "0.1.7"),
        ("conformer", "0.3.2"),
        ("diffusers", "0.29.0"),
        ("gradio", "6.8.0"),
        ("librosa", "0.11.0"),
        ("numpy", "1.26.4"),
        ("omegaconf", "2.3.0"),
        ("pykakasi", "2.3.0"),
        ("pyloudnorm", "0.2.0"),
        ("resemble-perth", "1.0.1"),
        ("s3tokenizer", "0.3.0"),
        ("safetensors", "0.5.3"),
        ("spacy-pkuseg", "1.0.1"),
        ("torch", project.torch_version),
        ("torchaudio", project.torchaudio_version),
        ("transformers", "5.2.0"),
    ])?;
    let imports = serde_json::to_string(&[
        "chatterbox",
        "chatterbox.tts",
        "torch",
        "torchaudio",
        "librosa",
        "s3tokenizer",
        "transformers",
        "diffusers",
        "perth",
        "conformer",
        "safetensors",
        "spacy_pkuseg",
        "pykakasi",
        "pyloudnorm",
        "omegaconf",
    ])?;
    let source_commit = serde_json::to_string(mayhem_engine::CHATTERBOX_SOURCE_COMMIT)?;
    let source_tree_sha256 = serde_json::to_string(CHATTERBOX_SOURCE_TREE_SHA256)?;
    let perth_commit = serde_json::to_string(mayhem_engine::CHATTERBOX_PERTH_COMMIT)?;
    let cuda_version = project
        .cuda_version
        .map(serde_json::to_string)
        .transpose()?
        .unwrap_or_else(|| "None".to_owned());
    let script = format!(
        "import hashlib,importlib,importlib.metadata as m,json,pathlib,sys; assert sys.version_info[:2] == (3,11), sys.version; expected=dict({expected_versions}); mismatched=[f'{{name}}={{m.version(name)}} (expected {{version}})' for name,version in expected.items() if m.version(name) != version]; assert not mismatched, '; '.join(mismatched); modules={{name:importlib.import_module(name) for name in {imports}}}; perth_raw=m.distribution('resemble-perth').read_text('direct_url.json'); assert perth_raw is not None and json.loads(perth_raw).get('vcs_info',{{}}).get('commit_id') == {perth_commit}, 'Perth VCS commit mismatch'; source_root=pathlib.Path(modules['chatterbox'].__file__).resolve().parent; source_base=source_root.parent; source_rows=''.join(f'{{hashlib.sha256(path.read_bytes()).hexdigest()}}  {{path.relative_to(source_base).as_posix()}}\\n' for path in sorted(source_root.rglob('*.py'),key=lambda path:path.relative_to(source_base).as_posix())); assert hashlib.sha256(source_rows.encode()).hexdigest() == {source_tree_sha256}, 'installed Chatterbox source tree does not match reviewed upstream commit ' + {source_commit}; expected_cuda={cuda_version}; assert expected_cuda is None or modules['torch'].cuda.is_available(), 'frozen CUDA runtime installed but CUDA is unavailable'; assert expected_cuda is None or modules['torch'].version.cuda == expected_cuda, f'frozen CUDA runtime mismatch: {{modules[\"torch\"].version.cuda}} (expected {{expected_cuda}})'; print('__MAYHEM_CHATTERBOX_RUNTIME__=' + m.version('chatterbox-tts'))"
    );
    let mut command = Command::new(python);
    configure_validation_cache(&mut command, cache_root)?;
    configure_offline_validation(&mut command);
    let output = command
        .arg("-c")
        .arg(script)
        .output()
        .with_context(|| format!("starting {}", python.display()))?;
    if !output.status.success() {
        let detail = command_output_detail(&output);
        bail!(
            "{} could not validate the frozen Chatterbox {} runtime{}",
            python.display(),
            project.name,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    ensure!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == "__MAYHEM_CHATTERBOX_RUNTIME__=0.1.7"),
        "{} validated Chatterbox but did not emit its runtime marker",
        python.display()
    );
    Ok(())
}

fn write_chatterbox_runtime_marker(venv: &Path, runtime_sha256: &str) -> Result<()> {
    let marker = venv.join(".mayhem-chatterbox-runtime-sha256");
    let bytes = format!("{runtime_sha256}\n");
    let expected = format!("{:x}", Sha256::digest(bytes.as_bytes()));
    materialize_embedded_regular_file(
        &marker,
        bytes.as_bytes(),
        &expected,
        "Chatterbox runtime marker",
    )
}

fn validate_explicit_chatterbox_python_path(home: &Path, python: &Path) -> Result<PathBuf> {
    let original = fs::symlink_metadata(python)
        .with_context(|| format!("reading explicit Chatterbox Python {}", python.display()))?;
    ensure!(
        original.is_file() && !original.file_type().is_symlink(),
        "MAYHEM_CHATTERBOX_PYTHON must name a regular, non-symlink file"
    );
    let python = fs::canonicalize(python).with_context(|| {
        format!(
            "canonicalizing explicit Chatterbox Python {}",
            python.display()
        )
    })?;
    let mut roots = Vec::new();
    for root in [
        Some(home.to_path_buf()),
        env::var_os("HOME").map(PathBuf::from),
        env::var_os("USERPROFILE").map(PathBuf::from),
        env::var_os("LOCALAPPDATA").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(root) = fs::canonicalize(root) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let root = most_specific_user_controlled_root(&python, &roots)
        .context(
            "MAYHEM_CHATTERBOX_PYTHON must live under the Mayhem home or current user's profile; system-owned runtimes are not accepted",
        )?;
    verify_user_controlled_path(root, &python)?;
    let venv = explicit_chatterbox_venv_root(&python)?;
    validate_explicit_chatterbox_pyvenv(&venv, &roots)?;
    let parent = python
        .parent()
        .context("explicit Chatterbox Python has no parent directory")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = parent.join(format!(
        ".mayhem-chatterbox-access-{}-{nonce}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .with_context(|| {
            format!(
                "MAYHEM_CHATTERBOX_PYTHON parent {} is not user-writable; use the managed runtime instead",
                parent.display()
            )
        })?;
    drop(file);
    fs::remove_file(&probe)
        .with_context(|| format!("removing Chatterbox access probe {}", probe.display()))?;
    Ok(python)
}

fn explicit_chatterbox_venv_root(python: &Path) -> Result<PathBuf> {
    let scripts = python
        .parent()
        .context("explicit Chatterbox Python has no scripts directory")?;
    let venv = scripts
        .parent()
        .context("explicit Chatterbox Python has no virtual-environment root")?;
    let marker = venv.join("pyvenv.cfg");
    let metadata = fs::symlink_metadata(&marker).with_context(|| {
        format!(
            "MAYHEM_CHATTERBOX_PYTHON must belong to a virtual environment with {}",
            marker.display()
        )
    })?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && !metadata_is_windows_reparse_point(&metadata),
        "explicit Chatterbox virtual-environment marker {} is not a regular file",
        marker.display()
    );
    Ok(venv.to_path_buf())
}

fn validate_explicit_chatterbox_pyvenv(venv: &Path, roots: &[PathBuf]) -> Result<()> {
    let marker = venv.join("pyvenv.cfg");
    let text = fs::read_to_string(&marker).with_context(|| {
        format!(
            "reading explicit Chatterbox venv marker {}",
            marker.display()
        )
    })?;
    let mut base_home = None;
    let mut system_site_packages = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        match name.trim() {
            "home" => base_home = Some(PathBuf::from(value.trim())),
            "include-system-site-packages" => {
                system_site_packages = Some(value.trim().eq_ignore_ascii_case("true"));
            }
            _ => {}
        }
    }
    ensure!(
        system_site_packages != Some(true),
        "explicit Chatterbox virtual environment must not include system site-packages"
    );
    let base_home = base_home
        .context("explicit Chatterbox pyvenv.cfg does not declare its base Python home")?;
    ensure!(
        base_home.is_absolute(),
        "explicit Chatterbox base Python home must be absolute"
    );
    let base_home = fs::canonicalize(&base_home).with_context(|| {
        format!(
            "canonicalizing explicit Chatterbox base Python home {}",
            base_home.display()
        )
    })?;
    let root = most_specific_user_controlled_root(&base_home, roots).with_context(|| {
        format!(
            "explicit Chatterbox venv points to system-owned base Python {}; use the managed runtime instead",
            base_home.display()
        )
    })?;
    verify_user_controlled_path(root, &base_home)
}

fn most_specific_user_controlled_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a Path> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .map(PathBuf::as_path)
}

fn verify_user_controlled_path(root: &Path, path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        let metadata = fs::symlink_metadata(candidate)
            .with_context(|| format!("reading explicit runtime path {}", candidate.display()))?;
        ensure!(
            !metadata.file_type().is_symlink() && !metadata_is_windows_reparse_point(&metadata),
            "explicit Chatterbox runtime path contains a symlink or reparse point at {}",
            candidate.display()
        );
        if candidate == root {
            return Ok(());
        }
        current = candidate.parent();
    }
    bail!(
        "explicit Chatterbox runtime {} escaped its user-controlled root {}",
        path.display(),
        root.display()
    )
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagedUvArchiveKind {
    TarGz,
    Zip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManagedUvAsset {
    archive_kind: ManagedUvArchiveKind,
    archive_name: Cow<'static, str>,
    archive_sha256: Cow<'static, str>,
    executable_member: Cow<'static, str>,
    executable_sha256: Cow<'static, str>,
    url: Cow<'static, str>,
}

fn managed_uv_asset(target_os: &str, target_arch: &str) -> Result<ManagedUvAsset> {
    let (target, archive_kind, archive_sha256, executable_sha256) = match (target_os, target_arch) {
        ("macos", "aarch64") => (
            "aarch64-apple-darwin",
            ManagedUvArchiveKind::TarGz,
            "61c04acc52a33ef0f331e494bdfbedcdb6c26c6970c022ed3699e5860f8930e3",
            "3ac242bb6bca0841cad65b877e21a3e9f65c97141712b5c8438cc8a8c89ead54",
        ),
        ("macos", "x86_64") => (
            "x86_64-apple-darwin",
            ManagedUvArchiveKind::TarGz,
            "c4c4de482da9ccdd076dc4fb5cfe7b740609029385c72f58606be3153602387d",
            "3fcfeb23eb951da9c2db2ebdde52b7f83cafe3bf90f1d6225519b6d0db43c04b",
        ),
        ("linux", "aarch64") => (
            "aarch64-unknown-linux-gnu",
            ManagedUvArchiveKind::TarGz,
            "94500fb064ae3c971a873cba64d94694c50677e0a4dbf78735c80509e7429919",
            "40de8760ec3d368ae7a19d06392a071b761dddbe1b926d23f736ce65befe131c",
        ),
        ("linux", "x86_64") => (
            "x86_64-unknown-linux-gnu",
            ManagedUvArchiveKind::TarGz,
            "04f8b82f5d47f0512dcd32c67a4a6f16a0ea27c81537c338fd0ad6b23cebe829",
            "4f26786f798cce6e9f467fe917d4305b9600ef8bf14994aa016fbb32523e5ca5",
        ),
        ("windows", "aarch64") => (
            "aarch64-pc-windows-msvc",
            ManagedUvArchiveKind::Zip,
            "55b597ae81bc29531a7c352a1431a8a73cc2755d7a5b9ec454580cbe02e5154f",
            "bbafdd69166bdc7038b7362c0aacd44fc5a25a5e505bb7a86bdde388590197b2",
        ),
        ("windows", "x86_64") => (
            "x86_64-pc-windows-msvc",
            ManagedUvArchiveKind::Zip,
            "a047d55651bc3e0ca24595b25ec4cfcb10f9dca9fb56514e661269b37d4fae68",
            "6d40479cd1d0d5db7fc0fe68ad703fc8acbd84bba50d864bb97461f6af9d9561",
        ),
        _ => bail!(
            "the pinned uv {} standalone bootstrap does not support {target_os}/{target_arch}",
            MANAGED_UV_VERSION
        ),
    };
    let extension = match archive_kind {
        ManagedUvArchiveKind::TarGz => "tar.gz",
        ManagedUvArchiveKind::Zip => "zip",
    };
    let archive_name = format!("uv-{target}.{extension}");
    let executable_member = match archive_kind {
        ManagedUvArchiveKind::TarGz => format!("uv-{target}/uv"),
        ManagedUvArchiveKind::Zip => "uv.exe".to_owned(),
    };
    Ok(ManagedUvAsset {
        archive_kind,
        archive_sha256: Cow::Borrowed(archive_sha256),
        executable_member: Cow::Owned(executable_member),
        executable_sha256: Cow::Borrowed(executable_sha256),
        url: Cow::Owned(format!(
            "https://github.com/astral-sh/uv/releases/download/{MANAGED_UV_VERSION}/{archive_name}"
        )),
        archive_name: Cow::Owned(archive_name),
    })
}

fn ensure_managed_uv(home: &Path) -> Result<PathBuf> {
    let asset = managed_uv_asset(env::consts::OS, env::consts::ARCH)?;
    let tools = home.join("tools");
    fs::create_dir_all(&tools)
        .with_context(|| format!("creating managed tools directory {}", tools.display()))?;
    let installation = tools.join(format!("uv-{MANAGED_UV_VERSION}"));
    let uv = managed_uv_executable(&installation);
    if validate_managed_uv_install(&uv, &asset).is_ok() {
        return Ok(uv);
    }

    let lock_path = tools.join(format!(".uv-{MANAGED_UV_VERSION}.bootstrap.lock"));
    let lock = open_lock_file(&lock_path)?;
    lock.lock_exclusive()
        .with_context(|| format!("locking managed uv bootstrap {}", lock_path.display()))?;
    if validate_managed_uv_install(&uv, &asset).is_ok() {
        return Ok(uv);
    }
    if installation.exists() {
        let metadata = fs::symlink_metadata(&installation)
            .with_context(|| format!("reading managed uv path {}", installation.display()))?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "refusing to replace non-directory managed uv path {}",
            installation.display()
        );
        fs::remove_dir_all(&installation).with_context(|| {
            format!(
                "removing incomplete managed uv installation {}",
                installation.display()
            )
        })?;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    let staging = tools.join(format!(
        ".uv-{MANAGED_UV_VERSION}.{}-{nonce}.tmp",
        std::process::id()
    ));
    fs::create_dir(&staging)
        .with_context(|| format!("creating managed uv staging area {}", staging.display()))?;
    let result = (|| {
        let archive = staging.join(asset.archive_name.as_ref());
        download_managed_uv_archive(&asset, &archive)?;
        extract_managed_uv_executable(&asset, &archive, &staging)?;
        let staged_uv = managed_uv_executable(&staging);
        validate_managed_uv_install(&staged_uv, &asset)?;
        fs::remove_file(&archive)
            .with_context(|| format!("removing verified uv archive {}", archive.display()))?;
        fs::rename(&staging, &installation).with_context(|| {
            format!(
                "activating managed uv installation {}",
                installation.display()
            )
        })?;
        validate_managed_uv_install(&uv, &asset)?;
        Ok(uv.clone())
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn managed_uv_executable(installation: &Path) -> PathBuf {
    if cfg!(windows) {
        installation.join("Scripts").join("uv.exe")
    } else {
        installation.join("bin").join("uv")
    }
}

fn download_managed_uv_archive(asset: &ManagedUvAsset, destination: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(Duration::from_secs(300))
        .user_agent(format!("openmayhem/managed-uv-{MANAGED_UV_VERSION}"))
        .build()
        .context("building the managed uv bootstrap client")?;
    let mut response = client
        .get(asset.url.as_ref())
        .send()
        .with_context(|| format!("downloading pinned uv from {}", asset.url))?
        .error_for_status()
        .with_context(|| format!("downloading pinned uv from {}", asset.url))?;
    if let Some(length) = response.content_length() {
        ensure!(
            length <= MANAGED_UV_MAX_ARCHIVE_BYTES,
            "pinned uv archive is unexpectedly large: {length} bytes"
        );
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .with_context(|| format!("creating {}", destination.display()))?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .context("reading the pinned uv archive response")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("pinned uv archive size overflow")?;
        ensure!(
            total <= MANAGED_UV_MAX_ARCHIVE_BYTES,
            "pinned uv archive exceeds {} bytes",
            MANAGED_UV_MAX_ARCHIVE_BYTES
        );
        digest.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("writing {}", destination.display()))?;
    }
    output
        .sync_all()
        .with_context(|| format!("syncing {}", destination.display()))?;
    let actual = format!("{:x}", digest.finalize());
    ensure!(
        actual == asset.archive_sha256,
        "pinned uv archive checksum mismatch: expected {}, got {}",
        asset.archive_sha256,
        actual
    );
    Ok(())
}

fn extract_managed_uv_executable(
    asset: &ManagedUvAsset,
    archive_path: &Path,
    installation: &Path,
) -> Result<()> {
    let executable = managed_uv_executable(installation);
    let executable_dir = executable
        .parent()
        .context("managed uv executable has no parent directory")?;
    fs::create_dir_all(executable_dir).with_context(|| {
        format!(
            "creating managed uv executable directory {}",
            executable_dir.display()
        )
    })?;
    match asset.archive_kind {
        ManagedUvArchiveKind::TarGz => {
            ensure_safe_managed_uv_member(asset.executable_member.as_ref())?;
            let archive_file = File::open(archive_path)
                .with_context(|| format!("opening {}", archive_path.display()))?;
            let mut archive = tar::Archive::new(GzDecoder::new(archive_file));
            let mut extracted = false;
            for entry in archive
                .entries()
                .with_context(|| format!("reading {}", archive_path.display()))?
            {
                let entry = entry.with_context(|| format!("reading {}", archive_path.display()))?;
                if entry
                    .path()
                    .context("reading pinned uv archive entry path")?
                    != Path::new(asset.executable_member.as_ref())
                {
                    continue;
                }
                ensure!(
                    !extracted,
                    "pinned uv archive contains duplicate executable entries"
                );
                ensure!(
                    entry.header().entry_type().is_file(),
                    "pinned uv executable entry is not a regular file"
                );
                ensure!(
                    entry.size() <= MANAGED_UV_MAX_EXECUTABLE_BYTES,
                    "pinned uv executable is unexpectedly large"
                );
                let expected_size = entry.size();
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&executable)
                    .with_context(|| format!("creating {}", executable.display()))?;
                let copied = std::io::copy(
                    &mut entry.take(MANAGED_UV_MAX_EXECUTABLE_BYTES + 1),
                    &mut output,
                )
                .with_context(|| format!("extracting {}", executable.display()))?;
                ensure!(
                    copied == expected_size,
                    "pinned uv executable extraction was incomplete: expected {expected_size} bytes, got {copied}"
                );
                output
                    .sync_all()
                    .with_context(|| format!("syncing {}", executable.display()))?;
                extracted = true;
            }
            ensure!(extracted, "pinned uv archive has no uv executable");
        }
        ManagedUvArchiveKind::Zip => extract_managed_uv_zip(
            archive_path,
            &executable,
            asset.executable_member.as_ref(),
            MANAGED_UV_MAX_EXECUTABLE_BYTES,
        )?,
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {}", executable.display()))?;
    }
    Ok(())
}

fn ensure_safe_managed_uv_member(member: &str) -> Result<()> {
    let path = Path::new(member);
    ensure!(
        !path.as_os_str().is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
        "pinned uv executable has an unsafe archive member path"
    );
    Ok(())
}

fn extract_managed_uv_zip(
    archive_path: &Path,
    executable: &Path,
    executable_member: &str,
    maximum_executable_bytes: u64,
) -> Result<()> {
    ensure_safe_managed_uv_member(executable_member)?;
    let mut archive_file =
        File::open(archive_path).with_context(|| format!("opening {}", archive_path.display()))?;
    let declared_entries = managed_uv_zip_declared_entry_count(&mut archive_file)?;
    ensure!(
        declared_entries <= MANAGED_UV_MAX_ZIP_ENTRIES,
        "pinned uv ZIP contains too many entries: {declared_entries}"
    );
    archive_file
        .seek(SeekFrom::Start(0))
        .with_context(|| format!("rewinding {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(archive_file)
        .with_context(|| format!("reading {}", archive_path.display()))?;
    ensure!(
        archive.len() == declared_entries,
        "pinned uv ZIP contains duplicate or ambiguous central-directory entries"
    );

    let expected_name = executable_member.as_bytes();
    let mut executable_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .with_context(|| format!("reading entry {index} from {}", archive_path.display()))?;
        if entry.name_raw() != expected_name {
            continue;
        }
        ensure!(
            executable_index.replace(index).is_none(),
            "pinned uv ZIP contains duplicate executable entries"
        );
        ensure!(
            entry.enclosed_name().as_deref() == Some(Path::new(executable_member)),
            "pinned uv ZIP executable has an unsafe path"
        );
        ensure!(
            !entry.encrypted(),
            "pinned uv ZIP executable must not be encrypted"
        );
        ensure!(
            entry.is_file(),
            "pinned uv ZIP executable is not a regular file"
        );
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            ensure!(
                file_type == 0 || file_type == 0o100000,
                "pinned uv ZIP executable is a symbolic link or special file"
            );
        }
        ensure!(
            matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            ),
            "pinned uv ZIP executable uses unsupported compression"
        );
        ensure!(
            entry.compressed_size() <= MANAGED_UV_MAX_ARCHIVE_BYTES,
            "pinned uv ZIP executable has an invalid compressed size"
        );
        ensure!(
            entry.size() <= maximum_executable_bytes,
            "pinned uv ZIP executable exceeds {maximum_executable_bytes} bytes"
        );
    }
    let executable_index =
        executable_index.context("pinned uv ZIP has no exact uv executable entry")?;
    let entry = archive
        .by_index(executable_index)
        .with_context(|| format!("opening uv executable from {}", archive_path.display()))?;
    let expected_size = entry.size();
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(executable)
        .with_context(|| format!("creating {}", executable.display()))?;
    let copied = std::io::copy(
        &mut entry.take(maximum_executable_bytes.saturating_add(1)),
        &mut output,
    )
    .with_context(|| format!("extracting {}", executable.display()))?;
    ensure!(
        copied == expected_size && copied <= maximum_executable_bytes,
        "pinned uv ZIP executable extraction size mismatch: expected {expected_size} bytes, got {copied}"
    );
    output
        .sync_all()
        .with_context(|| format!("syncing {}", executable.display()))?;
    Ok(())
}

fn managed_uv_zip_declared_entry_count(archive: &mut File) -> Result<usize> {
    const EOCD_SIZE: usize = 22;
    const MAX_COMMENT_SIZE: usize = u16::MAX as usize;

    let archive_size = archive
        .metadata()
        .context("reading pinned uv ZIP metadata")?
        .len();
    ensure!(
        archive_size <= MANAGED_UV_MAX_ARCHIVE_BYTES,
        "pinned uv ZIP exceeds {} bytes",
        MANAGED_UV_MAX_ARCHIVE_BYTES
    );
    ensure!(
        archive_size >= EOCD_SIZE as u64,
        "pinned uv ZIP is missing its end-of-central-directory record"
    );
    let tail_size = usize::try_from(archive_size.min((EOCD_SIZE + MAX_COMMENT_SIZE) as u64))
        .context("pinned uv ZIP tail size does not fit in memory")?;
    archive
        .seek(SeekFrom::End(-(tail_size as i64)))
        .context("seeking to the pinned uv ZIP central directory")?;
    let mut tail = vec![0_u8; tail_size];
    archive
        .read_exact(&mut tail)
        .context("reading the pinned uv ZIP central directory")?;
    let eocd_offset = (0..=tail.len() - EOCD_SIZE)
        .rev()
        .find(|offset| {
            tail[*offset..].starts_with(b"PK\x05\x06")
                && *offset
                    + EOCD_SIZE
                    + u16::from_le_bytes([tail[*offset + 20], tail[*offset + 21]]) as usize
                    == tail.len()
        })
        .context("pinned uv ZIP has no valid end-of-central-directory record")?;
    let eocd = &tail[eocd_offset..eocd_offset + EOCD_SIZE];
    let disk = u16::from_le_bytes([eocd[4], eocd[5]]);
    let directory_disk = u16::from_le_bytes([eocd[6], eocd[7]]);
    let disk_entries = u16::from_le_bytes([eocd[8], eocd[9]]);
    let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
    ensure!(
        disk == 0 && directory_disk == 0 && disk_entries == total_entries,
        "pinned uv ZIP must be a single-disk archive"
    );
    ensure!(
        total_entries != u16::MAX,
        "pinned uv ZIP64 archives are not accepted"
    );
    let directory_size = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]) as u64;
    let directory_offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as u64;
    let absolute_eocd_offset = archive_size - tail_size as u64 + eocd_offset as u64;
    ensure!(
        directory_offset.checked_add(directory_size) == Some(absolute_eocd_offset),
        "pinned uv ZIP central-directory bounds are invalid"
    );
    Ok(total_entries as usize)
}

fn validate_managed_uv_install(uv: &Path, asset: &ManagedUvAsset) -> Result<()> {
    let metadata = fs::symlink_metadata(uv).with_context(|| format!("reading {}", uv.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{} is not a regular managed uv executable",
        uv.display()
    );
    let actual = file_sha256(uv)?;
    ensure!(
        actual == asset.executable_sha256,
        "{} checksum mismatch: expected {}, got {}",
        uv.display(),
        asset.executable_sha256,
        actual
    );
    validate_uv(uv)
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
    let embedded_module = python_embedded_module_literal(spec)?;
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

fn python_embedded_module_literal(spec: &PythonRuntimeSpec) -> Result<String> {
    match spec.embedded_module {
        Some(module) => Ok(serde_json::to_string(&(module.name, module.source_sha256))?),
        None => Ok("None".to_owned()),
    }
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

fn configure_offline_validation(command: &mut Command) {
    command
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_DATASETS_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("DIFFUSERS_OFFLINE", "1")
        .env("PIP_NO_INDEX", "1")
        .env("UV_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("DO_NOT_TRACK", "1")
        .env("PYTHONNOUSERSITE", "1");
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
    fn chatterbox_uv_projects_are_frozen_hashed_and_distinct() {
        assert_eq!(CHATTERBOX_PYTHON_VERSION, "3.11");
        assert_eq!(CHATTERBOX_MIN_FREE_BYTES, 16 * GIB);
        assert_eq!(
            mayhem_engine::CHATTERBOX_SOURCE_COMMIT,
            "59bc590b3cad826e5d5987745bf6844627a21ad5"
        );
        assert_eq!(
            mayhem_engine::CHATTERBOX_PERTH_COMMIT,
            "ce86c49d029f42272c1902eccb675556b9ed2330"
        );
        let mut runtime_ids = BTreeSet::new();
        for (flavor, torch_version, torchaudio_version, cuda_version, index) in [
            (
                ChatterboxRuntimeFlavor::CpuArm64,
                "2.6.0",
                "2.6.0",
                None,
                None,
            ),
            (
                ChatterboxRuntimeFlavor::CpuX86,
                "2.6.0+cpu",
                "2.6.0+cpu",
                None,
                Some("https://download.pytorch.org/whl/cpu"),
            ),
            (
                ChatterboxRuntimeFlavor::Cuda124,
                "2.6.0+cu124",
                "2.6.0+cu124",
                Some("12.4"),
                Some("https://download.pytorch.org/whl/cu124"),
            ),
            (
                ChatterboxRuntimeFlavor::Cuda130Arm64,
                "2.9.1+cu130",
                "2.9.1",
                Some("13.0"),
                Some("https://download.pytorch.org/whl/cu130"),
            ),
        ] {
            let project = chatterbox_uv_project(flavor);
            assert_eq!(project.torch_version, torch_version);
            assert_eq!(project.torchaudio_version, torchaudio_version);
            assert_eq!(project.cuda_version, cuda_version);
            assert!(
                !project.project.contains(&b'\r') && !project.lock.contains(&b'\r'),
                "byte-hashed Chatterbox runtime inputs must be embedded with LF line endings"
            );
            let project_text = std::str::from_utf8(project.project).unwrap();
            assert!(project_text.contains("chatterbox-tts==0.1.7"));
            assert!(project_text.contains(&format!(
                "Perth.git@{}",
                mayhem_engine::CHATTERBOX_PERTH_COMMIT
            )));
            assert!(project_text.contains("requires-python = \"==3.11.*\""));
            if let Some(index) = index {
                assert!(project_text.contains(index));
            }
            if flavor == ChatterboxRuntimeFlavor::Cuda130Arm64 {
                assert!(project_text
                    .contains("\"sys_platform == 'linux' and platform_machine == 'aarch64'\""));
                assert!(project_text.contains("override-dependencies"));
                let lock_text = std::str::from_utf8(project.lock).unwrap();
                assert!(lock_text
                    .contains("torch-2.9.1%2Bcu130-cp311-cp311-manylinux_2_28_aarch64.whl"));
                assert!(lock_text.contains(
                    "sha256:fd6c7d297e21758a7fa07624f2b5bb15607ee3b1dcc52519e8e796c6d4fcf960"
                ));
                assert!(
                    lock_text.contains("torchaudio-2.9.1-cp311-cp311-manylinux_2_28_aarch64.whl")
                );
                assert!(lock_text.contains(
                    "sha256:493421d061375074ce84840ca619605f625892e16dead63ec97181ef02da3357"
                ));
            }
            let runtime_id = verify_chatterbox_uv_project(&project).unwrap();
            assert_eq!(runtime_id.len(), 64);
            assert!(runtime_ids.insert(runtime_id));
        }
    }

    #[test]
    fn chatterbox_runtime_selection_tracks_device_and_platform() {
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("auto"), true, "linux", "x86_64").unwrap(),
            ChatterboxRuntimeFlavor::Cuda124
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("auto"), false, "linux", "x86_64").unwrap(),
            ChatterboxRuntimeFlavor::CpuX86
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("cpu"), true, "windows", "x86_64").unwrap(),
            ChatterboxRuntimeFlavor::CpuX86
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("mps"), false, "macos", "aarch64").unwrap(),
            ChatterboxRuntimeFlavor::CpuArm64
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("cpu"), false, "linux", "aarch64").unwrap(),
            ChatterboxRuntimeFlavor::CpuArm64
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("auto"), true, "linux", "aarch64").unwrap(),
            ChatterboxRuntimeFlavor::Cuda130Arm64
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("auto"), false, "linux", "aarch64").unwrap(),
            ChatterboxRuntimeFlavor::CpuArm64
        );
        assert_eq!(
            select_chatterbox_runtime_flavor(Some("cuda"), true, "linux", "aarch64").unwrap(),
            ChatterboxRuntimeFlavor::Cuda130Arm64
        );
        assert!(select_chatterbox_runtime_flavor(Some("cuda"), false, "linux", "x86_64").is_err());
        assert!(select_chatterbox_runtime_flavor(Some("cuda"), false, "linux", "aarch64").is_err());
        assert!(select_chatterbox_runtime_flavor(Some("mps"), true, "linux", "aarch64").is_err());
        assert!(select_chatterbox_runtime_flavor(Some("auto"), false, "macos", "x86_64").is_err());
        assert!(select_chatterbox_runtime_flavor(Some("bogus"), false, "linux", "x86_64").is_err());
    }

    #[test]
    fn needle_uv_projects_are_frozen_hashed_and_minimal() {
        assert_eq!(NEEDLE_PYTHON_VERSION, "3.11");
        let mut runtime_ids = BTreeSet::new();
        for (flavor, torch_version, cuda_version, index) in [
            (NeedleRuntimeFlavor::CpuArm64, "2.9.1", None, None),
            (
                NeedleRuntimeFlavor::CpuX86,
                "2.9.1+cpu",
                None,
                Some("https://download.pytorch.org/whl/cpu"),
            ),
            (
                NeedleRuntimeFlavor::Cuda130Arm64,
                "2.9.1+cu130",
                Some("13.0"),
                Some("https://download.pytorch.org/whl/cu130"),
            ),
            (
                NeedleRuntimeFlavor::Cuda130X86,
                "2.9.1+cu130",
                Some("13.0"),
                Some("https://download.pytorch.org/whl/cu130"),
            ),
        ] {
            let project = needle_uv_project(flavor);
            assert_eq!(project.torch_version, torch_version);
            assert_eq!(project.cuda_version, cuda_version);
            assert!(!project.project.contains(&b'\r') && !project.lock.contains(&b'\r'));
            let project_text = std::str::from_utf8(project.project).unwrap();
            for dependency in [
                "numpy==1.26.4",
                "safetensors==0.5.3",
                "sentencepiece==0.2.1",
                "transformers==5.2.0",
            ] {
                assert!(project_text.contains(dependency));
            }
            for forbidden in [
                "accelerate",
                "cactus",
                "datasets",
                "jax",
                "peft",
                "tensorboard",
                "trl",
                "wandb",
            ] {
                assert!(!project_text.contains(forbidden));
            }
            if let Some(index) = index {
                assert!(project_text.contains(index));
            }
            let lock_text = std::str::from_utf8(project.lock).unwrap();
            match flavor {
                NeedleRuntimeFlavor::Cuda130Arm64 => {
                    assert!(lock_text
                        .contains("torch-2.9.1%2Bcu130-cp311-cp311-manylinux_2_28_aarch64.whl"));
                }
                NeedleRuntimeFlavor::Cuda130X86 => {
                    assert!(lock_text
                        .contains("torch-2.9.1%2Bcu130-cp311-cp311-manylinux_2_28_x86_64.whl"));
                    assert!(lock_text.contains("torch-2.9.1%2Bcu130-cp311-cp311-win_amd64.whl"));
                }
                NeedleRuntimeFlavor::CpuArm64 | NeedleRuntimeFlavor::CpuX86 => {}
            }
            let runtime_id = verify_needle_uv_project(&project).unwrap();
            assert_eq!(runtime_id.len(), 64);
            assert!(runtime_ids.insert(runtime_id));
        }

        let mut tampered = needle_uv_project(NeedleRuntimeFlavor::CpuArm64);
        tampered.project_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(verify_needle_uv_project(&tampered)
            .unwrap_err()
            .to_string()
            .contains("pyproject checksum mismatch"));
    }

    #[test]
    fn needle_runtime_selection_tracks_device_and_platform() {
        assert_eq!(normalize_explicit_needle_device(" CPU ").unwrap(), "cpu");
        assert_eq!(normalize_explicit_needle_device("GPU").unwrap(), "gpu");
        assert_eq!(normalize_explicit_needle_device("cuda").unwrap(), "cuda");
        assert!(normalize_explicit_needle_device("MPS").is_err());
        assert!(normalize_explicit_needle_device("auto").is_err());
        assert!(normalize_explicit_needle_device("bogus").is_err());
        assert_eq!(
            select_needle_runtime(Some("auto"), false, "macos", "aarch64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }
        );
        assert!(select_needle_runtime(Some("gpu"), false, "macos", "aarch64").is_err());
        assert_eq!(
            select_needle_runtime(Some("cpu"), false, "macos", "aarch64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("auto"), true, "linux", "aarch64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130Arm64,
                device: NeedleDevice::Cuda,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("auto"), false, "linux", "aarch64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuArm64,
                device: NeedleDevice::Cpu,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("cpu"), true, "linux", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("auto"), true, "linux", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }
        );
        assert!(select_needle_runtime(Some("cuda"), false, "linux", "aarch64").is_err());
        assert!(select_needle_runtime(Some("gpu"), false, "linux", "x86_64").is_err());
        assert_eq!(
            select_needle_runtime(Some("gpu"), true, "linux", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }
        );
        assert!(select_needle_runtime(Some("mps"), true, "linux", "aarch64").is_err());
        assert_eq!(
            select_needle_runtime(Some("cpu"), false, "windows", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::CpuX86,
                device: NeedleDevice::Cpu,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("auto"), true, "windows", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }
        );
        assert_eq!(
            select_needle_runtime(Some("gpu"), true, "windows", "x86_64").unwrap(),
            NeedleRuntimeSelection {
                flavor: NeedleRuntimeFlavor::Cuda130X86,
                device: NeedleDevice::Cuda,
            }
        );
        assert!(select_needle_runtime(Some("gpu"), false, "windows", "x86_64").is_err());
        assert!(select_needle_runtime(Some("bogus"), false, "linux", "aarch64").is_err());
        assert!(!needle_cuda_driver_supported(""));
        assert!(!needle_cuda_driver_supported("579.99"));
        assert!(!needle_cuda_driver_supported("not-a-version"));
        assert!(needle_cuda_driver_supported("580.00"));
        assert!(needle_cuda_driver_supported("590.12\n590.12"));
    }

    #[test]
    fn needle_validation_is_device_specific_and_offline() {
        let cpu = needle_validation_script(
            &needle_uv_project(NeedleRuntimeFlavor::CpuArm64),
            NeedleDevice::Cpu,
        )
        .unwrap();
        assert!(cpu.contains("device == 'cpu'"));
        assert!(!cpu.contains("get_device_capability"));

        let cuda = needle_validation_script(
            &needle_uv_project(NeedleRuntimeFlavor::Cuda130Arm64),
            NeedleDevice::Cuda,
        )
        .unwrap();
        assert!(cuda.contains("torch.cuda.is_available()"));
        assert!(cuda.contains("expected CUDA 13.0"));
        assert!(cuda.contains("get_device_capability"));

        assert!(cuda.contains("compute capability >= 7.5"));
        for script in [&cpu, &cuda] {
            assert!(script.contains("HF_HUB_OFFLINE"));
            assert!(script.contains("TRANSFORMERS_OFFLINE"));
            assert!(script.contains("sentencepiece"));
        }
    }

    #[test]
    fn needle_uv_project_materialization_is_transactional() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-needle-project-test-{}-{nonce}",
            std::process::id()
        ));
        let cache = root.join("cache");
        let project = needle_uv_project(NeedleRuntimeFlavor::CpuArm64);
        let materialized = temporary_needle_install_project(&project, &cache).unwrap();
        assert_eq!(
            file_sha256(&materialized.path().join("pyproject.toml")).unwrap(),
            project.project_sha256
        );
        assert_eq!(
            file_sha256(&materialized.path().join("uv.lock")).unwrap(),
            project.lock_sha256
        );
        let materialized_path = materialized.path().to_path_buf();
        drop(materialized);
        assert!(!materialized_path.exists());

        let partial_venv = root.join("partial-venv");
        fs::create_dir_all(partial_venv.join("nested")).unwrap();
        fs::write(partial_venv.join("nested/partial.whl"), b"partial").unwrap();
        cleanup_incomplete_needle_venv(&partial_venv).unwrap();
        assert!(!partial_venv.exists());
        cleanup_incomplete_needle_venv(&partial_venv).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_needle_runtime_is_preserved_when_health_validation_is_transiently_unavailable() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-needle-runtime-health-test-{}-{nonce}",
            std::process::id()
        ));
        let venv = root.join("venv");
        fs::create_dir_all(&venv).unwrap();
        let runtime_sha256 =
            verify_needle_uv_project(&needle_uv_project(NeedleRuntimeFlavor::CpuArm64)).unwrap();
        write_needle_runtime_marker(&venv, &runtime_sha256).unwrap();
        let sentinel = venv.join("installed-package.sentinel");
        fs::write(&sentinel, b"complete").unwrap();

        let error = validate_existing_needle_runtime(&venv, &runtime_sha256, || {
            bail!("runtime health check is temporarily unavailable")
        })
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("temporarily unhealthy; preserving it without reinstall"));
        assert!(venv.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"complete");
        assert!(venv.join(".mayhem-needle-runtime-sha256").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chatterbox_uv_project_materialization_and_failed_install_cleanup_are_transactional() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-chatterbox-project-test-{}-{nonce}",
            std::process::id()
        ));
        let cache = root.join("cache");
        let project = chatterbox_uv_project(ChatterboxRuntimeFlavor::CpuArm64);
        let materialized = temporary_chatterbox_install_project(&project, &cache).unwrap();
        assert_eq!(
            file_sha256(&materialized.path().join("pyproject.toml")).unwrap(),
            project.project_sha256
        );
        assert_eq!(
            file_sha256(&materialized.path().join("uv.lock")).unwrap(),
            project.lock_sha256
        );
        let materialized_path = materialized.path().to_path_buf();
        drop(materialized);
        assert!(!materialized_path.exists());

        let partial_venv = root.join("partial-venv");
        fs::create_dir_all(partial_venv.join("nested")).unwrap();
        fs::write(partial_venv.join("nested/partial.whl"), b"partial").unwrap();
        cleanup_incomplete_chatterbox_venv(&partial_venv).unwrap();
        assert!(!partial_venv.exists());
        cleanup_incomplete_chatterbox_venv(&partial_venv).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_chatterbox_runtime_requires_a_user_controlled_regular_path() {
        let roots = vec![
            PathBuf::from("/users/alice"),
            PathBuf::from("/users/alice/.mayhem"),
        ];
        assert_eq!(
            most_specific_user_controlled_root(
                Path::new("/users/alice/.mayhem/venvs/chatterbox/bin/python"),
                &roots,
            ),
            Some(Path::new("/users/alice/.mayhem"))
        );
        assert!(
            most_specific_user_controlled_root(Path::new("/opt/python/bin/python"), &roots)
                .is_none()
        );

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let home = env::temp_dir().join(format!(
            "mayhem-chatterbox-explicit-test-{}-{nonce}",
            std::process::id()
        ));
        let python = home.join("venv").join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        let base = home.join("managed-python/bin");
        fs::create_dir_all(&base).unwrap();
        fs::write(&python, b"not executed").unwrap();
        fs::write(
            home.join("venv/pyvenv.cfg"),
            format!(
                "home = {}\ninclude-system-site-packages = false\nversion = 3.11\n",
                base.display()
            ),
        )
        .unwrap();
        assert_eq!(
            validate_explicit_chatterbox_python_path(&home, &python).unwrap(),
            fs::canonicalize(&python).unwrap()
        );
        assert_eq!(
            explicit_chatterbox_venv_root(&python).unwrap(),
            home.join("venv")
        );
        fs::write(
            home.join("venv/pyvenv.cfg"),
            "home = /definitely/system-owned/python\ninclude-system-site-packages = false\n",
        )
        .unwrap();
        assert!(validate_explicit_chatterbox_python_path(&home, &python).is_err());
        fs::remove_file(home.join("venv/pyvenv.cfg")).unwrap();
        assert!(explicit_chatterbox_venv_root(&python).is_err());
        fs::remove_dir_all(home).unwrap();
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
    fn managed_uv_bootstrap_assets_are_versioned_and_hash_pinned() {
        for (target_os, target_arch, archive_sha256, executable_sha256) in [
            (
                "macos",
                "aarch64",
                "61c04acc52a33ef0f331e494bdfbedcdb6c26c6970c022ed3699e5860f8930e3",
                "3ac242bb6bca0841cad65b877e21a3e9f65c97141712b5c8438cc8a8c89ead54",
            ),
            (
                "macos",
                "x86_64",
                "c4c4de482da9ccdd076dc4fb5cfe7b740609029385c72f58606be3153602387d",
                "3fcfeb23eb951da9c2db2ebdde52b7f83cafe3bf90f1d6225519b6d0db43c04b",
            ),
            (
                "linux",
                "aarch64",
                "94500fb064ae3c971a873cba64d94694c50677e0a4dbf78735c80509e7429919",
                "40de8760ec3d368ae7a19d06392a071b761dddbe1b926d23f736ce65befe131c",
            ),
            (
                "linux",
                "x86_64",
                "04f8b82f5d47f0512dcd32c67a4a6f16a0ea27c81537c338fd0ad6b23cebe829",
                "4f26786f798cce6e9f467fe917d4305b9600ef8bf14994aa016fbb32523e5ca5",
            ),
            (
                "windows",
                "aarch64",
                "55b597ae81bc29531a7c352a1431a8a73cc2755d7a5b9ec454580cbe02e5154f",
                "bbafdd69166bdc7038b7362c0aacd44fc5a25a5e505bb7a86bdde388590197b2",
            ),
            (
                "windows",
                "x86_64",
                "a047d55651bc3e0ca24595b25ec4cfcb10f9dca9fb56514e661269b37d4fae68",
                "6d40479cd1d0d5db7fc0fe68ad703fc8acbd84bba50d864bb97461f6af9d9561",
            ),
        ] {
            let asset = managed_uv_asset(target_os, target_arch).unwrap();
            assert!(asset
                .url
                .starts_with("https://github.com/astral-sh/uv/releases/download/0.11.29/"));
            assert_eq!(asset.archive_sha256, archive_sha256);
            assert_eq!(asset.executable_sha256, executable_sha256);
            assert_eq!(asset.archive_sha256.len(), 64);
            assert_eq!(asset.executable_sha256.len(), 64);
        }
        assert!(managed_uv_asset("linux", "mips64").is_err());
    }

    #[test]
    fn managed_uv_bootstrap_extraction_never_invokes_python() {
        let source = include_str!("python_runtime.rs");
        let start = source.find("fn ensure_managed_uv(").unwrap();
        let end = source[start..]
            .find("fn validate_uv(")
            .map(|offset| start + offset)
            .unwrap();
        let bootstrap = &source[start..end];
        assert!(!bootstrap.contains("Command::new"));
        assert!(!bootstrap.contains("resolve_base_python"));
        assert!(!bootstrap.contains("python3"));
        assert!(!bootstrap.contains("python.exe"));
        assert!(!bootstrap.contains("ensurepip"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_uv_standalone_archive_bootstraps_without_python_ensurepip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-managed-uv-bootstrap-test-{}-{nonce}",
            std::process::id()
        ));
        let archive_path = root.join("uv-fixture.tar.gz");
        let installation = root.join("installation");
        fs::create_dir_all(&root).unwrap();
        let executable = b"#!/bin/sh\nprintf 'uv 0.11.29\\n'\n";
        let archive_file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(archive_file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(executable.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "uv-fixture/uv", executable.as_slice())
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();

        let asset = ManagedUvAsset {
            archive_kind: ManagedUvArchiveKind::TarGz,
            archive_name: Cow::Borrowed("uv-fixture.tar.gz"),
            archive_sha256: Cow::Owned(file_sha256(&archive_path).unwrap()),
            executable_member: Cow::Borrowed("uv-fixture/uv"),
            executable_sha256: Cow::Owned(format!("{:x}", Sha256::digest(executable))),
            url: Cow::Borrowed("https://example.invalid/unused"),
        };
        extract_managed_uv_executable(&asset, &archive_path, &installation).unwrap();
        validate_managed_uv_install(&managed_uv_executable(&installation), &asset).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn write_managed_uv_zip_fixture(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);
        for (name, contents) in entries {
            archive.start_file(*name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn managed_uv_windows_zip_fixture_extracts_cross_platform() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-managed-uv-zip-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("uv-windows.zip");
        let executable = root.join("extracted-uv.exe");
        let expected = b"standalone uv fixture";
        write_managed_uv_zip_fixture(
            &archive,
            &[
                ("uvw.exe", b"ignored"),
                ("uv.exe", expected),
                ("uvx.exe", b"ignored"),
            ],
        );

        extract_managed_uv_zip(&archive, &executable, "uv.exe", 1024).unwrap();
        assert_eq!(fs::read(&executable).unwrap(), expected);
        assert!(extract_managed_uv_zip(&archive, &executable, "uv.exe", 1024).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_uv_zip_rejects_unsafe_ambiguous_and_oversized_entries() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "mayhem-managed-uv-zip-negative-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();

        let duplicate = root.join("duplicate.zip");
        write_managed_uv_zip_fixture(&duplicate, &[("uva.exe", b"first"), ("uvb.exe", b"second")]);
        let mut duplicate_bytes = fs::read(&duplicate).unwrap();
        let mut replacements = 0;
        for name in [b"uva.exe".as_slice(), b"uvb.exe".as_slice()] {
            for offset in 0..=duplicate_bytes.len() - name.len() {
                if &duplicate_bytes[offset..offset + name.len()] == name {
                    duplicate_bytes[offset..offset + name.len()].copy_from_slice(b"uvx.exe");
                    replacements += 1;
                }
            }
        }
        assert_eq!(replacements, 4);
        fs::write(&duplicate, duplicate_bytes).unwrap();
        assert!(
            extract_managed_uv_zip(&duplicate, &root.join("duplicate.exe"), "uvx.exe", 1024)
                .is_err()
        );

        let traversal = root.join("traversal.zip");
        write_managed_uv_zip_fixture(&traversal, &[("../uv.exe", b"unsafe")]);
        assert!(
            extract_managed_uv_zip(&traversal, &root.join("traversal.exe"), "../uv.exe", 1024)
                .is_err()
        );

        let symlink = root.join("symlink.zip");
        let file = File::create(&symlink).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .add_symlink(
                "uv.exe",
                "elsewhere.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.finish().unwrap();
        assert!(
            extract_managed_uv_zip(&symlink, &root.join("symlink.exe"), "uv.exe", 1024).is_err()
        );

        let oversized = root.join("oversized.zip");
        write_managed_uv_zip_fixture(&oversized, &[("uv.exe", b"more-than-eight-bytes")]);
        assert!(
            extract_managed_uv_zip(&oversized, &root.join("oversized.exe"), "uv.exe", 8).is_err()
        );
        fs::remove_dir_all(root).unwrap();
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
    fn managed_runtime_validation_uses_python_none_without_an_embedded_module() {
        let vllm = python_runtime_spec("vllm").expect("vLLM runtime");
        assert_eq!(python_embedded_module_literal(&vllm).unwrap(), "None");

        let sulphur = python_runtime_spec("sulphur").expect("Sulphur runtime");
        let literal = python_embedded_module_literal(&sulphur).unwrap();
        assert!(literal.starts_with("[\"mayhem_sulphur_runtime\","));
        assert!(!literal.contains("null"));
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
