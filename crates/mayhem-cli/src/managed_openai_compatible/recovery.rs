use super::*;

/// Recover an abandoned owned engine before taking the hardware snapshot used
/// for cold-start admission. Normal runtime preparation is too late: its old
/// engine can itself consume the RAM that prevents candidate selection.
/// A live owner keeps its lock, and must never be interrupted by this path.
pub(crate) fn recover_abandoned_runtime(
    home: &Path,
    provider_id: &str,
    enclave_id: &str,
    docker: impl FnOnce() -> Result<PathBuf>,
) -> Result<bool> {
    let root = home
        .join("runtime/managed/openai-compatible")
        .join(safe_component(enclave_id));
    let state_path = root.join("state.json");
    let metadata = match fs::symlink_metadata(&state_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("reading managed recovery state metadata"),
    };
    ensure!(
        metadata.file_type().is_file() && metadata.len() <= 64 * 1024,
        "managed recovery state must be a bounded regular file"
    );
    ensure!(
        root.canonicalize()?.starts_with(home.canonicalize()?),
        "managed recovery directory escapes the provider home"
    );
    let lock_path = root.join("lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
        ensure!(
            metadata.file_type().is_file(),
            "managed recovery lock is not a regular file"
        );
    }
    let lock = private_open(&lock_path)?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
        Err(error) => return Err(error).context("acquiring managed recovery ownership"),
    }
    // Read after locking: the previous owner may have completed cleanup while
    // this process was acquiring the lock. Never infer ownership from a name.
    let file = match File::open(&state_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("opening managed recovery state"),
    };
    let state: ManagedContainerState = serde_json::from_reader(file.take(64 * 1024))
        .context("parsing managed recovery identity")?;
    let home_hash = sha256_bytes(home.as_os_str().as_encoded_bytes());
    for (name, expected) in [
        ("mayhem.managed", "true"),
        ("mayhem.owner-home", home_hash.as_str()),
        ("mayhem.provider", provider_id),
        ("mayhem.enclave", enclave_id),
        ("mayhem.nonce", state.nonce.as_str()),
    ] {
        ensure!(
            state.labels.get(name).map(String::as_str) == Some(expected),
            "managed recovery ownership mismatch for {name}"
        );
    }
    ensure!(
        is_hex(&state.nonce, 32, 32),
        "invalid managed recovery nonce"
    );
    // Existing reconciliation checks schema, exact Docker ID and labels before
    // stopping anything, and retains state on failure. The lock stays held.
    reconcile_state_path(&docker()?, &state_path)?;
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        home: PathBuf,
        root: PathBuf,
        docker: PathBuf,
        state: ManagedContainerState,
    }

    impl Fixture {
        fn new() -> Self {
            let home =
                std::env::temp_dir().join(format!("mayhem-recovery-{}", random_hex(8).unwrap()));
            let root = home.join("runtime/managed/openai-compatible/enclave");
            fs::create_dir_all(&root).unwrap();
            let nonce = "ab".repeat(16);
            let state = ManagedContainerState {
                schema_version: 1,
                nonce: nonce.clone(),
                container_name: "mayhem-owned".to_owned(),
                container_id: Some("cd".repeat(32)),
                labels: BTreeMap::from([
                    ("mayhem.managed".into(), "true".into()),
                    (
                        "mayhem.owner-home".into(),
                        sha256_bytes(home.as_os_str().as_encoded_bytes()),
                    ),
                    ("mayhem.provider".into(), "provider".into()),
                    ("mayhem.enclave".into(), "enclave".into()),
                    ("mayhem.nonce".into(), nonce),
                ]),
            };
            atomic_write_private_json(&root.join("state.json"), &state).unwrap();
            fs::write(
                home.join("id"),
                format!("{}\n", state.container_id.as_ref().unwrap()),
            )
            .unwrap();
            fs::write(
                home.join("labels"),
                serde_json::to_vec(&state.labels).unwrap(),
            )
            .unwrap();
            let docker = home.join("docker");
            fs::write(
                &docker,
                r#"#!/bin/sh
cd "$(dirname "$0")" || exit 99
printf '%s\n' "$1" >> calls
case "$1" in
 inspect)
  if [ -f missing ]; then echo 'No such container' >&2; exit 1; fi
  if [ "$2" = '--type' ]; then cat id; fi
  cat labels
  ;;
 stop) [ ! -f fail-stop ] ;;
 rm) touch removed ;;
 *) exit 98 ;;
