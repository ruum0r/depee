//! Package manager detection and the CLI commands depee runs through it.

mod outdated;
mod update;

use std::{fs, path::Path};

use anyhow::{anyhow, Result};
use clap::ValueEnum;

pub use outdated::{scan, ScanResult};
pub use update::{compatible_update_rewrites_range, run_update, UpdateRequest};

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Pm {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Pm {
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    /// Picks the package manager from, in order: the `--pm` flag, the
    /// `packageManager` field, then lockfiles. Defaults to npm.
    pub fn detect(explicit: Option<Pm>, field: Option<&str>, cwd: &Path) -> Result<Pm> {
        if let Some(pm) = explicit {
            return Ok(pm);
        }
        if let Some(name) = field.and_then(|x| x.split('@').next()) {
            return match name {
                "npm" => Ok(Pm::Npm),
                "pnpm" => Ok(Pm::Pnpm),
                "yarn" => Ok(Pm::Yarn),
                "bun" => Ok(Pm::Bun),
                _ => Err(anyhow!("Unsupported packageManager: {name}")),
            };
        }
        let locks = [
            ("package-lock.json", Pm::Npm),
            ("npm-shrinkwrap.json", Pm::Npm),
            ("pnpm-lock.yaml", Pm::Pnpm),
            ("yarn.lock", Pm::Yarn),
            ("bun.lock", Pm::Bun),
            ("bun.lockb", Pm::Bun),
        ];
        let found: Vec<_> = locks
            .into_iter()
            .filter(|(file, _)| cwd.join(file).exists())
            .collect();
        let mut pms: Vec<Pm> = found.iter().map(|(_, pm)| *pm).collect();
        // Lockfiles of the same manager are adjacent in `locks`.
        pms.dedup();
        match pms.as_slice() {
            [] => Ok(Pm::Npm),
            [pm] => Ok(*pm),
            _ => Err(anyhow!(
                "Ambiguous lockfiles ({}); choose one with --pm",
                found
                    .iter()
                    .map(|(file, _)| *file)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// Yarn Classic and Berry need different commands for an in-range update, so
/// guess the flavor before `yarn --version` reports back.
pub fn detect_yarn_berry(package_manager: Option<&str>, cwd: &Path) -> bool {
    if let Some(major) = package_manager
        .and_then(|field| field.strip_prefix("yarn@"))
        .and_then(yarn_major)
    {
        return major >= 2;
    }
    if let Ok(lock) = fs::read_to_string(cwd.join("yarn.lock")) {
        return lock.contains("__metadata:");
    }
    cwd.join(".yarnrc.yml").exists()
}

pub fn yarn_major(version: &str) -> Option<u64> {
    version.trim().split('.').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pm_override_wins() {
        assert_eq!(
            Pm::detect(Some(Pm::Bun), Some("npm@10"), Path::new("/does-not-exist")).unwrap(),
            Pm::Bun
        );
    }

    #[test]
    fn detects_from_package_manager_field_then_lockfiles() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        assert_eq!(Pm::detect(None, Some("pnpm@9.1.0"), cwd).unwrap(), Pm::Pnpm);
        assert!(Pm::detect(None, Some("deno@2"), cwd).is_err());
        assert_eq!(Pm::detect(None, None, cwd).unwrap(), Pm::Npm);
        fs::write(cwd.join("bun.lock"), "").unwrap();
        fs::write(cwd.join("bun.lockb"), "").unwrap();
        assert_eq!(Pm::detect(None, None, cwd).unwrap(), Pm::Bun);
        fs::write(cwd.join("yarn.lock"), "").unwrap();
        let error = Pm::detect(None, None, cwd).unwrap_err().to_string();
        assert!(error.contains("yarn.lock, bun.lock, bun.lockb"), "{error}");
    }

    #[test]
    fn detects_yarn_flavor() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert!(detect_yarn_berry(Some("yarn@4.9.1"), dir));
        assert!(!detect_yarn_berry(Some("yarn@1.22.22"), dir));
        assert!(!detect_yarn_berry(None, dir));
        fs::write(dir.join("yarn.lock"), "# yarn lockfile v1\n").unwrap();
        assert!(!detect_yarn_berry(None, dir));
        fs::write(dir.join("yarn.lock"), "__metadata:\n  version: 8\n").unwrap();
        assert!(detect_yarn_berry(None, dir));
        assert_eq!(yarn_major("4.9.1\n"), Some(4));
        assert_eq!(yarn_major("N/A"), None);
    }
}
