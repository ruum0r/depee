//! Application state and the operations the UI and key handlers perform on it.

mod input;
mod message;
mod tasks;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, Semaphore};

pub use message::Msg;

use crate::{
    package::Package,
    pm::{self, Pm, UpdateRequest},
    project::{self, Manifest},
};

/// Registry metadata requests running at once. They share one HTTP client
/// and its connection pool.
const METADATA_CONCURRENCY: usize = 8;

/// Facts about the inspected project and its toolchain.
pub struct ProjectInfo {
    pub cwd: PathBuf,
    pub name: String,
    pub pm: Pm,
    pub pm_version: String,
    pub yarn_berry: bool,
    pub node_version: String,
    /// Declared dependencies, outdated or not.
    pub total: usize,
    /// Set when `cwd` is a member below a workspace root.
    pub workspace: Option<Workspace>,
}

/// The workspace enclosing a member project. Package manager detection uses
/// its root; scans and updates still run in the member directory.
pub struct Workspace {
    pub root: PathBuf,
    /// The member directory relative to `root`, `/`-separated.
    pub member: String,
}

impl ProjectInfo {
    pub fn load(cwd: &Path, pm_override: Option<Pm>) -> Result<Self> {
        let cwd = cwd
            .canonicalize()
            .with_context(|| format!("Project directory not found: {}", cwd.display()))?;
        let manifest = Manifest::read(&cwd)?;
        let workspace = project::find_workspace_root(&cwd).and_then(|root| {
            let member = cwd.strip_prefix(&root).ok()?;
            let member = member
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            (!member.is_empty()).then_some(Workspace { root, member })
        });
        // Lockfiles and Yarn settings live in the workspace root. The member's
        // own `packageManager` field wins over the root's.
        let pm_dir = workspace
            .as_ref()
            .map_or(cwd.as_path(), |w| w.root.as_path());
        let field = manifest.package_manager.clone().or_else(|| {
            let root = &workspace.as_ref()?.root;
            Manifest::read(root).ok()?.package_manager
        });
        let pm = Pm::detect(pm_override, field.as_deref(), pm_dir)?;
        let yarn_berry = pm == Pm::Yarn && pm::detect_yarn_berry(field.as_deref(), pm_dir);
        let total = manifest.dependency_count();
        let name = manifest.name.unwrap_or_else(|| {
            cwd.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_owned()
        });
        Ok(Self {
            cwd,
            name,
            pm,
            pm_version: "checking…".into(),
            yarn_berry,
            node_version: "checking…".into(),
            total,
            workspace,
        })
    }
}

/// What the main screen is doing; each mode owns its key bindings and footer.
pub enum Mode {
    Browse,
    /// Typing a package name filter.
    Search(String),
    /// Choosing a release of the selected package.
    VersionPicker,
    Confirm(Confirm),
}

#[derive(Default)]
pub struct Confirm {
    pub scroll: u16,
    /// Furthest useful scroll offset, recorded by the last draw.
    pub scroll_max: Cell<u16>,
    /// Version chosen in the version picker; it forces an upgrade to itself.
    pub pinned: Option<String>,
}

pub enum MetadataStatus {
    Loading,
    Loaded,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    Change,
    Published,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Change,
            Self::Change => Self::Published,
            Self::Published => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Change => "Change",
            Self::Published => "Published",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateChoice {
    Now,
    Skip,
    Later,
}

impl UpdateChoice {
    pub const ALL: [Self; 3] = [Self::Now, Self::Skip, Self::Later];

    pub fn label(self) -> &'static str {
        match self {
            Self::Now => "Update now",
            Self::Skip => "Skip this version",
            Self::Later => "Remind me later",
        }
    }
}

/// Startup screen offering a newer depee release.
pub struct UpdatePrompt {
    pub version: String,
    pub selected: usize,
}

