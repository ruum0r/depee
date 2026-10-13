//! Running `<pm> outdated` and normalizing each package manager's JSON output.

use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};

use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use tokio::{process::Command, time::timeout};

use super::Pm;
use crate::{package::Package, project::Manifest};

/// One row of `outdated` output, before it is matched against package.json.
#[derive(Debug, PartialEq, Eq)]
struct OutdatedEntry {
    name: String,
    current: String,
    wanted: Option<String>,
    latest: Option<String>,
}

pub struct ScanResult {
    /// Sorted by name.
    pub packages: Vec<Package>,
    /// Non-fatal problem worth surfacing in the status bar.
    pub warning: Option<String>,
}

/// Lists the outdated direct dependencies of the project at `cwd`.
pub async fn scan(cwd: &Path, pm: Pm) -> Result<ScanResult> {
    let declared = Manifest::read(cwd)?.declared();
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
    let mut packages: Vec<Package> = parse_outdated(pm, &data)
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
            if let Some(fields) = item.as_object() {
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
        });
    }
}

fn entry(name: &str, fields: &Map<String, Value>) -> OutdatedEntry {
    OutdatedEntry {
        name: name.to_string(),
        current: field(fields, &["current"]).unwrap_or("N/A").to_string(),
        wanted: field(fields, &["wanted"]).map(str::to_string),
        latest: field(fields, &["latest"]).map(str::to_string),
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
        }
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
    fn unexpected_shapes_yield_nothing() {
        for pm in [Pm::Npm, Pm::Pnpm, Pm::Yarn, Pm::Bun] {
            assert!(parse_outdated(pm, &Value::Null).is_empty());
            assert!(parse_outdated(pm, &json!("oops")).is_empty());
        }
    }
}
