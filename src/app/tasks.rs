//! Background work started by the app. Each task reports back through a `Msg`.

use tokio::process::Command;

use super::{App, MetadataStatus, Msg, UpdatePrompt};
use crate::{pm, registry, self_update};

impl App {
    /// Kicks off the initial scan, toolchain probes, and the self-update check.
    pub fn start(&mut self) {
        self.spawn_scan();
        let pm = self.project.pm;
        let tx = self.tx.clone();
        let mut pm_version = Command::new(pm.name());
        pm_version.current_dir(&self.project.cwd);
        tokio::spawn(async move {
            let _ = tx.send(Msg::PmVersion(tool_version(pm_version).await));
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let version = tool_version(Command::new("node")).await;
            let _ = tx.send(Msg::NodeVersion(version));
        });
        if self_update::enabled() {
            self.update_prompt = self_update::pending().map(|version| UpdatePrompt {
                version,
                selected: 0,
            });
            tokio::spawn(async {
                let _ = self_update::refresh_cache().await;
            });
        }
    }

    pub fn refresh(&mut self) {
        if !self.loading && !self.updating {
            self.spawn_scan();
        }
    }

    fn spawn_scan(&mut self) {
        self.loading = true;
        self.status = "Loading dependencies…".into();
        let cwd = self.project.cwd.clone();
        let pm = self.project.pm;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = pm::scan(&cwd, pm).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Scan(result));
        });
    }

    /// Fetches registry metadata for one package unless it is loading or loaded.
    pub fn request_metadata(&mut self, index: usize) {
        if matches!(
            self.metadata.get(&index),
            Some(MetadataStatus::Loading | MetadataStatus::Loaded)
        ) {
            return;
        }
        let Some(package) = self.packages.get(index) else {
            return;
        };
        self.metadata.insert(index, MetadataStatus::Loading);
        let name = package.name.clone();
        let tx = self.tx.clone();
        let limit = self.metadata_limit.clone();
        tokio::spawn(async move {
            let Ok(_permit) = limit.acquire_owned().await else {
                return;
            };
            let result = registry::fetch_metadata(&name)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Metadata(index, result));
        });
    }

    pub fn start_update(&mut self) {
        let Some(request) = self.update_request() else {
            return;
        };
        self.close_confirm();
        self.updating = true;
        self.status = "Updating…".into();
        let cwd = self.project.cwd.clone();
        let pm = self.project.pm;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Update(pm::run_update(&cwd, pm, &request).await));
        });
    }
}

/// Output of `<program> --version`, or "N/A" when it cannot be run.
async fn tool_version(mut command: Command) -> String {
    command
        .arg("--version")
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "N/A".into())
}