esac
"#,
            )
            .unwrap();
            fs::set_permissions(&docker, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                home,
                root,
                docker,
                state,
            }
        }

        fn recover(&self) -> Result<bool> {
            recover_abandoned_runtime(&self.home, "provider", "enclave", || {
                Ok(self.docker.clone())
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    #[test]
    fn recovery_removes_only_verified_abandoned_engine_and_is_idempotent() {
        let f = Fixture::new();
        assert!(f.recover().unwrap());
        assert!(f.home.join("removed").exists());
        assert!(!f.root.join("state.json").exists());
        let calls = fs::read_to_string(f.home.join("calls")).unwrap();
        assert_eq!(calls, "inspect\nstop\ninspect\nrm\n");
        assert!(!f.recover().unwrap());
        assert_eq!(fs::read_to_string(f.home.join("calls")).unwrap(), calls);
    }

    #[test]
    fn recovery_leaves_live_owner_and_other_enclaves_untouched() {
        let f = Fixture::new();
        let lock = private_open(&f.root.join("lock")).unwrap();
        lock.try_lock_exclusive().unwrap();
        assert!(!f.recover().unwrap());
        assert!(
            !recover_abandoned_runtime(&f.home, "provider", "other", || panic!("no Docker needed"))
                .unwrap()
        );
        assert!(f.root.join("state.json").exists());
        assert!(!f.home.join("calls").exists());
    }

    #[test]
    fn recovery_rejects_foreign_identity_without_contacting_docker() {
        let mut f = Fixture::new();
        for key in [
            "mayhem.provider",
            "mayhem.owner-home",
            "mayhem.enclave",
            "mayhem.managed",
            "mayhem.nonce",
        ] {
            let prior = f.state.labels.insert(key.into(), "foreign".into()).unwrap();
            atomic_write_private_json(&f.root.join("state.json"), &f.state).unwrap();
            assert!(f.recover().is_err(), "{key}");
            assert!(!f.home.join("calls").exists());
            f.state.labels.insert(key.into(), prior);
        }
    }

    #[test]
    fn recovery_refuses_changed_container_and_retains_failed_cleanup() {
        let f = Fixture::new();
        fs::write(f.home.join("id"), format!("{}\n", "ef".repeat(32))).unwrap();
        assert!(f.recover().is_err());
        assert_eq!(
            fs::read_to_string(f.home.join("calls")).unwrap(),
            "inspect\n"
        );
        fs::write(
            f.home.join("id"),
            format!("{}\n", f.state.container_id.as_ref().unwrap()),
        )
        .unwrap();
        fs::write(f.home.join("fail-stop"), b"").unwrap();
        assert!(f.recover().is_err());
        assert!(f.root.join("state.json").exists());
        assert!(!f.home.join("removed").exists());
    }

    #[test]
    fn recovery_refuses_changed_docker_labels_before_stopping() {
        let f = Fixture::new();
        let mut actual = f.state.labels.clone();
        actual.insert("mayhem.provider".into(), "another-provider".into());
        fs::write(f.home.join("labels"), serde_json::to_vec(&actual).unwrap()).unwrap();
        assert!(f.recover().is_err());
        assert_eq!(fs::read_to_string(f.home.join("calls")).unwrap(), "inspect\n");
        assert!(f.root.join("state.json").exists());
    }

    #[test]
    fn recovery_clears_missing_container_state_and_rejects_symlinks() {
        let f = Fixture::new();
        fs::write(f.home.join("missing"), b"").unwrap();
        assert!(f.recover().unwrap());
        assert!(!f.home.join("removed").exists());
        std::os::unix::fs::symlink(f.home.join("labels"), f.root.join("state.json")).unwrap();
        assert!(f.recover().is_err());
    }
}
