//! Running `<pm> outdated` and normalizing each package manager's JSON output.

use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};

use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use tokio::{process::Command, time::timeout};

use super::Pm;
use crate::{
    package::Package,
    project::{self, Manifest},
};

/// One row of `outdated` output, before it is matched against package.json.
#[derive(Debug, PartialEq, Eq)]
struct OutdatedEntry {
    name: String,
    current: String,
    wanted: Option<String>,
    latest: Option<String>,
    /// The workspace that declares the dependency, when the package manager
    /// reports it: npm's `dependent` (the directory name) or Yarn Classic's
    /// `Workspace` column (the package name).
    dependent: Option<String>,
}

pub struct ScanResult {
    /// Sorted by name.
    pub packages: Vec<Package>,
    /// Non-fatal problem worth surfacing in the status bar.
    pub warning: Option<String>,
}

/// Lists the outdated direct dependencies of the project at `cwd`.
pub async fn scan(cwd: &Path, pm: Pm) -> Result<ScanResult> {
    let manifest = Manifest::read(cwd)?;
    // npm and Yarn Classic report rows for every workspace member, not just
    // the one `outdated` runs in, and say which member each row belongs to.
    let owner = match pm {
        Pm::Npm => cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        Pm::Yarn => manifest.name.clone(),
        Pm::Pnpm | Pm::Bun => None,
    };
    let declared = manifest.declared();
    let args: &[&str] = match pm {
        Pm::Pnpm => &["outdated", "--format", "json"],
        Pm::Npm | Pm::Yarn | Pm::Bun => &["outdated", "--json"],
    };
    // pnpm's default `wanted` field is the lockfile resolution. Ask pnpm for
    // its compatible target separately; in this output, `latest` is the
    // highest version that still satisfies the manifest range. This is
    // supplementary, so a failure falls back to pnpm's own `wanted`.
    let (data, compatible_targets) = if pm == Pm::Pnpm {
        let (data, compatible) = tokio::join!(
            outdated_json(cwd, pm, args),
            outdated_json(cwd, pm, &["outdated", "--compatible", "--format", "json"]),
        );
        let compatible = compatible.map(|compatible| {
            parse_outdated(Pm::Pnpm, &compatible)
                .into_iter()
                .filter_map(|entry| entry.latest.map(|target| (entry.name, target)))
                .collect::<HashMap<_, _>>()
        });
        (data?, Some(compatible))
    } else {
        (outdated_json(cwd, pm, args).await?, None)
    };
    let warning = match &compatible_targets {
        Some(Err(e)) => Some(format!(
            "compatible check failed, Wanted may be stale: {e:#}"
        )),
        _ => None,
    };
    let entries = own_entries(parse_outdated(pm, &data), owner.as_deref(), |name| {
        project::installed_version(cwd, name)
    });
    let mut packages: Vec<Package> = entries
        .into_iter()
        .filter_map(|entry| {
            let (current_spec, kind) = declared.get(&entry.name)?;
            let wanted = match &compatible_targets {
                Some(Ok(targets)) => targets
                    .get(&entry.name)
                    .cloned()
                    .or_else(|| Some(entry.current.clone())),
                _ => entry.wanted,
            };
            Some(Package::new(
                entry.name,
                entry.current,
                current_spec.clone(),
                wanted,
                entry.latest,
                *kind,
            ))
        })
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ScanResult { packages, warning })
}

/// Keeps the rows that belong to `owner` (rows without a dependent always
/// count), then one row per package. When several remain, for example two
/// members whose directories share a name, the one whose `current` matches
/// the version installed for this project wins, else the first.
fn own_entries(
    entries: Vec<OutdatedEntry>,
    owner: Option<&str>,
    installed: impl Fn(&str) -> Option<String>,
) -> Vec<OutdatedEntry> {
    let mut by_name: HashMap<String, Vec<OutdatedEntry>> = HashMap::new();
    let mut order = vec![];
    for entry in entries {
        let mine = match (owner, &entry.dependent) {
            (Some(owner), Some(dependent)) => owner == dependent,
            _ => true,
        };
        if !mine {
            continue;
        }
        if !by_name.contains_key(&entry.name) {
            order.push(entry.name.clone());
        }
        by_name.entry(entry.name.clone()).or_default().push(entry);
    }
    order
        .into_iter()
        .filter_map(|name| {
            let mut candidates = by_name.remove(&name)?;
            if candidates.len() > 1 {
                if let Some(version) = installed(&name) {
                    if let Some(i) = candidates.iter().position(|e| e.current == version) {
                        return Some(candidates.swap_remove(i));
                    }
                }
            }
            Some(candidates.swap_remove(0))
        })
        .collect()
}

