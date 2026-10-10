//! The same CLI acceptance can run from a signed disposable installation.
//! Never silently fall back to checkout assets when that mode is requested.
use super::*;

pub(super) fn installed_root() -> Option<PathBuf> {
    std::env::var_os("MAYHEM_SETUP_INSTALLED_ROOT").map(|path| {
        let path = PathBuf::from(path);
        assert!(
            path.is_absolute(),
            "explicit installed fixture root required"
        );
        std::fs::canonicalize(path).unwrap()
    })
}

pub(super) async fn prepare(binary: &Path, fixture: &Path) -> PathBuf {
    if let Some(install) = installed_root() {
        assert_eq!(
            std::fs::canonicalize(binary.parent().unwrap()).unwrap(),
            install.join("bin"),
            "installed fixture must execute the installed CLI"
        );
        let assets = install.join("share/mayhem");
        assert!(assets.join("intercom/contract/proxy-protocol.js").is_file());
        assert!(assets.join("RULES.md").is_file());
        return assets;
    }
    let root = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let assets = fixture.join("verified-assets");
    let output = tokio::process::Command::new("node")
        .arg(root.join("crates/mayhem-proxy/tests/setup/run_assets.mjs"))
        .arg(root)
        .arg(&assets)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "isolated candidate assets: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assets
}

pub(super) fn configure(command: &mut tokio::process::Command, assets: &Path, home: &Path) {
    // An empty disposable cwd prevents repository-relative discovery. Installed
    // binaries must discover their own sibling share directory without a hint.
    command.current_dir(home);
    if installed_root().is_some() {
        command.env_remove("MAYHEM_ASSET_DIR");
    } else {
        command.env("MAYHEM_ASSET_DIR", assets);
    }
}
