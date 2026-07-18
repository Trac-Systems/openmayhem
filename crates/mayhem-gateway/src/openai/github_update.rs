use std::{env, time::Duration};

use reqwest::{
    header::{ETAG, IF_NONE_MATCH},
    Client, StatusCode,
};
use semver::Version;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const DEFAULT_GITHUB_REPOSITORY_API_URL: &str =
    "https://api.github.com/repos/Trac-Systems/openmayhem";
const DEFAULT_SOURCE_REF: &str = "main";
const DEFAULT_CHECK_INTERVAL_SECONDS: u64 = 12 * 60 * 60;
const MIN_CHECK_INTERVAL_SECONDS: u64 = 5 * 60;
const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 4;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayGithubUpdate {
    pub kind: String,
    pub installed_version: String,
    pub available_version: Option<String>,
    pub installed_revision: Option<String>,
    pub available_revision: Option<String>,
    pub release_url: Option<String>,
    pub compare_url: Option<String>,
    pub published_at: Option<String>,
    pub installable: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayGithubUpdateStatus {
    pub state: String,
    pub installed_version: String,
    pub installed_revision: Option<String>,
    pub checked_at_seconds: Option<u64>,
    pub update: Option<GatewayGithubUpdate>,
    pub message: String,
}

impl GatewayGithubUpdateStatus {
    pub(super) fn disabled() -> Self {
        Self {
            state: "disabled".to_owned(),
            installed_version: installed_app_version().to_owned(),
            installed_revision: installed_source_revision(),
            checked_at_seconds: None,
            update: None,
            message: "Automatic GitHub update checks are disabled.".to_owned(),
        }
    }

    pub(super) fn checking() -> Self {
        Self {
            state: "checking".to_owned(),
            installed_version: installed_app_version().to_owned(),
            installed_revision: installed_source_revision(),
            checked_at_seconds: None,
            update: None,
            message: "Checking GitHub for newer Mayhem source and signed releases.".to_owned(),
        }
    }

    fn unavailable(checked_at_seconds: u64) -> Self {
        Self {
            state: "unavailable".to_owned(),
            installed_version: installed_app_version().to_owned(),
            installed_revision: installed_source_revision(),
            checked_at_seconds: Some(checked_at_seconds),
            update: None,
            message: "GitHub could not be checked. Mayhem continues normally.".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct GithubUpdateCheckConfig {
    repository_api_url: String,
    release_feed_url: String,
    source_ref: String,
    target: String,
    request_timeout: Duration,
    pub interval: Duration,
}

impl GithubUpdateCheckConfig {
    pub(super) fn from_env() -> Self {
        let repository_api_url = env::var("MAYHEM_GITHUB_REPOSITORY_API_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_GITHUB_REPOSITORY_API_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        let release_feed_url = env::var("MAYHEM_RELEASE_FEED_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("{repository_api_url}/releases/latest"));
        let source_ref = env::var("MAYHEM_GITHUB_SOURCE_REF")
            .ok()
            .filter(|value| valid_source_ref(value))
            .unwrap_or_else(|| DEFAULT_SOURCE_REF.to_owned());
        let interval_seconds = env_u64("MAYHEM_UPDATE_CHECK_INTERVAL_SECONDS")
            .unwrap_or(DEFAULT_CHECK_INTERVAL_SECONDS)
            .max(MIN_CHECK_INTERVAL_SECONDS);
        let timeout_seconds = env_u64("MAYHEM_UPDATE_CHECK_TIMEOUT_SECONDS")
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECONDS)
            .clamp(1, 30);
        Self {
            repository_api_url,
            release_feed_url,
            source_ref,
            target: release_host_target(),
            request_timeout: Duration::from_secs(timeout_seconds),
            interval: Duration::from_secs(interval_seconds),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubCompare {
    status: String,
    #[serde(default)]
    ahead_by: u64,
    html_url: String,
    #[serde(default)]
    commits: Vec<GithubCommit>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Debug, Default)]
pub(super) struct GithubUpdateCache {
    release: EndpointCache<GithubRelease>,
    compare: EndpointCache<GithubCompare>,
}

#[derive(Debug)]
struct EndpointCache<T> {
    etag: Option<String>,
    value: Option<T>,
}

impl<T> Default for EndpointCache<T> {
    fn default() -> Self {
        Self {
            etag: None,
            value: None,
        }
    }
}

pub(super) async fn check_github_update(
    config: &GithubUpdateCheckConfig,
    cache: &mut GithubUpdateCache,
    checked_at_seconds: u64,
) -> GatewayGithubUpdateStatus {
    let client = match Client::builder()
        .user_agent(format!("mayhem-update-check/{}", installed_app_version()))
        .timeout(config.request_timeout)
        .build()
    {
        Ok(client) => client,
        Err(_) => return GatewayGithubUpdateStatus::unavailable(checked_at_seconds),
    };

    let release = fetch_cached_json(&client, &config.release_feed_url, &mut cache.release).await;
    let compare = match installed_source_revision() {
        Some(revision) => {
            let url = format!(
                "{}/compare/{}...{}",
                config.repository_api_url, revision, config.source_ref
            );
            fetch_cached_json(&client, &url, &mut cache.compare).await
        }
        None => Ok(None),
    };
    if release.is_err() && compare.is_err() {
        return GatewayGithubUpdateStatus::unavailable(checked_at_seconds);
    }
    evaluate_github_update(
        installed_app_version(),
        installed_source_revision().as_deref(),
        release.ok().flatten().as_ref(),
        compare.ok().flatten().as_ref(),
        &config.target,
        checked_at_seconds,
    )
}

async fn fetch_cached_json<T>(
    client: &Client,
    url: &str,
    cache: &mut EndpointCache<T>,
) -> Result<Option<T>, String>
where
    T: Clone + DeserializeOwned,
{
    let mut request = client.get(url);
    if let Some(etag) = cache.etag.as_deref() {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(cache.value.clone());
    }
    if response.status() == StatusCode::NOT_FOUND {
        cache.etag = None;
        cache.value = None;
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let value = response
        .json::<T>()
        .await
        .map_err(|error| error.to_string())?;
    cache.etag = etag;
    cache.value = Some(value.clone());
    Ok(Some(value))
}

fn evaluate_github_update(
    installed_version: &str,
    installed_revision: Option<&str>,
    release: Option<&GithubRelease>,
    compare: Option<&GithubCompare>,
    target: &str,
    checked_at_seconds: u64,
) -> GatewayGithubUpdateStatus {
    let release_update = release.and_then(|release| {
        let installed = parse_version(installed_version)?;
        let available = parse_version(&release.tag_name)?;
        if release.draft
            || release.prerelease
            || !available.pre.is_empty()
            || available <= installed
        {
            return None;
        }
        let installable = release_has_installable_assets(release, target);
        Some(GatewayGithubUpdate {
            kind: if installable { "release" } else { "source-release" }.to_owned(),
            installed_version: installed_version.to_owned(),
            available_version: Some(release.tag_name.clone()),
            installed_revision: installed_revision.map(str::to_owned),
            available_revision: None,
            release_url: Some(release.html_url.clone()),
            compare_url: None,
            published_at: release.published_at.clone(),
            installable,
            message: if installable {
                format!(
                    "Mayhem {} is available as a signed update for this system.",
                    release.tag_name
                )
            } else {
                format!(
                    "Mayhem {} is published on GitHub, but no signed executable is available for this system yet.",
                    release.tag_name
                )
            },
        })
    });
    let source_update = compare.and_then(|compare| {
        if compare.status != "ahead" || compare.ahead_by == 0 {
            return None;
        }
        Some(GatewayGithubUpdate {
            kind: "source".to_owned(),
            installed_version: installed_version.to_owned(),
            available_version: None,
            installed_revision: installed_revision.map(str::to_owned),
            available_revision: compare.commits.last().map(|commit| commit.sha.clone()),
            release_url: None,
            compare_url: Some(compare.html_url.clone()),
            published_at: None,
            installable: false,
            message: format!(
                "{} newer source {} available on GitHub.",
                compare.ahead_by,
                if compare.ahead_by == 1 {
                    "change is"
                } else {
                    "changes are"
                }
            ),
        })
    });
    let update = release_update.or(source_update);
    GatewayGithubUpdateStatus {
        state: if update.is_some() {
            "available"
        } else {
            "current"
        }
        .to_owned(),
        installed_version: installed_version.to_owned(),
        installed_revision: installed_revision.map(str::to_owned),
        checked_at_seconds: Some(checked_at_seconds),
        message: update
            .as_ref()
            .map(|update| update.message.clone())
            .unwrap_or_else(|| {
                "This Mayhem build is current with the configured GitHub source.".to_owned()
            }),
        update,
    }
}

fn release_has_installable_assets(release: &GithubRelease, target: &str) -> bool {
    let base = format!("mayhem-{}-{target}", release.tag_name);
    let archive_tar = format!("{base}.tar.gz");
    let archive_zip = format!("{base}.zip");
    let manifest = format!("{base}.manifest.json");
    let signature = format!("{manifest}.sig");
    let has = |name: &str| release.assets.iter().any(|asset| asset.name == name);
    (has(&archive_tar) || has(&archive_zip)) && has(&manifest) && has(&signature)
}

pub(super) fn automatic_update_check_enabled() -> bool {
    !env::var("MAYHEM_UPDATE_CHECK").ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

pub(super) fn installed_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub(super) fn installed_source_revision() -> Option<String> {
    let revision = env!("MAYHEM_BUILD_GIT_SHA");
    is_git_revision(revision).then(|| revision.to_ascii_lowercase())
}

fn parse_version(value: &str) -> Option<Version> {
    Version::parse(value.trim().trim_start_matches('v')).ok()
}

fn release_host_target() -> String {
    let arch = env::consts::ARCH;
    let os = match env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    format!("{arch}-{os}")
}

fn valid_source_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn is_git_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_owned(),
            html_url: format!("https://github.test/releases/{tag}"),
            published_at: Some("2026-07-18T12:00:00Z".to_owned()),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|name| GithubReleaseAsset {
                    name: (*name).to_owned(),
                })
                .collect(),
        }
    }

    fn compare(ahead_by: u64) -> GithubCompare {
        GithubCompare {
            status: if ahead_by == 0 { "identical" } else { "ahead" }.to_owned(),
            ahead_by,
            html_url: "https://github.test/compare/current...main".to_owned(),
            commits: (0..ahead_by)
                .map(|index| GithubCommit {
                    sha: format!("{index:040x}"),
                })
                .collect(),
        }
    }

    #[test]
    fn source_changes_are_visible_without_claiming_an_installable_release() {
        let status = evaluate_github_update(
            "0.2.0",
            Some("1111111111111111111111111111111111111111"),
            None,
            Some(&compare(3)),
            "x86_64-pc-windows-msvc",
            42,
        );
        let update = status.update.expect("source update");
        assert_eq!(update.kind, "source");
        assert!(!update.installable);
        assert!(update.message.contains("3 newer source changes"));
    }

    #[test]
    fn source_only_release_is_distinct_from_a_signed_executable() {
        let status = evaluate_github_update(
            "0.2.0",
            None,
            Some(&release("0.3.0", &[])),
            None,
            "x86_64-pc-windows-msvc",
            42,
        );
        let update = status.update.expect("source release");
        assert_eq!(update.kind, "source-release");
        assert!(!update.installable);
    }

    #[test]
    fn complete_signed_assets_enable_the_existing_updater_automatically() {
        let target = "x86_64-pc-windows-msvc";
        let base = format!("mayhem-0.3.0-{target}");
        let assets = [
            format!("{base}.zip"),
            format!("{base}.manifest.json"),
            format!("{base}.manifest.json.sig"),
        ];
        let asset_refs = assets.iter().map(String::as_str).collect::<Vec<_>>();
        let status = evaluate_github_update(
            "0.2.0",
            None,
            Some(&release("0.3.0", &asset_refs)),
            None,
            target,
            42,
        );
        let update = status.update.expect("installable release");
        assert_eq!(update.kind, "release");
        assert!(update.installable);
    }

    #[test]
    fn beta_tag_does_not_downgrade_a_stable_build() {
        let status = evaluate_github_update(
            "0.2.0",
            None,
            Some(&release("0.2.0-beta", &[])),
            None,
            "x86_64-pc-windows-msvc",
            42,
        );
        assert_eq!(status.state, "current");
        assert!(status.update.is_none());
    }
}
