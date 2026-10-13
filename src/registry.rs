//! npm Registry client: package documents (cached on disk) and download counts.

use std::{collections::HashMap, fs, path::PathBuf, sync::OnceLock, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Utc};
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
    let doc = match read_cached(name) {
        Some(doc) => doc,
        None => {
            let doc: Value = client
                .get(format!("https://registry.npmjs.org/{encoded}"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            write_cache(name, &doc)?;
            doc
        }
    };
    // Download counts are a nice-to-have; failures leave them unknown.
    let downloads = match client
        .get(format!(
            "https://api.npmjs.org/downloads/point/last-week/{encoded}"
        ))
        .send()
        .await
    {
        Ok(response) => response
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v.get("downloads").and_then(Value::as_u64)),
        Err(_) => None,
    };
    Ok(parse_document(&doc, downloads))
}

fn cache_path(name: &str) -> Option<PathBuf> {
    cache::dir().map(|dir| dir.join(format!("{}.json", name.replace('/', "_").replace('@', ""))))
}

fn read_cached(name: &str) -> Option<Value> {
    let bytes = fs::read(cache_path(name)?).ok()?;
    let (doc, fetched_at) = serde_json::from_slice::<(Value, DateTime<Utc>)>(&bytes).ok()?;
    (Utc::now().signed_duration_since(fetched_at).num_hours() < CACHE_TTL_HOURS).then_some(doc)
}

/// The cache is best effort; only serialization failures are reported.
fn write_cache(name: &str, doc: &Value) -> Result<()> {
    if let Some(path) = cache_path(name) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, serde_json::to_vec(&(doc, Utc::now()))?);
    }
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

    #[test]
    fn accepts_string_repository_and_missing_fields() {
        let metadata = parse_document(&json!({"repository": "https://example.com/r"}), None);
        assert_eq!(metadata.repo.as_deref(), Some("https://example.com/r"));
        assert!(metadata.releases.is_empty());
        assert_eq!(metadata.published, None);
    }
}
