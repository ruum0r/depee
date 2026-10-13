//! npm Registry client: package metadata (parsed, cached on disk) and download counts.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cache,
    package::{Change, DependencySpecs, Metadata, Release},
};

const CACHE_TTL_HOURS: i64 = 12;

/// One client for every registry request, so its connection pool (and the
/// TLS sessions in it) is shared across packages.
fn client() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    // A failed build is returned, not cached, so the next request retries.
    let client = reqwest::Client::builder()
        .user_agent("depee/0.1")
        .timeout(Duration::from_secs(12))
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

pub async fn fetch_metadata(name: &str) -> Result<Metadata> {
    let client = client()?;
    let encoded = name.replace('/', "%2f");
    let path = cache_path(name);
    let now = Utc::now();
    let cached = path.as_deref().and_then(|path| read_cached(path, now));
    match plan(cached) {
        Plan::Cached(metadata) => Ok(metadata),
        Plan::Downloads(entry) => {
            let downloads = fetch_downloads(client, &encoded).await;
            let metadata = Metadata {
                downloads,
                ..entry.metadata
            };
            // Keep the original fetch time so the entry still expires on schedule.
            if let (Some(_), Some(path)) = (downloads, &path) {
                write_cache(path, &metadata, entry.fetched_at)?;
            }
            Ok(metadata)
        }
        Plan::Everything => {
            let (doc, downloads) = tokio::join!(
                fetch_document(client, &encoded),
                fetch_downloads(client, &encoded)
            );
            let metadata = parse_document(&doc?, downloads);
            if let Some(path) = &path {
                write_cache(path, &metadata, now)?;
            }
            Ok(metadata)
        }
    }
}

/// What `fetch_metadata` still has to request, given the fresh cache entry.
#[derive(Debug)]
enum Plan {
    /// Everything is cached; no request at all.
    Cached(Metadata),
    /// The document is cached but the download count is unknown (its last
    /// lookup failed), so only the count is retried.
    Downloads(CacheEntry<Metadata>),
    /// No fresh entry: the document and the download count.
    Everything,
}

fn plan(cached: Option<CacheEntry<Metadata>>) -> Plan {
    match cached {
        None => Plan::Everything,
        Some(entry) if entry.metadata.downloads.is_none() => Plan::Downloads(entry),
        Some(entry) => Plan::Cached(entry.metadata),
    }
}

