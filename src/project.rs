//! Reading the Node.js project on disk: `package.json`, `node_modules`, and
//! the enclosing workspace root.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

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

/// Nearest directory at or above `cwd` that declares a workspace: it has a
/// `pnpm-workspace.yaml`, or its `package.json` has a `workspaces` array or
/// `{ "packages": [...] }` object. The search stops after the first directory
/// containing `.git`, so it never leaves the repository.
pub fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        if is_workspace_root(dir) {
            return Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    None
}

fn is_workspace_root(dir: &Path) -> bool {
    if dir.join("pnpm-workspace.yaml").is_file() {
        return true;
    }
    let Ok(bytes) = fs::read(dir.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    match value.get("workspaces") {
        Some(Value::Array(_)) => true,
        Some(Value::Object(fields)) => fields.get("packages").is_some_and(Value::is_array),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_found_from_members() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Keep the search inside the temp dir.
        fs::create_dir(root.join(".git")).unwrap();
        let member = root.join("packages/web");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("package.json"), r#"{"name":"web"}"#).unwrap();
        assert_eq!(find_workspace_root(&member), None);

        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n",
        )
        .unwrap();
        assert_eq!(find_workspace_root(&member).as_deref(), Some(root));
        assert_eq!(find_workspace_root(root).as_deref(), Some(root));
        fs::remove_file(root.join("pnpm-workspace.yaml")).unwrap();

        fs::write(
            root.join("package.json"),
            r#"{"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        assert_eq!(find_workspace_root(&member).as_deref(), Some(root));

        fs::write(
            root.join("package.json"),
            r#"{"workspaces":{"packages":["packages/*"],"nohoist":[]}}"#,
        )
        .unwrap();
        assert_eq!(find_workspace_root(&member).as_deref(), Some(root));

        for manifest in [
            r#"{"workspaces":{"nohoist":[]}}"#,
            r#"{"workspaces":"packages/*"}"#,
            "{",
        ] {
            fs::write(root.join("package.json"), manifest).unwrap();
            assert_eq!(find_workspace_root(&member), None, "{manifest}");
        }
    }

    #[test]
    fn workspace_search_stops_at_repository_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path();
        fs::create_dir(outer.join(".git")).unwrap();
        fs::write(outer.join("package.json"), r#"{"workspaces":["*"]}"#).unwrap();
        let repo = outer.join("repo");
        let member = repo.join("packages/web");
        fs::create_dir_all(&member).unwrap();
        assert_eq!(find_workspace_root(&member).as_deref(), Some(outer));

        // Worktrees and submodules have a `.git` file rather than a directory.
        fs::write(repo.join(".git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(find_workspace_root(&member), None);
        fs::write(repo.join("pnpm-workspace.yaml"), "").unwrap();
        assert_eq!(
            find_workspace_root(&member).as_deref(),
            Some(repo.as_path())
        );
    }

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
