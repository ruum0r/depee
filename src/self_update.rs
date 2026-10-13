//! Checking GitHub Releases for a newer depee and running its installer.

use std::{fs, path::PathBuf, time::Duration};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache;

const REPO: &str = "ruum0r/depee";
const CHECK_INTERVAL_HOURS: i64 = 24;

#[derive(Serialize, Deserialize)]
struct UpdateCheckCache {
    checked_at: DateTime<Utc>,
    latest_version: Option<String>,
    #[serde(default)]
    skipped_version: Option<String>,
}

/// Whether update checks are enabled in `~/.config/depee/config.toml`.
pub fn enabled() -> bool {
    dirs::home_dir()
        .map(|home| home.join(".config/depee/config.toml"))
        .and_then(|path| fs::read_to_string(path).ok())
        .is_none_or(|contents| check_for_updates(&contents))
}

/// Reads `check_for_updates` from a minimal TOML subset; the last assignment
/// wins and a value other than `true`/`false` keeps checks enabled.
fn check_for_updates(config: &str) -> bool {
    config
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            let (key, value) = line.split_once('=')?;
            (key.trim() == "check_for_updates").then(|| value.trim())
        })
        .next_back()
        .and_then(|value| match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
        .unwrap_or(true)
}

/// A newer release found by an earlier check that the user has not skipped.
pub fn pending() -> Option<String> {
    read_cache().as_ref().and_then(pending_update)
}

fn pending_update(cache: &UpdateCheckCache) -> Option<String> {
    cache
        .latest_version
        .clone()
        .filter(|version| is_newer(version))
        .filter(|version| cache.skipped_version.as_ref() != Some(version))
}

/// Fetches the latest release into the cache. The result is shown on the next launch.
pub async fn refresh_cache() -> Result<()> {
    if let Some(cache) = read_cache() {
        let age = Utc::now().signed_duration_since(cache.checked_at);
        if age.num_seconds() >= 0 && age.num_hours() < CHECK_INTERVAL_HOURS {
            return Ok(());
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("depee/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(3))
        .build()?;
    let release: Value = client
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let latest_version = release
        .get("tag_name")
        .and_then(Value::as_str)
        .and_then(release_version);

    // Re-read so a version skipped while the request was in flight is kept.
    let skipped_version = read_cache().and_then(|cache| cache.skipped_version);
    write_cache(&UpdateCheckCache {
        checked_at: Utc::now(),
        latest_version,
        skipped_version,
    });
    Ok(())
}

pub fn skip(version: &str) {
    if let Some(mut cache) = read_cache() {
        cache.skipped_version = Some(version.to_string());
        write_cache(&cache);
    }
}

fn cache_path() -> Option<PathBuf> {
    cache::dir().map(|dir| dir.join(".update-check.json"))
}

fn read_cache() -> Option<UpdateCheckCache> {
    let bytes = fs::read(cache_path()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(cache: &UpdateCheckCache) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = fs::write(path, bytes);
    }
}

/// Only `v`-prefixed semver tags are releases; the tag ruleset protects that namespace.
fn release_version(tag: &str) -> Option<String> {
    let version = tag.strip_prefix('v')?;
    semver::Version::parse(version).ok()?;
    Some(version.to_string())
}

fn is_newer(version: &str) -> bool {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"));
    let latest = semver::Version::parse(version);
    matches!((current, latest), (Ok(current), Ok(latest)) if latest > current)
}

/// Pinned to the release tag (protected by a ruleset) rather than a branch, so
/// updates only run an installer that shipped with that release.
fn install_script_url(version: &str) -> String {
    format!("https://raw.githubusercontent.com/{REPO}/v{version}/install.sh")
}

/// Replaces this process with the installer so the running binary can be overwritten.
#[cfg(unix)]
pub fn install(version: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("Could not locate the depee executable")?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("Could not determine the install directory"))?;
    println!("Updating depee to v{version}…");
    // The URL is passed as an argument so it is never parsed as shell code.
    let error = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"curl -fsSL "$1" | sh && echo 'Restart depee to use the new version.'"#)
        .arg("sh")
        .arg(install_script_url(version))
        .env("DEPEE_INSTALL_DIR", install_dir)
        .env("DEPEE_VERSION", version)
        .exec();
    Err(error).context("Could not run the installer")
}

#[cfg(not(unix))]
pub fn install(version: &str) -> Result<()> {
    println!(
        "Install the update with: curl -fsSL {} | DEPEE_VERSION={version} sh",
        install_script_url(version)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_update_respects_version_and_skip() {
        let cache = |latest: &str, skipped: Option<&str>| UpdateCheckCache {
            checked_at: Utc::now(),
            latest_version: Some(latest.into()),
            skipped_version: skipped.map(Into::into),
        };
        assert_eq!(
            pending_update(&cache("99.0.0", None)).as_deref(),
            Some("99.0.0")
        );
        assert_eq!(pending_update(&cache("99.0.0", Some("99.0.0"))), None);
        assert_eq!(
            pending_update(&cache("99.1.0", Some("99.0.0"))).as_deref(),
            Some("99.1.0")
        );
        assert_eq!(pending_update(&cache("0.0.1", None)), None);
    }

    #[test]
    fn release_version_requires_v_prefixed_semver_tag() {
        assert_eq!(release_version("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(
            release_version("v1.2.3-rc.1").as_deref(),
            Some("1.2.3-rc.1")
        );
        assert_eq!(release_version("1.2.3"), None);
        assert_eq!(release_version("vv1.2.3"), None);
        assert_eq!(release_version("v1.2"), None);
    }

    #[test]
    fn install_script_url_is_pinned_to_tag() {
        assert_eq!(
            install_script_url("1.2.3"),
            "https://raw.githubusercontent.com/ruum0r/depee/v1.2.3/install.sh"
        );
    }

    #[test]
    fn update_cache_without_skipped_version_parses() {
        let cache: UpdateCheckCache = serde_json::from_str(
            r#"{"checked_at":"2026-01-01T00:00:00Z","latest_version":"9.9.9"}"#,
        )
        .unwrap();
        assert_eq!(cache.skipped_version, None);
    }

    #[test]
    fn config_toggles_update_checks() {
        assert!(check_for_updates(""));
        assert!(!check_for_updates("check_for_updates = false"));
        assert!(!check_for_updates(
            "check_for_updates=true\ncheck_for_updates = false # off"
        ));
        assert!(check_for_updates("# check_for_updates = false"));
        assert!(check_for_updates("check_for_updates = \"no\""));
        assert!(check_for_updates("other = false"));
    }
}