async fn outdated_json(cwd: &Path, pm: Pm, args: &[&str]) -> Result<Value> {
    let command = format!("{} {}", pm.name(), args.join(" "));
    let mut cmd = Command::new(pm.name());
    cmd.current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = timeout(Duration::from_secs(45), cmd.output())
        .await
        .with_context(|| format!("Timed out running {command}"))??;
    // `outdated` exits with 1 when something is outdated, so only treat
    // higher codes without output as failures.
    if out.stdout.is_empty() && out.status.code().unwrap_or(0) > 1 {
        return Err(anyhow!(
            "{command} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(if out.stdout.is_empty() {
        Value::Null
    } else if pm == Pm::Yarn {
        // Yarn Classic prints one JSON document per line.
        Value::Array(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect(),
        )
    } else {
        serde_json::from_slice(&out.stdout)
            .with_context(|| format!("Invalid JSON from {command}"))?
    })
}

fn parse_outdated(pm: Pm, v: &Value) -> Vec<OutdatedEntry> {
    let mut out = vec![];
    match pm {
        Pm::Npm | Pm::Bun => parse_keyed_or_list(v, v.as_array(), &["name"], &mut out),
        Pm::Pnpm => parse_keyed_or_list(
            v,
            v.as_array()
                .or_else(|| v.get("packages").and_then(Value::as_array)),
            &["name", "package"],
            &mut out,
        ),
        Pm::Yarn => {
            for item in v.as_array().into_iter().flatten() {
                let Some(fields) = item.as_object() else {
                    continue;
                };
                if let Some(name) = field(fields, &["name"]) {
                    out.push(entry(name, fields));
                }
                if let Some(table) = item.pointer("/data").and_then(Value::as_object) {
                    parse_yarn_table(table, &mut out);
                }
            }
        }
    }
    out
}

/// Handles `{"<name>": {…}}` objects and `[{"name": …}]` lists.
fn parse_keyed_or_list(
    v: &Value,
    list: Option<&Vec<Value>>,
    name_keys: &[&str],
    out: &mut Vec<OutdatedEntry>,
) {
    if let Some(object) = v.as_object() {
        for (name, item) in object {
            // npm lists one object per dependent workspace when several
            // members depend on the same package.
            let items = match item {
                Value::Array(items) => items.as_slice(),
                item => std::slice::from_ref(item),
            };
            for fields in items.iter().filter_map(Value::as_object) {
                out.push(entry(name, fields));
            }
        }
    } else if let Some(items) = list {
        for fields in items.iter().filter_map(Value::as_object) {
            if let Some(name) = field(fields, name_keys) {
                out.push(entry(name, fields));
            }
        }
    }
}

/// Yarn Classic reports `{"type":"table","data":{"head":[…],"body":[[…]]}}`.
fn parse_yarn_table(table: &Map<String, Value>, out: &mut Vec<OutdatedEntry>) {
    let heads = table.get("head").and_then(Value::as_array);
    let body = table.get("body").and_then(Value::as_array);
    let (Some(heads), Some(body)) = (heads, body) else {
        return;
    };
    let column = |label: &str| {
        heads.iter().position(|head| {
            head.as_str()
                .is_some_and(|head| head.eq_ignore_ascii_case(label))
        })
    };
    let workspace_i = column("Workspace");
    let (Some(name_i), Some(current_i), Some(wanted_i), Some(latest_i)) = (
        column("Package"),
        column("Current"),
        column("Wanted"),
        column("Latest"),
    ) else {
        return;
    };
    for row in body.iter().filter_map(Value::as_array) {
        let at = |i: usize| row.get(i).and_then(Value::as_str);
        let Some(name) = at(name_i) else { continue };
        out.push(OutdatedEntry {
            name: name.to_string(),
            current: at(current_i).unwrap_or("N/A").to_string(),
            wanted: at(wanted_i).map(str::to_string),
            latest: at(latest_i).map(str::to_string),
            dependent: workspace_i.and_then(at).map(str::to_string),
        });
    }
}

fn entry(name: &str, fields: &Map<String, Value>) -> OutdatedEntry {
    OutdatedEntry {
        name: name.to_string(),
        current: field(fields, &["current"]).unwrap_or("N/A").to_string(),
        wanted: field(fields, &["wanted"]).map(str::to_string),
        latest: field(fields, &["latest"]).map(str::to_string),
        dependent: field(fields, &["dependent"]).map(str::to_string),
    }
}

fn field<'a>(fields: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(name: &str, current: &str, wanted: &str, latest: &str) -> OutdatedEntry {
        OutdatedEntry {
            name: name.into(),
            current: current.into(),
            wanted: Some(wanted.into()),
            latest: Some(latest.into()),
            dependent: None,
        }
    }

    fn owned(mut entry: OutdatedEntry, dependent: &str) -> OutdatedEntry {
        entry.dependent = Some(dependent.into());
        entry
    }

    #[test]
    fn npm_json_parses() {
        let v = json!({"vite":{"current":"1.0.0","wanted":"1.2.0","latest":"2.0.0"}});
        assert_eq!(
            parse_outdated(Pm::Npm, &v),
            [entry("vite", "1.0.0", "1.2.0", "2.0.0")]
        );
    }

    #[test]
    fn bun_array_parses_and_defaults_missing_current() {
        let v = json!([{"name":"vite","wanted":"1.2.0","latest":"2.0.0"}, {"current":"1"}]);
        assert_eq!(
            parse_outdated(Pm::Bun, &v),
            [entry("vite", "N/A", "1.2.0", "2.0.0")]
        );
    }

    #[test]
    fn pnpm_json_object_parses() {
        let v = json!({
            "vite": {
                "current": "6.0.1",
                "wanted": "6.4.0",
                "latest": "7.1.2",
                "dependencyType": "devDependencies"
            }
        });
        assert_eq!(
            parse_outdated(Pm::Pnpm, &v),
            [entry("vite", "6.0.1", "6.4.0", "7.1.2")]
        );
    }

    #[test]
    fn pnpm_array_accepts_package_key() {
        let v = json!([{"package":"vite","current":"6.0.1","wanted":"6.4.0","latest":"7.1.2"}]);
        assert_eq!(
            parse_outdated(Pm::Pnpm, &v),
            [entry("vite", "6.0.1", "6.4.0", "7.1.2")]
        );
    }

    #[test]
    fn yarn_classic_table_parses_by_column_name() {
        let v = json!([
            {"type":"info","data":"Color legend"},
            {"type":"table","data":{
                "head":["Package","Current","Wanted","Latest","Package Type","URL"],
                "body":[["left-pad","1.0.0","1.3.0","2.0.0","dependencies",""]]
            }}
        ]);
        assert_eq!(
            parse_outdated(Pm::Yarn, &v),
            [entry("left-pad", "1.0.0", "1.3.0", "2.0.0")]
        );
    }

    #[test]
    fn npm_workspace_arrays_parse_one_entry_per_dependent() {
        let v = json!({
            "chalk": {"current":"4.1.2","wanted":"4.1.2","latest":"6.0.1",
                      "dependent":"app","location":"node_modules/chalk"},
            "semver": [
                {"current":"6.3.1","wanted":"6.3.1","latest":"7.8.5",
                 "dependent":"app","location":"node_modules/semver"},
                {"current":"5.7.2","wanted":"5.7.2","latest":"7.8.5",
                 "dependent":"lib","location":"packages/lib/node_modules/semver"}
            ]
        });
        assert_eq!(
            parse_outdated(Pm::Npm, &v),
            [
                owned(entry("chalk", "4.1.2", "4.1.2", "6.0.1"), "app"),
                owned(entry("semver", "6.3.1", "6.3.1", "7.8.5"), "app"),
                owned(entry("semver", "5.7.2", "5.7.2", "7.8.5"), "lib"),
            ]
        );
    }

    #[test]
    fn yarn_classic_reads_workspace_column() {
        let v = json!([
            {"type":"table","data":{
                "head":["Package","Current","Wanted","Latest","Workspace","Package Type","URL"],
                "body":[
                    ["kleur","3.0.3","3.0.3","4.1.5","fx-yarn-classic","devDependencies",""],
                    ["semver","6.3.1","6.3.1","7.8.5","@fx/app","dependencies",""],
                    ["semver","5.7.2","5.7.2","7.8.5","@fx/lib","dependencies",""]
                ]
            }}
        ]);
        assert_eq!(
            parse_outdated(Pm::Yarn, &v),
            [
                owned(entry("kleur", "3.0.3", "3.0.3", "4.1.5"), "fx-yarn-classic"),
                owned(entry("semver", "6.3.1", "6.3.1", "7.8.5"), "@fx/app"),
                owned(entry("semver", "5.7.2", "5.7.2", "7.8.5"), "@fx/lib"),
            ]
        );
    }

    #[test]
    fn only_the_owners_entries_are_kept() {
        let entries = || {
            vec![
                owned(entry("kleur", "3.0.3", "3.0.3", "4.1.5"), "npm"),
                owned(entry("semver", "6.3.1", "6.3.1", "7.8.5"), "app"),
                owned(entry("semver", "5.7.2", "5.7.2", "7.8.5"), "lib"),
                owned(entry("is-number", "6.0.0", "6.0.0", "7.0.0"), "lib"),
            ]
        };
        let names = |entries: Vec<OutdatedEntry>| {
            entries
                .into_iter()
                .map(|e| format!("{}@{}", e.name, e.current))
                .collect::<Vec<_>>()
        };
        let none = |_: &str| None;
        assert_eq!(
            names(own_entries(entries(), Some("app"), none)),
            ["semver@6.3.1"]
        );
        assert_eq!(
            names(own_entries(entries(), Some("lib"), none)),
            ["semver@5.7.2", "is-number@6.0.0"]
        );
        assert_eq!(
            names(own_entries(entries(), Some("npm"), none)),
            ["kleur@3.0.3"]
        );
    }

    #[test]
    fn entries_without_dependent_are_kept() {
        let v = vec![entry("vite", "1.0.0", "1.2.0", "2.0.0")];
        assert_eq!(
            own_entries(v, Some("app"), |_| None),
            [entry("vite", "1.0.0", "1.2.0", "2.0.0")]
        );
        let v = vec![owned(entry("vite", "1.0.0", "1.2.0", "2.0.0"), "web")];
        assert_eq!(own_entries(v, None, |_| None).len(), 1);
    }

    #[test]
    fn duplicates_prefer_the_installed_version_else_the_first() {
        // Two members whose directories are both named `app`.
        let entries = || {
            vec![
                owned(entry("semver", "6.3.1", "6.3.1", "7.8.5"), "app"),
                owned(entry("semver", "5.7.2", "5.7.2", "7.8.5"), "app"),
            ]
        };
        let pick = |installed: Option<&str>| {
            let installed = installed.map(str::to_string);
            let kept = own_entries(entries(), Some("app"), |name| {
                assert_eq!(name, "semver");
                installed.clone()
            });
            assert_eq!(kept.len(), 1);
            kept[0].current.clone()
        };
        assert_eq!(pick(Some("5.7.2")), "5.7.2");
        assert_eq!(pick(Some("4.0.0")), "6.3.1");
        assert_eq!(pick(None), "6.3.1");
    }

    #[test]
    fn unexpected_shapes_yield_nothing() {
        for pm in [Pm::Npm, Pm::Pnpm, Pm::Yarn, Pm::Bun] {
            assert!(parse_outdated(pm, &Value::Null).is_empty());
            assert!(parse_outdated(pm, &json!("oops")).is_empty());
        }
    }
}
