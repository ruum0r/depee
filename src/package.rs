//! Core data types describing an outdated dependency and its published releases.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prod,
    Dev,
    Optional,
}

/// Size of the semver jump between two versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Patch,
    Minor,
    Major,
    Pre,
    Unknown,
}

impl Change {
    pub fn between(from: &str, to: Option<&str>) -> Self {
        let (Ok(from), Some(Ok(to))) = (
            semver::Version::parse(clean_spec(from)),
            to.map(|to| semver::Version::parse(clean_spec(to))),
        ) else {
            return Self::Unknown;
        };
        if !to.pre.is_empty() {
            Self::Pre
        } else if from.major != to.major {
            Self::Major
        } else if from.minor != to.minor {
            Self::Minor
        } else if from.patch != to.patch {
            Self::Patch
        } else {
            Self::Unknown
        }
    }

    /// Sort key: larger jumps rank higher.
    pub fn rank(self) -> u8 {
        match self {
            Self::Major => 4,
            Self::Minor => 3,
            Self::Patch => 2,
            Self::Pre => 1,
            Self::Unknown => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Patch => "PATCH",
            Self::Minor => "MINOR",
            Self::Major => "MAJOR",
            Self::Pre => "PRE",
            Self::Unknown => "N/A",
        }
    }
}

pub fn clean_spec(s: &str) -> &str {
    s.trim_start_matches(['v', '^', '~', '='])
}

#[derive(Clone, Debug, Default)]
pub struct DependencySpecs {
    pub dependencies: HashMap<String, String>,
    pub peer: HashMap<String, String>,
    pub optional: HashMap<String, String>,
}

impl DependencySpecs {
    pub fn groups(&self) -> [(&'static str, &HashMap<String, String>); 3] {
        [
            ("dependencies", &self.dependencies),
            ("peerDependencies", &self.peer),
            ("optionalDependencies", &self.optional),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub published: Option<DateTime<Utc>>,
    /// Change relative to the installed version; set once the package is known.
    pub change: Change,
    pub prerelease: bool,
    pub dependencies: Vec<String>,
    pub dependency_specs: DependencySpecs,
    pub engines_node: Option<String>,
    pub deprecated: Option<String>,
    pub license: Option<String>,
}

/// npm Registry data for one package.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub description: Option<String>,
    pub license: Option<String>,
    pub repo: Option<String>,
    pub homepage: Option<String>,
    pub published: Option<DateTime<Utc>>,
    pub downloads: Option<u64>,
    /// Newest first.
    pub releases: Vec<Release>,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub current: String,
    pub current_spec: String,
    pub wanted: Option<String>,
    pub latest: Option<String>,
    pub kind: Kind,
    pub change: Change,
    pub description: Option<String>,
    pub license: Option<String>,
    pub repo: Option<String>,
    pub homepage: Option<String>,
    pub current_published: Option<DateTime<Utc>>,
    pub published: Option<DateTime<Utc>>,
    pub downloads: Option<u64>,
    pub releases: Vec<Release>,
    /// Cursor of the version picker into `releases`.
    pub release_index: usize,
}

impl Package {
    /// A package as reported by the package manager, before registry metadata arrives.
    pub fn new(
        name: String,
        current: String,
        current_spec: String,
        wanted: Option<String>,
        latest: Option<String>,
        kind: Kind,
    ) -> Self {
        let change = Change::between(&current, latest.as_deref());
        Self {
            name,
            current,
            current_spec,
            wanted,
            latest,
            kind,
            change,
            description: None,
            license: None,
            repo: None,
            homepage: None,
            current_published: None,
            published: None,
            downloads: None,
            releases: vec![],
            release_index: 0,
        }
    }

    pub fn apply_metadata(&mut self, metadata: Metadata) {
        self.description = metadata.description;
        self.license = metadata.license;
        self.repo = metadata.repo;
        self.homepage = metadata.homepage;
        self.current_published = metadata
            .releases
            .iter()
            .find(|release| release.version == self.current)
            .and_then(|release| release.published);
        self.published = metadata.published;
        self.downloads = metadata.downloads;
        self.releases = metadata.releases;
        for release in &mut self.releases {
            release.change = Change::between(&self.current, Some(&release.version));
        }
    }

    pub fn release(&self, version: &str) -> Option<&Release> {
        self.releases
            .iter()
            .find(|release| release.version == version)
    }

    /// GitHub releases page when the repository is on GitHub, else the homepage.
    pub fn changelog_url(&self) -> Option<String> {
        self.repo
            .as_deref()
            .filter(|repo| repo.contains("github.com"))
            .map(|repo| format!("{}/releases", repo.trim_end_matches('/')))
            .or_else(|| self.homepage.clone())
    }

    /// Newest stable version reachable as a patch, minor, and major update.
    pub fn version_lanes(&self) -> (String, String, String) {
        let Ok(current) = semver::Version::parse(clean_spec(&self.current)) else {
            return ("N/A".into(), "N/A".into(), "N/A".into());
        };
        let stable: Vec<(semver::Version, &str)> = self
            .releases
            .iter()
            .filter_map(|release| {
                let version = semver::Version::parse(&release.version).ok()?;
                version
                    .pre
                    .is_empty()
                    .then_some((version, release.version.as_str()))
            })
            .collect();
        fn newest<'a>(
            matching: impl Iterator<Item = &'a (semver::Version, &'a str)>,
        ) -> Option<String> {
            matching
                .max_by(|a, b| a.0.cmp(&b.0))
                .map(|(_, version)| (*version).to_string())
        }
        let patch = newest(stable.iter().filter(|(version, _)| {
            version.major == current.major && version.minor == current.minor && *version >= current
        }))
        .unwrap_or_else(|| "N/A".into());
        let minor = newest(
            stable
                .iter()
                .filter(|(version, _)| version.major == current.major && *version >= current),
        )
        .unwrap_or_else(|| patch.clone());
        let major = newest(stable.iter())
            .or_else(|| self.latest.clone())
            .unwrap_or_else(|| "N/A".into());
        (patch, minor, major)
    }
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub fn package() -> Package {
        Package::new(
            "lib".into(),
            "1.0.0".into(),
            "^1.0.0".into(),
            Some("1.1.0".into()),
            Some("2.0.0".into()),
            Kind::Prod,
        )
    }

