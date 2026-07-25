use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use serde_json::Value;
use tokio::process::Command;

const WALLET_VERSION: &str = "1.0.1";
const SOURCE_OWNED_DEPENDENCY_DIRS: &[&str] =
    &["trac/msb/node_modules", "trac/trac-peer/node_modules"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Readiness {
    pub(crate) repaired: bool,
}

pub(crate) async fn ensure_ready(intercom_dir: &Path) -> Result<Readiness> {
    let intercom_dir = canonical_directory(intercom_dir, "Intercom runtime root")?;
    let asset_root = intercom_dir
        .parent()
        .context("Intercom runtime root has no asset parent")?;
    let verifier = asset_root
        .join("scripts")
        .join("verify-intercom-dependency-topology.mjs");

    if !verifier.is_file() {
        validate_runtime_topology(&intercom_dir)?;
        return Ok(Readiness { repaired: false });
    }

    if run_verifier(&verifier, &intercom_dir)
        .await?
        .status
        .success()
    {
        validate_runtime_topology(&intercom_dir)?;
        return Ok(Readiness { repaired: false });
    }

    let lock_dir = asset_root.join(".mayhem-local").join("locks");
    fs::create_dir_all(&lock_dir).with_context(|| {
        format!(
            "creating Intercom hydration lock directory {}",
            lock_dir.display()
        )
    })?;
    let lock_path = lock_dir.join("intercom-runtime-hydration.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening Intercom hydration lock {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("locking Intercom hydration at {}", lock_path.display()))?;

    let result = async {
        if run_verifier(&verifier, &intercom_dir)
            .await?
            .status
            .success()
        {
            validate_runtime_topology(&intercom_dir)?;
            return Ok(Readiness { repaired: false });
        }

        repair_source_runtime(asset_root, &intercom_dir, &verifier).await?;
        validate_runtime_topology(&intercom_dir)?;
        Ok(Readiness { repaired: true })
    }
    .await;

    let _ = FileExt::unlock(&lock);
    result
}

async fn repair_source_runtime(
    asset_root: &Path,
    intercom_dir: &Path,
    verifier: &Path,
) -> Result<()> {
    for required in [
        asset_root.join("Cargo.toml"),
        intercom_dir.join("package.json"),
        intercom_dir.join("package-lock.json"),
        intercom_dir.join(".npmrc"),
        intercom_dir
            .join("scripts")
            .join("materialize-local-dependencies.mjs"),
        verifier.to_path_buf(),
    ] {
        ensure_regular_file(&required)?;
    }

    ensure_safe_directory_if_present(&intercom_dir.join("node_modules"))?;
    remove_source_owned_dependency_trees(intercom_dir)?;

    run_checked(
        npm_program(),
        ["ci", "--omit=dev", "--install-links=true"],
        intercom_dir,
        "installing the root-authoritative Intercom dependency lock",
    )
    .await?;

    let materializer = intercom_dir
        .join("scripts")
        .join("materialize-local-dependencies.mjs");
    run_checked_paths(
        node_program(),
        [&materializer, intercom_dir],
        intercom_dir,
        "materializing pinned Intercom local packages",
    )
    .await?;

    let verified = run_verifier(verifier, intercom_dir).await?;
    if !verified.status.success() {
        bail!(
            "Intercom dependency repair completed but topology verification failed: {}",
            output_detail(&verified)
        );
    }
    Ok(())
}

fn validate_runtime_topology(intercom_dir: &Path) -> Result<()> {
    for relative in SOURCE_OWNED_DEPENDENCY_DIRS {
        let nested = intercom_dir.join(relative);
        if fs::symlink_metadata(&nested).is_ok() {
            bail!(
                "Intercom pinned source retains a generated dependency tree at {}; rerun `mayhem up` from a writable source checkout or reinstall the exact release",
                nested.display()
            );
        }
    }

    let node_modules = canonical_directory(
        &intercom_dir.join("node_modules"),
        "Intercom root node_modules",
    )?;
    for package in ["trac-msb", "trac-peer", "trac-wallet"] {
        canonical_directory(
            &node_modules.join(package),
            &format!("Intercom root package {package}"),
        )?;
    }

    let wallet_root = node_modules.join("trac-wallet");
    let wallet_manifest_path = wallet_root.join("package.json");
    ensure_regular_file(&wallet_manifest_path)?;
    let wallet_manifest: Value = serde_json::from_slice(
        &fs::read(&wallet_manifest_path)
            .with_context(|| format!("reading {}", wallet_manifest_path.display()))?,
    )
    .with_context(|| format!("parsing {}", wallet_manifest_path.display()))?;
    ensure!(
        wallet_manifest.get("version").and_then(Value::as_str) == Some(WALLET_VERSION),
        "Intercom root trac-wallet must be exact version {WALLET_VERSION}"
    );

    let wallet_entry = wallet_root.join("index.js");
    ensure_regular_file(&wallet_entry)?;
    let wallet_source = fs::read_to_string(&wallet_entry)
        .with_context(|| format!("reading {}", wallet_entry.display()))?;
    ensure!(
        wallet_source.contains("encodeBech32mSafe"),
        "Intercom root trac-wallet {WALLET_VERSION} does not expose encodeBech32mSafe"
    );

    let peer_wallet_shadow = node_modules
        .join("trac-peer")
        .join("node_modules")
        .join("trac-wallet");
    ensure!(
        fs::symlink_metadata(&peer_wallet_shadow).is_err(),
        "Intercom trac-peer contains a nested trac-wallet that shadows the root-authoritative runtime"
    );
    Ok(())
}

fn remove_source_owned_dependency_trees(intercom_dir: &Path) -> Result<()> {
    for relative in SOURCE_OWNED_DEPENDENCY_DIRS {
        let nested = intercom_dir.join(relative);
        let Ok(metadata) = fs::symlink_metadata(&nested) else {
            continue;
        };
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "refusing to remove non-directory or linked generated dependency path {}",
            nested.display()
        );
        fs::remove_dir_all(&nested).with_context(|| {
            format!(
                "removing stale generated dependency tree {}",
                nested.display()
            )
        })?;
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {label} at {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{label} must be a real directory: {}",
        path.display()
    );
    fs::canonicalize(path).with_context(|| format!("resolving {label} at {}", path.display()))
}

fn ensure_safe_directory_if_present(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Intercom dependency directory must be a real directory: {}",
        path.display()
    );
    Ok(())
}

fn ensure_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting required Intercom file {}", path.display()))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "required Intercom path must be a regular non-link file: {}",
        path.display()
    );
    Ok(())
}