async fn fetch_document(client: &reqwest::Client, encoded: &str) -> Result<Value> {
    Ok(client
        .get(format!("https://registry.npmjs.org/{encoded}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Download counts are a nice-to-have; failures leave them unknown.
async fn fetch_downloads(client: &reqwest::Client, encoded: &str) -> Option<u64> {
    let response = client
        .get(format!(
            "https://api.npmjs.org/downloads/point/last-week/{encoded}"
        ))
        .send()
        .await
        .ok()?;
    response
        .json::<Value>()
        .await
        .ok()?
        .get("downloads")
        .and_then(Value::as_u64)
}

fn cache_path(name: &str) -> Option<PathBuf> {
    cache::dir().map(|dir| dir.join(format!("{}.json", name.replace('/', "_").replace('@', ""))))
}

/// Bump when the cached `Metadata` changes meaning, so older entries are
/// refetched. Files from before versioning (raw registry documents stored as
/// `[document, fetched_at]`) fail to deserialize and are refetched too.
/// Format 2 stores the weekly download count (`None` when its lookup failed).
const CACHE_FORMAT: u32 = 2;

/// On-disk cache entry: parsed metadata, including the download count.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry<M> {
    format: u32,
    fetched_at: DateTime<Utc>,
    metadata: M,
}

/// The fresh entry at `path`, or `None` when it is missing, unreadable, from
/// another format, or older than the TTL.
fn read_cached(path: &Path, now: DateTime<Utc>) -> Option<CacheEntry<Metadata>> {
    let bytes = fs::read(path).ok()?;
    let entry = serde_json::from_slice::<CacheEntry<Metadata>>(&bytes).ok()?;
    let fresh = entry.format == CACHE_FORMAT
        && now.signed_duration_since(entry.fetched_at).num_hours() < CACHE_TTL_HOURS;
    fresh.then_some(entry)
}

/// The cache is best effort; only serialization failures are reported.
/// `fetched_at` is when the document was fetched; the TTL counts from it.
fn write_cache(path: &Path, metadata: &Metadata, fetched_at: DateTime<Utc>) -> Result<()> {
    let entry = CacheEntry {
        format: CACHE_FORMAT,
        fetched_at,
        metadata,
    };
    let bytes = serde_json::to_vec(&entry)?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, bytes);
    Ok(())
}

/// Converts a registry package document (`GET /<name>`) into `Metadata`.
fn parse_document(doc: &Value, downloads: Option<u64>) -> Metadata {
    let latest = doc
        .pointer("/dist-tags/latest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let versions = doc.get("versions").and_then(Value::as_object);
    let times = doc.get("time").and_then(Value::as_object);
    let published_at = |version: &str| {
        times
            .and_then(|t| t.get(version))
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    };
    let mut releases: Vec<Release> = versions
        .into_iter()
        .flatten()
        .map(|(version, data)| parse_release(version, data, published_at(version)))
        .collect();
    releases.sort_by(|a, b| {
        semver::Version::parse(&b.version)
            .ok()
            .cmp(&semver::Version::parse(&a.version).ok())
    });
    let repo = doc
        .pointer("/repository/url")
        .and_then(Value::as_str)
        .or_else(|| doc.get("repository").and_then(Value::as_str))
        .map(|s| {
            s.trim_start_matches("git+")
                .trim_end_matches(".git")
                .to_string()
        });
    Metadata {
        description: string(doc.get("description")),
        license: versions
            .and_then(|v| v.get(latest))
            .and_then(|data| license(data.get("license"))),
        repo,
        homepage: string(doc.get("homepage")),
        published: published_at(latest),
        downloads,
        releases,
    }
}

fn parse_release(version: &str, data: &Value, published: Option<DateTime<Utc>>) -> Release {
    let dependency_specs = DependencySpecs {
        dependencies: string_map(data.get("dependencies")),
        peer: string_map(data.get("peerDependencies")),
        optional: string_map(data.get("optionalDependencies")),
    };
    Release {
        version: version.to_string(),
        published,
        change: Change::Unknown,
        prerelease: semver::Version::parse(version).is_ok_and(|v| !v.pre.is_empty()),
        dependencies: dependency_specs.dependencies.keys().cloned().collect(),
        dependency_specs,
        engines_node: string(data.pointer("/engines/node")),
        deprecated: string(data.get("deprecated")),
        license: license(data.get("license")),
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

/// `"MIT"` or the legacy `{"type": "MIT"}` form.
fn license(value: Option<&Value>) -> Option<String> {
    let value = value?;
    string(Some(value)).or_else(|| string(value.get("type")))
}

fn string_map(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(name, spec)| {
                    spec.as_str().map(|spec| (name.clone(), spec.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reuses_one_client() {
        let first = client().unwrap();
        let second = client().unwrap();
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn parses_registry_document() {
        let doc = json!({
            "description": "A lib",
            "dist-tags": {"latest": "2.0.0"},
            "repository": {"url": "git+https://github.com/x/lib.git"},
            "time": {"1.0.0": "2024-01-01T00:00:00Z", "2.0.0": "2025-01-01T00:00:00Z"},
            "versions": {
                "1.0.0": {"license": {"type": "ISC"}},
                "2.0.0-rc.1": {},
                "2.0.0": {
                    "license": "MIT",
                    "engines": {"node": ">=18"},
                    "deprecated": "use lib2",
                    "dependencies": {"a": "^1.0.0", "bad": 1},
                    "peerDependencies": {"react": "^19.0.0"}
                }
            }
        });
        let metadata = parse_document(&doc, Some(42));
        assert_eq!(metadata.description.as_deref(), Some("A lib"));
        assert_eq!(metadata.license.as_deref(), Some("MIT"));
        assert_eq!(metadata.repo.as_deref(), Some("https://github.com/x/lib"));
        assert_eq!(metadata.homepage, None);
        assert_eq!(metadata.downloads, Some(42));
        assert_eq!(
            metadata.published.map(|d| d.to_rfc3339()).as_deref(),
            Some("2025-01-01T00:00:00+00:00")
        );
        let versions: Vec<_> = metadata.releases.iter().map(|r| &r.version[..]).collect();
        assert_eq!(versions, ["2.0.0", "2.0.0-rc.1", "1.0.0"]);

        let latest = &metadata.releases[0];
        assert_eq!(latest.dependencies, ["a"]);
        assert_eq!(latest.dependency_specs.peer["react"], "^19.0.0");
        assert_eq!(latest.engines_node.as_deref(), Some(">=18"));
        assert_eq!(latest.deprecated.as_deref(), Some("use lib2"));
        assert!(metadata.releases[1].prerelease);
        assert_eq!(metadata.releases[2].license.as_deref(), Some("ISC"));
    }

    fn sample_document() -> Value {
        json!({
            "description": "A lib",
            "dist-tags": {"latest": "2.0.0"},
            "time": {"1.0.0": "2024-01-01T00:00:00Z", "2.0.0": "2025-01-01T00:00:00Z"},
            "versions": {
                "1.0.0": {"license": "ISC"},
                "2.0.0": {
                    "license": "MIT",
                    "engines": {"node": ">=18"},
                    "dependencies": {"a": "^1.0.0"},
                    "peerDependencies": {"react": "^19.0.0"}
                }
            }
        })
    }

    #[test]
    fn cache_round_trips_metadata_with_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("lib.json");
        let now = Utc::now();
        write_cache(&path, &parse_document(&sample_document(), Some(42)), now).unwrap();

        let entry = read_cached(&path, now).unwrap();
        assert_eq!(entry.fetched_at, now);
        let metadata = entry.metadata;
        assert_eq!(metadata.description.as_deref(), Some("A lib"));
        assert_eq!(metadata.license.as_deref(), Some("MIT"));
        assert_eq!(metadata.downloads, Some(42));
        assert_eq!(
            metadata.published.map(|d| d.to_rfc3339()).as_deref(),
            Some("2025-01-01T00:00:00+00:00")
        );
        let versions: Vec<_> = metadata.releases.iter().map(|r| &r.version[..]).collect();
        assert_eq!(versions, ["2.0.0", "1.0.0"]);
        let latest = &metadata.releases[0];
        assert_eq!(latest.change, Change::Unknown);
        assert_eq!(latest.dependencies, ["a"]);
        assert_eq!(latest.dependency_specs.peer["react"], "^19.0.0");
        assert_eq!(latest.engines_node.as_deref(), Some(">=18"));
        assert_eq!(metadata.releases[1].license.as_deref(), Some("ISC"));
    }

    fn entry(downloads: Option<u64>) -> CacheEntry<Metadata> {
        CacheEntry {
            format: CACHE_FORMAT,
            fetched_at: Utc::now(),
            metadata: parse_document(&sample_document(), downloads),
        }
    }

    #[test]
    fn plan_fetches_everything_without_a_fresh_entry() {
        assert!(matches!(plan(None), Plan::Everything));
    }

    #[test]
    fn plan_fetches_nothing_when_downloads_are_cached() {
        let Plan::Cached(metadata) = plan(Some(entry(Some(42)))) else {
            panic!("expected a cache hit");
        };
        assert_eq!(metadata.downloads, Some(42));
        assert_eq!(metadata.description.as_deref(), Some("A lib"));
    }

    #[test]
    fn plan_retries_only_a_failed_download_lookup() {
        let Plan::Downloads(entry) = plan(Some(entry(None))) else {
            panic!("expected a download retry");
        };
        assert_eq!(entry.metadata.description.as_deref(), Some("A lib"));
    }

    #[test]
    fn failed_download_lookup_is_retried_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.json");
        let fetched_at = Utc::now();
        write_cache(&path, &parse_document(&sample_document(), None), fetched_at).unwrap();
        let cached = read_cached(&path, fetched_at);
        assert!(matches!(plan(cached), Plan::Downloads(_)));

        // A successful retry is cached without extending the entry's TTL.
        let later = fetched_at + chrono::Duration::hours(CACHE_TTL_HOURS - 1);
        write_cache(
            &path,
            &parse_document(&sample_document(), Some(7)),
            fetched_at,
        )
        .unwrap();
        let Plan::Cached(metadata) = plan(read_cached(&path, later)) else {
            panic!("expected a cache hit");
        };
        assert_eq!(metadata.downloads, Some(7));
        let expired = fetched_at + chrono::Duration::hours(CACHE_TTL_HOURS);
        assert!(matches!(
            plan(read_cached(&path, expired)),
            Plan::Everything
        ));
    }

    #[test]
    fn expired_cache_entry_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.json");
        let fetched_at = Utc::now();
        write_cache(&path, &parse_document(&sample_document(), None), fetched_at).unwrap();

        let hours = |h| fetched_at + chrono::Duration::hours(h);
        assert!(read_cached(&path, hours(CACHE_TTL_HOURS - 1)).is_some());
        assert!(read_cached(&path, hours(CACHE_TTL_HOURS)).is_none());
    }

    #[test]
    fn legacy_document_cache_is_a_miss_and_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.json");
        let now = Utc::now();
        let legacy = serde_json::to_vec(&(sample_document(), now)).unwrap();
        fs::write(&path, legacy).unwrap();
        assert!(read_cached(&path, now).is_none());

        write_cache(&path, &parse_document(&sample_document(), None), now).unwrap();
        assert!(read_cached(&path, now).is_some());
    }

    #[test]
    fn corrupt_or_unknown_format_cache_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.json");
        let now = Utc::now();
        assert!(read_cached(&path, now).is_none());

        write_cache(&path, &parse_document(&sample_document(), None), now).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(read_cached(&path, now).is_none());

        fs::write(&path, b"not json").unwrap();
        assert!(read_cached(&path, now).is_none());

        let mut entry: Value = serde_json::from_slice(&bytes).unwrap();
        for format in [CACHE_FORMAT - 1, CACHE_FORMAT + 1] {
            entry["format"] = json!(format);
            fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
            assert!(read_cached(&path, now).is_none());
        }
    }

    #[test]
    fn accepts_string_repository_and_missing_fields() {
        let metadata = parse_document(&json!({"repository": "https://example.com/r"}), None);
        assert_eq!(metadata.repo.as_deref(), Some("https://example.com/r"));
        assert!(metadata.releases.is_empty());
        assert_eq!(metadata.published, None);
    }
}
