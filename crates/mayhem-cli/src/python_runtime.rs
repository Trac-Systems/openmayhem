use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    required_imports: &'static [&'static str],
    version: &'static str,
    requirements: &'static [u8],
    requirements_sha256: &'static str,
    extra_index_urls: &'static [&'static str],
    min_free_bytes: u64,
}

pub(crate) fn ensure_backend_python(home: &Path, backend: &str) -> Result<PythonRuntime> {
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
            let _ = fs::remove_dir_all(&venv);
            bail!(
                "installing pinned {}=={} for {} failed with {}; check network access and the backend OS prerequisites, then retry",
                spec.distribution,
                spec.version,
                spec.backend,
                install.status
            );
        }
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
        }),
        "mlx" => Some(PythonRuntimeSpec {
            backend: "mlx",
            override_env: "MAYHEM_MLX_PYTHON",
            distribution: "mlx-lm",
            required_imports: &["mlx_lm", "mlx", "transformers", "tokenizers", "safetensors"],
            version: "0.31.3",
            requirements: MLX_REQUIREMENTS,
            requirements_sha256: "d3167fca548be3265d62c6397f6ded6a688017b3d37de1ed4eed5eabf16b9747",
            extra_index_urls: &[],
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
    let pairs = exact_requirement_pairs(text).with_context(|| {
        format!(
            "embedded {} requirements must contain only exact name==version pins",
            spec.backend
        )
    })?;
    if !pairs
        .iter()
        .any(|(distribution, version)| distribution == spec.distribution && version == spec.version)
    {
        bail!(
            "embedded {} requirements must include {}",
            spec.backend,
            expected
        );
    }
    Ok(())
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
        let normalized = distribution.to_ascii_lowercase().replace('_', "-");
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
    let expected_versions = serde_json::to_string(&exact_requirement_pairs(text)?)?;
    let required_imports = serde_json::to_string(spec.required_imports)?;
    const VERSION_MARKER: &str = "__MAYHEM_RUNTIME_VERSION__=";
    let script = format!(
        "import importlib, importlib.metadata as m; expected=dict({expected_versions}); mismatched=[f'{{name}}={{m.version(name)}} (expected {{version}})' for name,version in expected.items() if m.version(name) != version]; assert not mismatched, '; '.join(mismatched); [importlib.import_module(name) for name in {required_imports}]; print({:?} + m.version({:?}))",
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
    }

    #[test]
    fn requirements_reject_ranges_options_and_duplicates() {
        assert!(exact_requirement_pairs("mlx-lm>=0.31.3\n").is_err());
        assert!(exact_requirement_pairs("--extra-index-url https://example.invalid\n").is_err());
        assert!(exact_requirement_pairs("mlx-lm==0.31.3\nmlx_lm==0.31.3\n").is_err());
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
    fn only_trt_uses_the_cuda_pytorch_index() {
        assert!(python_runtime_spec("vllm")
            .unwrap()
            .extra_index_urls
            .is_empty());
        assert!(python_runtime_spec("mlx")
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