async fn run_verifier(verifier: &Path, intercom_dir: &Path) -> Result<Output> {
    let mut command = Command::new(node_program());
    command
        .arg(verifier)
        .arg(intercom_dir)
        .current_dir(intercom_dir);
    command.output().await.with_context(|| {
        format!(
            "running Intercom dependency topology verifier {}",
            verifier.display()
        )
    })
}

async fn run_checked<const N: usize>(
    program: &str,
    args: [&str; N],
    cwd: &Path,
    action: &str,
) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("{action}: starting {program}"))?;
    if !output.status.success() {
        bail!("{action} failed: {}", output_detail(&output));
    }
    Ok(())
}

async fn run_checked_paths<const N: usize>(
    program: &str,
    args: [&Path; N],
    cwd: &Path,
    action: &str,
) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("{action}: starting {program}"))?;
    if !output.status.success() {
        bail!("{action} failed: {}", output_detail(&output));
    }
    Ok(())
}

fn output_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    let mut chars = detail.chars();
    let shortened = chars.by_ref().take(4096).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else if shortened.is_empty() {
        output.status.to_string()
    } else {
        shortened
    }
}

#[cfg(windows)]
fn npm_program() -> &'static str {
    "npm.cmd"
}

#[cfg(not(windows))]
fn npm_program() -> &'static str {
    "npm"
}

fn node_program() -> &'static str {
    "node"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mayhem-intercom-runtime-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let intercom = root.join("intercom");
        for package in ["trac-msb", "trac-peer", "trac-wallet"] {
            fs::create_dir_all(intercom.join("node_modules").join(package)).unwrap();
        }
        fs::write(
            intercom.join("node_modules/trac-wallet/package.json"),
            r#"{"name":"trac-wallet","version":"1.0.1"}"#,
        )
        .unwrap();
        fs::write(
            intercom.join("node_modules/trac-wallet/index.js"),
            "export default class PeerWallet { static encodeBech32mSafe() {} }\n",
        )
        .unwrap();
        intercom
    }

    #[test]
    fn accepts_one_root_authoritative_wallet() {
        let intercom = fixture();
        validate_runtime_topology(&intercom).unwrap();
        fs::remove_dir_all(intercom.parent().unwrap()).unwrap();
    }

    #[test]
    fn rejects_source_owned_dependency_shadow() {
        let intercom = fixture();
        fs::create_dir_all(intercom.join("trac/trac-peer/node_modules/trac-wallet")).unwrap();
        let error = validate_runtime_topology(&intercom)
            .unwrap_err()
            .to_string();
        assert!(error.contains("generated dependency tree"), "{error}");
        fs::remove_dir_all(intercom.parent().unwrap()).unwrap();
    }

    #[test]
    fn removes_only_real_source_owned_dependency_directories() {
        let intercom = fixture();
        let nested = intercom.join("trac/msb/node_modules/trac-wallet");
        fs::create_dir_all(&nested).unwrap();
        remove_source_owned_dependency_trees(&intercom).unwrap();
        assert!(!intercom.join("trac/msb/node_modules").exists());
        assert!(intercom.join("node_modules/trac-wallet").is_dir());
        fs::remove_dir_all(intercom.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_source_owned_dependency_path() {
        use std::os::unix::fs::symlink;

        let intercom = fixture();
        let outside = intercom.parent().unwrap().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(intercom.join("trac/trac-peer")).unwrap();
        symlink(&outside, intercom.join("trac/trac-peer/node_modules")).unwrap();
        let error = remove_source_owned_dependency_trees(&intercom)
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing to remove"), "{error}");
        assert!(outside.is_dir());
        fs::remove_dir_all(intercom.parent().unwrap()).unwrap();
    }
}
