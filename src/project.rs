//! Reading the Node.js project on disk: `package.json` and `node_modules`.

use std::{collections::HashMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::package::Kind;

#[derive(Deserialize)]
pub struct Manifest {
    pub name: Option<String>,
    #[serde(rename = "packageManager")]
    pub package_manager: Option<String>,
    dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "devDependencies")]
    dev: Option<HashMap<String, String>>,
    #[serde(rename = "optionalDependencies")]
    optional: Option<HashMap<String, String>>,
}

impl Manifest {
    pub fn read(cwd: &Path) -> Result<Self> {
        let bytes = fs::read(cwd.join("package.json")).context("Could not read package.json")?;
        serde_json::from_slice(&bytes).context("package.json is invalid JSON")
    }

    pub fn dependency_count(&self) -> usize {
        [&self.dependencies, &self.dev, &self.optional]
            .into_iter()
            .map(|group| group.as_ref().map_or(0, HashMap::len))
            .sum()
    }

    /// Declared range and dependency kind by package name.
    pub fn declared(self) -> HashMap<String, (String, Kind)> {
        let mut declared = HashMap::new();
        for (kind, group) in [
            (Kind::Prod, self.dependencies),
            (Kind::Dev, self.dev),
            (Kind::Optional, self.optional),
        ] {
            for (name, spec) in group.unwrap_or_default() {
                declared.insert(name, (spec, kind));
            }
        }
        declared
    }
}

/// Version of `package` that Node would resolve from `cwd`, walking up through
/// parent `node_modules` directories.
pub fn installed_version(cwd: &Path, package: &str) -> Option<String> {
    let parts: Vec<_> = package.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    cwd.ancestors().find_map(|root| {
        let path = parts
            .iter()
            .fold(root.join("node_modules"), |path, part| path.join(part))
            .join("package.json");
        let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        value.get("version")?.as_str().map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_version_walks_up_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let package_dir = dir.path().join("node_modules/@scope/peer");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();
        let nested = dir.path().join("nested");
        assert_eq!(
            installed_version(&nested, "@scope/peer").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(installed_version(&nested, "../escape"), None);
        assert_eq!(installed_version(&nested, "@scope/"), None);
        assert_eq!(installed_version(&nested, "missing"), None);
    }

    #[test]
    fn manifest_counts_and_classifies_dependencies() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "dependencies": {"a": "^1.0.0"},
                "devDependencies": {"b": "~2.0.0", "c": "3"},
                "optionalDependencies": {"d": "*"}
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.dependency_count(), 4);
        let declared = manifest.declared();
        assert_eq!(declared["a"], ("^1.0.0".into(), Kind::Prod));
        assert_eq!(declared["b"], ("~2.0.0".into(), Kind::Dev));
        assert_eq!(declared["d"], ("*".into(), Kind::Optional));
    }
}
