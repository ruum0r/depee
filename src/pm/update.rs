//! Building and running the command that updates one dependency.

use std::{path::Path, process::Stdio, time::Duration};

use tokio::{process::Command, time::timeout};

use super::Pm;
use crate::{package::Kind, project::installed_version};

/// Everything needed to update one package, as shown on the confirmation screen.
#[derive(Clone, Debug)]
pub struct UpdateRequest {
    pub name: String,
    pub kind: Kind,
    pub current_spec: String,
    /// Version the user asked for; passed to the command when upgrading.
    pub requested_target: String,
    /// Version the update is expected to install.
    pub effective_target: String,
    /// Upgrade rewrites the manifest range; otherwise stay within it.
    pub upgrade: bool,
    pub yarn_berry: bool,
}

impl UpdateRequest {
    pub fn args(&self, pm: Pm) -> Vec<String> {
        update_args(pm, self)
    }

    pub fn command_line(&self, pm: Pm) -> String {
        format!("{} {}", pm.name(), self.args(pm).join(" "))
    }
}

// `pnpm update` and `bun update` stay in range but rewrite the manifest range
// to start at the installed version (`^2.0.0` → `^2.1.3`). npm and the Yarn
// commands from `update_args` leave package.json untouched.
pub fn compatible_update_rewrites_range(pm: Pm) -> bool {
    matches!(pm, Pm::Pnpm | Pm::Bun)
}

fn update_args(pm: Pm, request: &UpdateRequest) -> Vec<String> {
    let name = &request.name;
    let selector = if request.upgrade {
        format!("{name}@{}", request.requested_target)
    } else {
        name.clone()
    };
    match (pm, request.upgrade, request.kind) {
        (Pm::Npm | Pm::Pnpm | Pm::Bun, false, _) => vec!["update".into(), name.clone()],
        // Berry's `yarn up <name>` jumps to latest; pinning the existing range
        // keeps it in range and leaves package.json as is. Classic has no `up`.
        (Pm::Yarn, false, _) if request.yarn_berry => {
            vec!["up".into(), format!("{name}@{}", request.current_spec)]
        }
        (Pm::Yarn, false, _) => vec!["upgrade".into(), name.clone()],
        (Pm::Npm, true, Kind::Dev) => vec!["install".into(), "--save-dev".into(), selector],
        (Pm::Npm, true, Kind::Optional) => {
            vec!["install".into(), "--save-optional".into(), selector]
        }
        (Pm::Npm, true, Kind::Prod) => vec!["install".into(), "--save".into(), selector],
        (Pm::Pnpm | Pm::Yarn, true, Kind::Dev) => vec!["add".into(), "-D".into(), selector],
        (Pm::Pnpm | Pm::Yarn, true, Kind::Optional) => {
            vec!["add".into(), "-O".into(), selector]
        }
        (Pm::Bun, true, Kind::Dev) => vec!["add".into(), "-d".into(), selector],
        (Pm::Bun, true, Kind::Optional) => vec!["add".into(), "--optional".into(), selector],
        (Pm::Pnpm | Pm::Yarn | Pm::Bun, true, Kind::Prod) => vec!["add".into(), selector],
    }
}

/// Runs the update and describes what ended up installed.
pub async fn run_update(cwd: &Path, pm: Pm, request: &UpdateRequest) -> Result<String, String> {
    check_name(&request.name)?;
    let command = request.command_line(pm);
    let mut cmd = Command::new(pm.name());
    cmd.current_dir(cwd)
        .args(request.args(pm))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = timeout(Duration::from_secs(600), cmd.output())
        .await
        .map_err(|_| "Update timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("Update failed: {}", out.status)
        } else {
            stderr
        });
    }
    let name = &request.name;
    let requested = &request.requested_target;
    let effective = &request.effective_target;
    Ok(match installed_version(cwd, name) {
        Some(installed) if request.upgrade && installed == *requested => format!(
            "{command} succeeded · installed {name}@{installed} · requested target reached"
        ),
        Some(installed) if request.upgrade => format!(
            "{command} succeeded · installed {name}@{installed} · requested target {requested} not reached"
        ),
        Some(installed) if installed == *effective => format!(
            "{command} succeeded · installed {name}@{installed} · Compatible Update used Wanted {effective}"
        ),
        Some(installed) => format!(
            "{command} succeeded · installed {name}@{installed} · Wanted target was {effective}"
        ),
        None => format!(
            "{command} succeeded · installed version could not be read from node_modules"
        ),
    })
}

/// Names come from the package manager's output; one starting with `-` would
/// be read as an option.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("Refusing to update invalid package name: {name:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_look_like_options_are_refused() {
        assert!(check_name("@scope/lib").is_ok());
        assert!(check_name("lodash").is_ok());
        assert!(check_name("--global").is_err());
        assert!(check_name("").is_err());
    }

    fn request(target: &str, upgrade: bool, yarn_berry: bool) -> UpdateRequest {
        UpdateRequest {
            name: "@scope/lib".into(),
            kind: Kind::Dev,
            current_spec: "^1.0.0".into(),
            requested_target: target.into(),
            effective_target: target.into(),
            upgrade,
            yarn_berry,
        }
    }

    #[test]
    fn update_commands_keep_dependency_kind() {
        assert_eq!(
            request("2.0.0", true, false).args(Pm::Npm),
            ["install", "--save-dev", "@scope/lib@2.0.0"]
        );
        assert_eq!(
            request("1.1.0", false, false).args(Pm::Pnpm),
            ["update", "@scope/lib"]
        );
        // Yarn Classic has no `up`; Berry's bare `up` would jump to latest.
        assert_eq!(
            request("1.1.0", false, false).args(Pm::Yarn),
            ["upgrade", "@scope/lib"]
        );
        assert_eq!(
            request("1.1.0", false, true).args(Pm::Yarn),
            ["up", "@scope/lib@^1.0.0"]
        );
        assert_eq!(
            request("2.0.0", true, true).args(Pm::Yarn),
            ["add", "-D", "@scope/lib@2.0.0"]
        );
    }

    #[test]
    fn upgrade_flags_per_manager_and_kind() {
        let args = |pm, kind| {
            let mut request = request("2.0.0", true, false);
            request.kind = kind;
            request.args(pm).join(" ")
        };
        assert_eq!(args(Pm::Npm, Kind::Prod), "install --save @scope/lib@2.0.0");
        assert_eq!(
            args(Pm::Npm, Kind::Optional),
            "install --save-optional @scope/lib@2.0.0"
        );
        assert_eq!(args(Pm::Pnpm, Kind::Optional), "add -O @scope/lib@2.0.0");
        assert_eq!(args(Pm::Bun, Kind::Dev), "add -d @scope/lib@2.0.0");
        assert_eq!(
            args(Pm::Bun, Kind::Optional),
            "add --optional @scope/lib@2.0.0"
        );
        assert_eq!(args(Pm::Yarn, Kind::Prod), "add @scope/lib@2.0.0");
        assert_eq!(
            request("2.0.0", true, false).command_line(Pm::Pnpm),
            "pnpm add -D @scope/lib@2.0.0"
        );
    }
}