pub struct App {
    pub project: ProjectInfo,
    /// Outdated packages, sorted by name; `selected` indexes into this.
    pub packages: Vec<Package>,
    pub selected: usize,
    pub mode: Mode,
    pub sort: Sort,
    pub descending: bool,
    /// Target Latest instead of Wanted; implies an upgrade.
    pub target_latest: bool,
    /// Upgrade (rewrite the manifest range) instead of a compatible update.
    pub upgrade: bool,
    pub loading: bool,
    pub updating: bool,
    pub status: String,
    /// Replaces the default status once the next scan finishes.
    status_after_refresh: Option<String>,
    pub error: Option<String>,
    pub update_prompt: Option<UpdatePrompt>,
    /// Release to install after the terminal is restored.
    pub install_update: Option<String>,
    pub should_quit: bool,
    /// Registry metadata state by package index; absent means not requested.
    pub metadata: HashMap<usize, MetadataStatus>,
    // `node_modules` lookups made while drawing; cleared whenever a scan finishes.
    installed_versions: RefCell<HashMap<String, Option<String>>>,
    metadata_limit: Arc<Semaphore>,
    tx: mpsc::UnboundedSender<Msg>,
}

impl App {
    pub fn new(project: ProjectInfo) -> (Self, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let app = Self {
            project,
            packages: vec![],
            selected: 0,
            mode: Mode::Browse,
            sort: Sort::Name,
            descending: false,
            target_latest: false,
            upgrade: false,
            loading: false,
            updating: false,
            status: String::new(),
            status_after_refresh: None,
            error: None,
            update_prompt: None,
            install_update: None,
            should_quit: false,
            metadata: HashMap::new(),
            installed_versions: RefCell::default(),
            metadata_limit: Arc::new(Semaphore::new(METADATA_CONCURRENCY)),
            tx,
        };
        (app, rx)
    }

    pub fn selected_package(&self) -> Option<&Package> {
        self.packages.get(self.selected)
    }

