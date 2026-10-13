//! Results of background tasks, applied to the app state on the UI loop.

use super::{App, MetadataStatus};
use crate::{
    package::Metadata,
    pm::{self, Pm, ScanResult},
};

pub enum Msg {
    Scan(Result<ScanResult, String>),
    Metadata(usize, Result<Metadata, String>),
    PmVersion(String),
    NodeVersion(String),
    Update(Result<String, String>),
}

impl App {
    pub fn handle_message(&mut self, msg: Msg) {
        match msg {
            Msg::Scan(result) => self.on_scan(result),
            Msg::Metadata(index, result) => self.on_metadata(index, result),
            Msg::PmVersion(version) => {
                if self.project.pm == Pm::Yarn {
                    if let Some(major) = pm::yarn_major(&version) {
                        self.project.yarn_berry = major >= 2;
                    }
                }
                self.project.pm_version = version;
            }
            Msg::NodeVersion(version) => self.project.node_version = version,
            Msg::Update(result) => {
                self.updating = false;
                match result {
                    Ok(summary) => {
                        self.status_after_refresh = Some(summary);
                        self.error = None;
                        self.refresh();
                    }
                    Err(error) => {
                        self.status = "Update failed · press e for error".into();
                        self.error = Some(error);
                    }
                }
            }
        }
    }

    fn on_scan(&mut self, result: Result<ScanResult, String>) {
        self.loading = false;
        let ScanResult { packages, warning } = match result {
            Ok(scan) => scan,
            Err(error) => {
                self.status_after_refresh = None;
                self.status = error.clone();
                self.error = Some(error);
                return;
            }
        };
        self.packages = packages;
        self.metadata.clear();
        self.installed_versions.borrow_mut().clear();
        self.status = self
            .status_after_refresh
            .take()
            .unwrap_or_else(|| format!("{} outdated", self.packages.len()));
        if warning.is_some() {
            self.status.push_str(" · warning: press e for details");
        }
        self.error = warning;
        if !self.packages.is_empty() {
            self.selected = self.selected.min(self.packages.len() - 1);
            for index in 0..self.packages.len() {
                self.request_metadata(index);
            }
        }
    }

    fn on_metadata(&mut self, index: usize, result: Result<Metadata, String>) {
        match result {
            Ok(metadata) => {
                self.metadata.insert(index, MetadataStatus::Loaded);
                if let Some(package) = self.packages.get_mut(index) {
                    package.apply_metadata(metadata);
                }
            }
            // Kept distinct from "no data" so the user can retry.
            Err(error) => {
                self.metadata.insert(index, MetadataStatus::Failed(error));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::fixtures::app,
        package::fixtures::{package, release},
    };

    fn metadata() -> Metadata {
        Metadata {
            description: None,
            license: None,
            repo: None,
            homepage: None,
            published: None,
            downloads: None,
            releases: vec![release("2.0.0")],
        }
    }

    #[tokio::test]
    async fn scan_results_replace_packages_and_status() {
        let mut app = app();
        app.loading = true;
        app.selected = 5;
        app.handle_message(Msg::Scan(Ok(ScanResult {
            packages: vec![package(), package()],
            warning: Some("stale".into()),
        })));
        assert!(!app.loading);
        assert_eq!(app.selected, 1);
        assert_eq!(app.status, "2 outdated · warning: press e for details");
        assert_eq!(app.error.as_deref(), Some("stale"));
        // Every package's metadata is requested right away.
        assert!(app
            .metadata
            .values()
            .all(|status| matches!(status, MetadataStatus::Loading)));
        assert_eq!(app.metadata.len(), 2);

        app.handle_message(Msg::Scan(Err("npm exploded".into())));
        assert_eq!(app.status, "npm exploded");
        assert_eq!(app.packages.len(), 2);
    }

    #[test]
    fn metadata_failure_allows_retry_and_success_applies() {
        let mut app = app();
        app.packages = vec![package()];
        app.metadata.insert(0, MetadataStatus::Loading);
        app.handle_message(Msg::Metadata(0, Err("timed out".into())));
        assert_eq!(app.selected_metadata_error(), Some("timed out"));

        app.handle_message(Msg::Metadata(0, Ok(metadata())));
        assert_eq!(app.selected_metadata_error(), None);
        assert_eq!(app.packages[0].releases.len(), 1);
    }

    #[tokio::test]
    async fn successful_update_rescans_with_its_summary() {
        let mut app = app();
        app.updating = true;
        app.error = Some("old".into());
        app.handle_message(Msg::Update(Ok("npm update lib succeeded".into())));
        assert!(!app.updating);
        assert!(app.loading);
        assert_eq!(app.error, None);
        app.handle_message(Msg::Scan(Ok(ScanResult {
            packages: vec![],
            warning: None,
        })));
        assert_eq!(app.status, "npm update lib succeeded");

        app.handle_message(Msg::Update(Err("EACCES".into())));
        assert_eq!(app.status, "Update failed · press e for error");
        assert_eq!(app.error.as_deref(), Some("EACCES"));
    }

    #[test]
    fn yarn_version_decides_berry() {
        let mut app = app();
        app.project.pm = Pm::Yarn;
        app.handle_message(Msg::PmVersion("4.1.0".into()));
        assert!(app.project.yarn_berry);
        app.handle_message(Msg::PmVersion("1.22.22".into()));
        assert!(!app.project.yarn_berry);
        app.handle_message(Msg::PmVersion("N/A".into()));
        assert!(!app.project.yarn_berry);
        assert_eq!(app.project.pm_version, "N/A");
    }
}
