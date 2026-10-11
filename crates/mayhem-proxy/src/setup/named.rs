//! One path rule for all local setup frontends. Existing bundles stay in place.
use super::*;
use std::{fs, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name(String);

impl Default for Name {
    fn default() -> Self {
        Self("default".into())
    }
}
impl FromStr for Name {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if !(1..=32).contains(&value.len())
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error::Name);
        }
        Ok(Self(value.into()))
    }
}
impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Protection),
    }
}

/// A wizard at the root makes that root the default bundle. No files move.
pub fn directory(home: &Path, name: &Name) -> Result<PathBuf> {
    require(home.is_absolute())?;
    let root = home.join("proxy-setup");
    if exists(&root)? {
        protected_directory(&root)?;
    }
    if name == &Name::default() && exists(&root.join("wizard.json"))? {
        Ok(root)
    } else {
        let destination = root.join(name.as_str());
        if exists(&destination)? {
            protected_directory(&destination)?;
        }
        Ok(destination)
    }
}

/// Create only the private collection directory, never an existing bundle.
pub fn prepare_root(home: &Path) -> Result<()> {
    protected_directory(home)?;
    let root = home.join("proxy-setup");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {
                fs::File::open(home)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::Storage)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(Error::Protection),
        }
    }
    #[cfg(windows)]
    if !exists(&root)? {
        use mayhem_windows_sandbox::{LeafName, NtfsDirectory};
        let parent = NtfsDirectory::open_existing(home).map_err(store::windows_error)?;
        let mut guard = parent.into_lock().map_err(store::windows_error)?;
        if !exists(&root)? {
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
            let temporary =
                LeafName::new(&format!(".proxy-root-{}", blake3::hash(&nonce).to_hex()))
                    .map_err(store::windows_error)?;
            let mut staged = guard
                .stage_directory(temporary, &[], 0)
                .map_err(store::windows_error)?;
            staged
                .publish(LeafName::new("proxy-setup").map_err(store::windows_error)?)
                .map_err(store::windows_error)?;
        }
    }
    protected_directory(&root)
}

fn protected_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| Error::Protection)?;
    require(meta.is_dir() && !meta.file_type().is_symlink())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(meta.mode() & 0o077 == 0 && meta.uid() == rustix::process::geteuid().as_raw())?;
    }
    #[cfg(windows)]
    mayhem_windows_sandbox::NtfsDirectory::open_existing(path).map_err(store::windows_error)?;
    Ok(())
}

#[derive(Serialize)]
pub struct Summary {
    pub name: String,
    pub lane: &'static str,
    pub model: String,
    pub market: serde_json::Value,
    pub publication: String,
    /// Retained local state, never a claim that a process is currently healthy.
    pub run: String,
}
#[derive(Serialize)]
pub struct Page {
    pub setups: Vec<Summary>,
    pub next: Option<String>,
}

fn read_optional<T: serde::de::DeserializeOwned>(path: &Path, max: usize) -> Result<Option<T>> {
    if !exists(path)? {
        return Ok(None);
    }
    let bytes = private_file(path, max).map_err(|_| Error::Protection)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| Error::Invalid)
}

fn summary(home: &Path, name: &Name) -> Result<Summary> {
    let bundle = directory(home, name)?;
    protected_directory(&bundle)?;
    // Parsing configuration does not load credentials, initialize locks or run
    // the wizard. Atomic file replacement yields complete retained snapshots.
    let config: FlowConfig =
        read_optional(&bundle.join("wizard.json"), MAX_BYTES)?.ok_or(Error::Missing)?;
    let state = if config.directory.is_absolute() {
        config.directory.clone()
    } else {
        bundle.join(&config.directory)
    };
    protected_directory(&state)?;
    let draft: Option<Record> = read_optional(&state.join("draft.json"), MAX_DRAFT_BYTES)?;
    let (model, market, publication) = match draft.as_ref() {
        Some(record) => {
            let review = record.review()?;
            (
                record.input.adapter.upstream_model.clone(),
                serde_json::json!(review.market),
                review.publication_status.into(),
            )
        }
        None => (
            config.profile.upstream_model,
            serde_json::json!(config.profile.market),
            "not_submitted".into(),
        ),
    };
    let run = super::run::retained_state(&state)?;
    Ok(Summary {
        name: name.to_string(),
        lane: "proxied",
        model,
        market,
        publication,
        run,
    })
}

/// Read at most one page of bundle contents and retain only limit + 1 names.
pub fn list(home: &Path, after: Option<&Name>, limit: usize) -> Result<Page> {
    require((1..=64).contains(&limit))?;
    let root = home.join("proxy-setup");
    let mut names = BTreeSet::new();
    if !exists(&root)? {
        return Ok(Page {
            setups: vec![],
            next: None,
        });
    }
    protected_directory(&root)?;
    let legacy = exists(&root.join("wizard.json"))?;
    let mut insert = |name: Name| {
        if after.is_none_or(|after| &name > after) {
            names.insert(name);
            if names.len() > limit + 1 {
                names.pop_last();
            }
        }
    };
    if legacy {
        insert(Name::default());
    }
    for entry in fs::read_dir(&root).map_err(|_| Error::Protection)? {
        let entry = entry.map_err(|_| Error::Protection)?;
        let Some(name) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<Name>().ok())
        else {
            continue;
        };
        if legacy && name == Name::default() {
            continue;
        }
        if entry.file_type().map_err(|_| Error::Protection)?.is_dir()
            && exists(&entry.path().join("wizard.json"))?
        {
            insert(name);
        }
    }
    let more = names.len() > limit;
    let setups = names
        .into_iter()
        .take(limit)
        .map(|name| summary(home, &name))
        .collect::<Result<Vec<_>>>()?;
    let next = more.then(|| setups.last().expect("nonempty page").name.clone());
    Ok(Page { setups, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_have_exact_ascii_bounds() {
        for value in ["default", "a", "0", "-", "model-123", &"a".repeat(32)] {
            assert_eq!(value.parse::<Name>().unwrap().as_str(), value);
        }
        for value in [
            "",
            "A",
            "a_b",
            "a/b",
            "../a",
            ".",
            "..",
            "a\\b",
            " a",
            "a ",
            "a\n",
            "ä",
            &"a".repeat(33),
        ] {
            assert!(value.parse::<Name>().is_err(), "{value:?}");
        }
    }
    #[cfg(unix)]
    #[test]
    fn collection_and_bundle_symlinks_are_rejected_and_missing_list_writes_nothing() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(list(home.path(), None, 1).unwrap().setups.is_empty());
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
        let target = tempfile::tempdir().unwrap();
        symlink(target.path(), home.path().join("proxy-setup")).unwrap();
        assert!(directory(home.path(), &Name::default()).is_err());
        assert!(prepare_root(home.path()).is_err());
        assert!(list(home.path(), None, 1).is_err());
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        prepare_root(home.path()).unwrap();
        symlink(target.path(), home.path().join("proxy-setup/alpha")).unwrap();
        assert!(directory(home.path(), &"alpha".parse().unwrap()).is_err());
    }
}