    pub fn query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Search(query) => Some(query),
            _ => None,
        }
    }

    pub fn confirm(&self) -> Option<&Confirm> {
        match &self.mode {
            Mode::Confirm(confirm) => Some(confirm),
            _ => None,
        }
    }

    pub fn selected_metadata_error(&self) -> Option<&str> {
        match self.metadata.get(&self.selected) {
            Some(MetadataStatus::Failed(error)) => Some(error),
            _ => None,
        }
    }

    fn metadata_pending(&self) -> bool {
        self.metadata
            .values()
            .any(|status| matches!(status, MetadataStatus::Loading))
    }

    /// Publication dates arrive with metadata, so sorting by them waits for it.
    pub fn active_sort(&self) -> Sort {
        if self.sort == Sort::Published && self.metadata_pending() {
            Sort::Name
        } else {
            self.sort
        }
    }

    /// Indexes of the packages to list, filtered and in display order.
    pub fn visible(&self) -> Vec<usize> {
        let query = self.query().map(str::to_lowercase);
        let mut ids: Vec<usize> = self
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                query
                    .as_ref()
                    .is_none_or(|q| p.name.to_lowercase().contains(q))
            })
            .map(|(i, _)| i)
            .collect();
        let sort = self.active_sort();
        ids.sort_by(|a, b| {
            let (a, b) = (&self.packages[*a], &self.packages[*b]);
            let primary = match sort {
                Sort::Name => a.name.cmp(&b.name),
                Sort::Change => a.change.rank().cmp(&b.change.rank()),
                // `None` sorts after any date.
                Sort::Published => match (a.published, b.published) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (a, b) => b.is_some().cmp(&a.is_some()),
                },
            };
            // Keep packages without a publication date last in either direction.
            let keeps_direction =
                sort == Sort::Published && a.published.is_some() != b.published.is_some();
            let primary = if self.descending && !keeps_direction {
                primary.reverse()
            } else {
                primary
            };
            primary.then_with(|| a.name.cmp(&b.name))
        });
        ids
    }

    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let position = visible
            .iter()
            .position(|x| *x == self.selected)
            .unwrap_or(0);
        self.selected = visible[input::step(position, delta, visible.len())];
    }

    pub fn open_confirm(&mut self, pinned: Option<String>) {
        self.mode = Mode::Confirm(Confirm {
            pinned,
            ..Confirm::default()
        });
    }

    pub fn close_confirm(&mut self) {
        self.mode = Mode::Browse;
    }

    pub fn pinned_version(&self) -> Option<&str> {
        self.confirm().and_then(|confirm| confirm.pinned.as_deref())
    }

    /// The version the user asked for: a pinned version, else Latest or Wanted.
    pub fn target(&self) -> Option<String> {
        if let Some(pinned) = self.pinned_version() {
            return Some(pinned.to_string());
        }
        let p = self.selected_package()?;
        if self.target_latest {
            p.latest.clone().or_else(|| p.wanted.clone())
        } else {
            p.wanted.clone().or_else(|| p.latest.clone())
        }
    }

    // Latest and explicitly picked versions can only be reached by an upgrade,
    // so they override the Compatible/Upgrade toggle without changing it.
    pub fn upgrading(&self) -> bool {
        self.upgrade || self.target_latest || self.pinned_version().is_some()
    }

    /// The version the update is expected to install.
    pub fn effective_target(&self) -> Option<String> {
        let package = self.selected_package()?;
        let target = if self.upgrading() {
            self.target()
        } else {
            package.wanted.clone()
        };
        Some(target.unwrap_or_else(|| package.current.clone()))
    }

    pub fn update_request(&self) -> Option<UpdateRequest> {
        let p = self.selected_package()?;
        Some(UpdateRequest {
            name: p.name.clone(),
            kind: p.kind,
            current_spec: p.current_spec.clone(),
            requested_target: self.target().unwrap_or_else(|| p.current.clone()),
            effective_target: self.effective_target()?,
            upgrade: self.upgrading(),
            yarn_berry: self.project.yarn_berry,
        })
    }

    pub fn installed_version(&self, package: &str) -> Option<String> {
        self.installed_versions
            .borrow_mut()
            .entry(package.to_string())
            .or_insert_with(|| project::installed_version(&self.project.cwd, package))
            .clone()
    }
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub fn app() -> App {
        let project = ProjectInfo {
            cwd: PathBuf::from("."),
            name: "test".into(),
            pm: Pm::Npm,
            pm_version: String::new(),
            yarn_berry: false,
            node_version: String::new(),
            total: 0,
            workspace: None,
        };
        App::new(project).0
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{TimeZone, Utc};

    use super::{fixtures::app, *};
    use crate::package::{fixtures::package, Change};

    fn named(name: &str, change: Change, published_day: Option<u32>) -> Package {
        let mut p = package();
        p.name = name.into();
        p.change = change;
        p.published = published_day.map(|d| Utc.with_ymd_and_hms(2025, 1, d, 0, 0, 0).unwrap());
        p
    }

    fn visible_names(app: &App) -> Vec<&str> {
        app.visible()
            .into_iter()
            .map(|i| app.packages[i].name.as_str())
            .collect()
    }

    #[test]
    fn visible_sorts_and_filters() {
        let mut app = app();
        app.packages = vec![
            named("alpha", Change::Patch, Some(3)),
            named("beta", Change::Major, None),
            named("gamma", Change::Minor, Some(1)),
        ];
        assert_eq!(visible_names(&app), ["alpha", "beta", "gamma"]);
        app.sort = Sort::Change;
        assert_eq!(visible_names(&app), ["alpha", "gamma", "beta"]);
        app.descending = true;
        assert_eq!(visible_names(&app), ["beta", "gamma", "alpha"]);
        // Packages without a date stay last in both directions.
        app.sort = Sort::Published;
        assert_eq!(visible_names(&app), ["alpha", "gamma", "beta"]);
        app.descending = false;
        assert_eq!(visible_names(&app), ["gamma", "alpha", "beta"]);
        // Dates are incomplete while metadata loads, so Name is used meanwhile.
        app.metadata.insert(1, MetadataStatus::Loading);
        assert_eq!(app.active_sort(), Sort::Name);
        assert_eq!(visible_names(&app), ["alpha", "beta", "gamma"]);
        app.mode = Mode::Search("MM".into());
        assert_eq!(visible_names(&app), ["gamma"]);
    }

    #[test]
    fn move_selection_follows_display_order_and_clamps() {
        let mut app = app();
        app.packages = vec![
            named("a", Change::Patch, None),
            named("b", Change::Major, None),
        ];
        app.sort = Sort::Change;
        app.descending = true;
        app.selected = 1;
        app.move_selection(1);
        assert_eq!(app.selected, 0);
        app.move_selection(5);
        assert_eq!(app.selected, 0);
        app.move_selection(-1);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn installed_versions_are_cached_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let package_dir = dir.path().join("node_modules/@scope/peer");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();
        let mut app = app();
        app.project.cwd = dir.path().join("nested");
        assert_eq!(
            app.installed_version("@scope/peer").as_deref(),
            Some("1.0.0")
        );
        fs::write(package_dir.join("package.json"), r#"{"version":"2.0.0"}"#).unwrap();
        assert_eq!(
            app.installed_version("@scope/peer").as_deref(),
            Some("1.0.0")
        );
        app.installed_versions.borrow_mut().clear();
        assert_eq!(
            app.installed_version("@scope/peer").as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn workspace_members_detect_from_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        fs::write(root.join("yarn.lock"), "__metadata:\n  version: 8\n").unwrap();
        let member = root.join("packages/web");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("package.json"), r#"{"name":"web"}"#).unwrap();

        let project = ProjectInfo::load(&member, None).unwrap();
        assert_eq!(project.pm, Pm::Yarn);
        assert!(project.yarn_berry);
        assert_eq!(project.cwd, member.canonicalize().unwrap());
        let workspace = project.workspace.unwrap();
        assert_eq!(workspace.member, "packages/web");
        assert_eq!(workspace.root, root.canonicalize().unwrap());

        // The root's packageManager field beats its lockfiles; the member's
        // own field beats the root's.
        fs::write(
            root.join("package.json"),
            r#"{"workspaces":["packages/*"],"packageManager":"pnpm@9.0.0"}"#,
        )
        .unwrap();
        assert_eq!(ProjectInfo::load(&member, None).unwrap().pm, Pm::Pnpm);
        fs::write(
            member.join("package.json"),
            r#"{"name":"web","packageManager":"bun@1.2.0"}"#,
        )
        .unwrap();
        assert_eq!(ProjectInfo::load(&member, None).unwrap().pm, Pm::Bun);
        assert_eq!(
            ProjectInfo::load(&member, Some(Pm::Npm)).unwrap().pm,
            Pm::Npm
        );

        // At the root itself there is no member to show.
        let project = ProjectInfo::load(root, None).unwrap();
        assert_eq!(project.pm, Pm::Pnpm);
        assert!(project.workspace.is_none());
    }

    #[test]
    fn standalone_projects_detect_from_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("yarn.lock"), "").unwrap();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("package.json"), "{}").unwrap();
        // Without a workspace declaration a parent lockfile is not used.
        let project = ProjectInfo::load(&nested, None).unwrap();
        assert_eq!(project.pm, Pm::Npm);
        assert!(project.workspace.is_none());
    }

    #[test]
    fn update_request_reflects_mode_and_target() {
        let mut app = app();
        app.packages = vec![package()];
        let request = app.update_request().unwrap();
        assert_eq!(
            (
                request.upgrade,
                &*request.requested_target,
                &*request.effective_target
            ),
            (false, "1.1.0", "1.1.0")
        );
        app.target_latest = true;
        let request = app.update_request().unwrap();
        assert_eq!(
            (
                request.upgrade,
                &*request.requested_target,
                &*request.effective_target
            ),
            (true, "2.0.0", "2.0.0")
        );
        app.open_confirm(Some("1.5.0".into()));
        let request = app.update_request().unwrap();
        assert_eq!(
            (request.upgrade, &*request.requested_target),
            (true, "1.5.0")
        );
    }
}