    pub fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            published: None,
            change: Change::Unknown,
            prerelease: false,
            dependencies: vec![],
            dependency_specs: DependencySpecs::default(),
            engines_node: None,
            deprecated: None,
            license: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn classifies_versions() {
        assert_eq!(Change::between("1.2.3", Some("1.2.4")), Change::Patch);
        assert_eq!(Change::between("1.2.3", Some("1.3.0")), Change::Minor);
        assert_eq!(Change::between("1.2.3", Some("2.0.0")), Change::Major);
        assert_eq!(Change::between("^1.2.3", Some("1.2.4")), Change::Patch);
        assert_eq!(
            Change::between("workspace:*", Some("2.0.0")),
            Change::Unknown
        );
        assert_eq!(Change::between("1.0.0", Some("2.0.0-beta.1")), Change::Pre);
        assert_eq!(Change::between("1.0.0", None), Change::Unknown);
    }

    #[test]
    fn version_table_groups_patch_minor_and_major() {
        let mut package = Package::new(
            "@types/node".into(),
            "24.13.2".into(),
            "^24.13.2".into(),
            Some("24.15.0".into()),
            Some("26.6.5".into()),
            Kind::Dev,
        );
        for version in ["24.13.2", "24.13.5", "24.15.0", "25.0.0", "26.6.5"] {
            package.releases.push(release(version));
        }
        assert_eq!(
            package.version_lanes(),
            ("24.13.5".into(), "24.15.0".into(), "26.6.5".into())
        );
    }

    #[test]
    fn version_lanes_skip_prereleases_and_fall_back() {
        let mut package = package();
        assert_eq!(
            package.version_lanes(),
            ("N/A".into(), "N/A".into(), "2.0.0".into())
        );
        package.releases = vec![release("3.0.0-rc.1"), release("1.0.1")];
        assert_eq!(
            package.version_lanes(),
            ("1.0.1".into(), "1.0.1".into(), "1.0.1".into())
        );
        package.current = "workspace:*".into();
        assert_eq!(
            package.version_lanes(),
            ("N/A".into(), "N/A".into(), "N/A".into())
        );
    }

    #[test]
    fn changelog_prefers_github_releases() {
        let mut package = package();
        assert_eq!(package.changelog_url(), None);
        package.homepage = Some("https://lib.dev".into());
        assert_eq!(package.changelog_url().as_deref(), Some("https://lib.dev"));
        package.repo = Some("https://gitlab.com/x/lib".into());
        assert_eq!(package.changelog_url().as_deref(), Some("https://lib.dev"));
        package.repo = Some("https://github.com/x/lib/".into());
        assert_eq!(
            package.changelog_url().as_deref(),
            Some("https://github.com/x/lib/releases")
        );
    }

    #[test]
    fn applying_metadata_classifies_releases_against_current() {
        let mut package = package();
        let mut current = release("1.0.0");
        current.published = Some(Utc::now());
        package.apply_metadata(Metadata {
            description: Some("desc".into()),
            license: None,
            repo: None,
            homepage: None,
            published: None,
            downloads: Some(5),
            releases: vec![release("2.0.0"), release("1.1.0"), current],
        });
        let changes: Vec<_> = package.releases.iter().map(|r| r.change).collect();
        assert_eq!(changes, [Change::Major, Change::Minor, Change::Unknown]);
        assert!(package.current_published.is_some());
        assert_eq!(package.description.as_deref(), Some("desc"));
        assert_eq!(package.downloads, Some(5));
    }
}
