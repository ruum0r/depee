//! Key bindings: one handler per screen or mode.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Mode, UpdateChoice};
use crate::self_update;

pub const HELP: &str =
    "↑↓/jk move · s cycle sort · S reverse · v version · u update · c changelog · r refresh · q quit";

/// Up/down movement shared by every list: ↑/↓ and Ctrl+p/Ctrl+n, plus j/k when
/// letters are not being typed into a text field.
pub fn nav_delta(k: &KeyEvent, letters: bool) -> Option<isize> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Up => Some(-1),
        KeyCode::Down => Some(1),
        KeyCode::Char('p') if ctrl => Some(-1),
        KeyCode::Char('n') if ctrl => Some(1),
        KeyCode::Char('k') if letters && !ctrl => Some(-1),
        KeyCode::Char('j') if letters && !ctrl => Some(1),
        _ => None,
    }
}

/// Moves `index` by `delta`, clamped to `0..len`.
pub fn step(index: usize, delta: isize, len: usize) -> usize {
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

impl App {
    pub fn handle_key(&mut self, k: KeyEvent) -> Result<()> {
        // Ctrl+c quits from every screen, before any screen-specific handling.
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Ok(());
        }
        if self.update_prompt.is_some() {
            self.handle_update_prompt_key(k);
            return Ok(());
        }
        match self.mode {
            Mode::Browse => self.handle_browse_key(k)?,
            Mode::Search(_) => self.handle_search_key(k),
            Mode::VersionPicker => self.handle_version_picker_key(k),
            Mode::Confirm(_) => self.handle_confirm_key(k),
            Mode::WorkspacePicker(_) => self.handle_workspace_picker_key(k),
        }
        Ok(())
    }

    fn handle_workspace_picker_key(&mut self, k: KeyEvent) {
        let len = self
            .project
            .workspace
            .as_ref()
            .map_or(0, |w| w.members.len());
        let Mode::WorkspacePicker(picker) = &mut self.mode else {
            return;
        };
        if let Some(d) = nav_delta(&k, true) {
            picker.selected = step(picker.selected, d, len);
            return;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        let selected = picker.selected;
        match k.code {
            KeyCode::Enter => self.switch_workspace(selected),
            KeyCode::Esc | KeyCode::Char('w') => self.cancel_workspace_picker(),
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_update_prompt_key(&mut self, k: KeyEvent) {
        let Some(prompt) = &mut self.update_prompt else {
            return;
        };
        if let Some(d) = nav_delta(&k, true) {
            prompt.selected = step(prompt.selected, d, UpdateChoice::ALL.len());
            return;
        }
        match k.code {
            KeyCode::Enter => match UpdateChoice::ALL[prompt.selected] {
                UpdateChoice::Now => {
                    self.install_update = Some(prompt.version.clone());
                    self.should_quit = true;
                }
                UpdateChoice::Skip => {
                    self_update::skip(&prompt.version);
                    self.update_prompt = None;
                }
                UpdateChoice::Later => self.update_prompt = None,
            },
            KeyCode::Esc => self.update_prompt = None,
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_browse_key(&mut self, k: KeyEvent) -> Result<()> {
        if let Some(d) = nav_delta(&k, true) {
            self.move_selection(d);
            return Ok(());
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Enter => {
                self.request_metadata(self.selected);
                if let Some(package) = self.selected_package() {
                    self.status = format!("Details: {}", package.name);
                }
            }
            KeyCode::Char('/') => self.mode = Mode::Search(String::new()),
            KeyCode::Char('s') if !shift => self.sort = self.sort.next(),
            KeyCode::Char('S') | KeyCode::Char('s') => self.descending = !self.descending,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('u') => self.open_confirm(None),
            KeyCode::Char('v') => {
                self.mode = Mode::VersionPicker;
                self.request_metadata(self.selected);
            }
            KeyCode::Char('c') => self.open_changelog()?,
            KeyCode::Char('w') if self.project.workspace.is_some() => {
                if self.updating {
                    self.status = "Wait for the update to finish before switching members".into();
                } else {
                    self.open_workspace_picker(false);
                }
            }
            KeyCode::Char('e') => {
                if let Some(error) = &self.error {
                    self.status = error.clone();
                }
            }
            KeyCode::Char('?') => {
                self.status = HELP.into();
                if self.project.workspace.is_some() {
                    self.status.push_str(" · w workspace");
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_search_key(&mut self, k: KeyEvent) {
        if let Some(d) = nav_delta(&k, false) {
            self.move_selection(d);
            return;
        }
        let Mode::Search(query) = &mut self.mode else {
            return;
        };
        match k.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Browse,
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(c);
                self.selected = self.visible().first().copied().unwrap_or(0);
            }
            _ => {}
        }
    }

    fn handle_version_picker_key(&mut self, k: KeyEvent) {
        if let Some(d) = nav_delta(&k, true) {
            if let Some(p) = self.packages.get_mut(self.selected) {
                p.release_index = step(p.release_index, d, p.releases.len());
            }
            return;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('v') => self.mode = Mode::Browse,
            KeyCode::Enter => {
                let pinned = self
                    .selected_package()
                    .and_then(|p| p.releases.get(p.release_index))
                    .map(|r| r.version.clone());
                self.open_confirm(pinned);
            }
            _ => {}
        }
    }

    fn handle_confirm_key(&mut self, k: KeyEvent) {
        let Mode::Confirm(confirm) = &mut self.mode else {
            return;
        };
        if let Some(d) = nav_delta(&k, true) {
            confirm.scroll = if d < 0 {
                confirm.scroll.saturating_sub(1)
            } else {
                confirm
                    .scroll
                    .saturating_add(1)
                    .min(confirm.scroll_max.get())
            };
            return;
        }
        // Letter shortcuts must not fire for Ctrl chords (Ctrl+n is not `n`).
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        let pinned = confirm.pinned.is_some();
        match k.code {
            KeyCode::Enter | KeyCode::Char('y') => self.start_update(),
            KeyCode::Char('m') if !self.target_latest && !pinned => self.upgrade = !self.upgrade,
            KeyCode::Char('l') if !pinned => self.target_latest = !self.target_latest,
            KeyCode::Char('r') if self.selected_metadata_error().is_some() => {
                self.request_metadata(self.selected);
            }
            KeyCode::Esc | KeyCode::Char('n') => self.close_confirm(),
            _ => {}
        }
    }

    fn open_changelog(&mut self) -> Result<()> {
        let Some(url) = self.selected_package().map(|p| p.changelog_url()) else {
            return Ok(());
        };
        self.status = match url {
            Some(url) => {
                open_url(&url)?;
                "Opened package releases".into()
            }
            None => "No repository or homepage URL available".into(),
        };
        Ok(())
    }
}

fn open_url(url: &str) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{fixtures::app, MetadataStatus, Sort, UpdatePrompt, Workspace},
        package::fixtures::{package, release},
    };

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
    }

    fn scroll(app: &App) -> u16 {
        app.confirm().unwrap().scroll
    }

    #[test]
    fn nav_keys_are_shared() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(nav_delta(&key(KeyCode::Up, none), true), Some(-1));
        assert_eq!(nav_delta(&key(KeyCode::Char('p'), ctrl), true), Some(-1));
        assert_eq!(nav_delta(&key(KeyCode::Char('k'), none), true), Some(-1));
        assert_eq!(nav_delta(&key(KeyCode::Down, none), false), Some(1));
        assert_eq!(nav_delta(&key(KeyCode::Char('n'), ctrl), false), Some(1));
        assert_eq!(nav_delta(&key(KeyCode::Char('j'), none), false), None);
        assert_eq!(nav_delta(&key(KeyCode::Char('n'), none), true), None);
        assert_eq!(step(0, -1, 3), 0);
        assert_eq!(step(2, 1, 3), 2);
        assert_eq!(step(0, 1, 0), 0);
    }

    #[test]
    fn ctrl_c_quits_instead_of_opening_changelog() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut browsing = app();
        browsing.handle_key(ctrl_c).unwrap();
        assert!(browsing.should_quit);
        let mut searching = app();
        searching.mode = Mode::Search(String::new());
        searching.handle_key(ctrl_c).unwrap();
        assert!(searching.should_quit);
    }

    #[test]
    fn search_filters_typed_letters_and_selects_first_match() {
        let mut app = app();
        let names = ["alpha", "beta", "jest"];
        app.packages = names
            .iter()
            .map(|name| {
                let mut p = package();
                p.name = (*name).into();
                p
            })
            .collect();
        press(&mut app, KeyCode::Char('/'));
        // `j` is typed into the query rather than moving the selection.
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.query(), Some("j"));
        assert_eq!(app.selected, 2);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.visible(), [1, 2]);
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected, 2);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Browse));
        assert_eq!(app.visible(), [0, 1, 2]);
    }

    fn workspace_app(dir: &std::path::Path) -> App {
        let mut app = app();
        let member = |path: &str| crate::workspace::Member {
            dir: dir.join(path),
            path: path.into(),
            name: None,
        };
        app.project.cwd = dir.to_path_buf();
        app.project.workspace = Some(Workspace {
            root: dir.to_path_buf(),
            member: String::new(),
            members: vec![member(""), member("packages/a"), member("packages/b")],
        });
        app
    }

    fn picked(app: &App) -> usize {
        app.workspace_picker().unwrap().selected
    }

    #[test]
    fn workspace_picker_moves_and_cancels() {
        // `w` does nothing outside a workspace.
        let mut standalone = app();
        press(&mut standalone, KeyCode::Char('w'));
        assert!(matches!(standalone.mode, Mode::Browse));

        let dir = tempfile::tempdir().unwrap();
        let mut app = workspace_app(dir.path());
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(picked(&app), 0);
        assert!(!app.workspace_picker().unwrap().startup);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(picked(&app), 2);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(picked(&app), 1);
        // Ctrl chords are not letter shortcuts.
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
        assert!(!app.loading);

        // The member directory is missing, so the switch fails visibly and
        // the project stays the root.
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app
            .status
            .contains("Workspace member not found: packages/a"));
        assert_eq!(app.project.workspace.as_ref().unwrap().member, "");

        // While updating, the picker does not open.
        app.updating = true;
        press(&mut app, KeyCode::Char('w'));
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[tokio::test]
    async fn startup_picker_falls_back_to_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = workspace_app(dir.path());
        app.open_workspace_picker(true);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
        assert_eq!(app.project.workspace.as_ref().unwrap().member, "");
        assert!(app.loading, "the root is scanned instead");
    }

    #[test]
    fn sort_keys_cycle_and_reverse() {
        let mut app = app();
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.sort, Sort::Change);
        app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT))
            .unwrap();
        assert!(app.descending);
        assert_eq!(app.sort, Sort::Change);
    }

    #[test]
    fn update_prompt_choices() {
        let prompt = || {
            Some(UpdatePrompt {
                version: "9.9.9".into(),
                selected: 0,
            })
        };
        let mut update_now = app();
        update_now.update_prompt = prompt();
        press(&mut update_now, KeyCode::Enter);
        assert_eq!(update_now.install_update.as_deref(), Some("9.9.9"));
        assert!(update_now.should_quit);

        let mut later = app();
        later.update_prompt = prompt();
        for _ in 0..3 {
            press(&mut later, KeyCode::Char('j'));
        }
        assert_eq!(later.update_prompt.as_ref().unwrap().selected, 2);
        press(&mut later, KeyCode::Enter);
        assert!(later.update_prompt.is_none());
        assert!(!later.should_quit);
        assert_eq!(later.install_update, None);
    }

    #[test]
    fn confirm_mode_overrides_do_not_stick() {
        let mut app = app();
        let mut package = package();
        package.releases = vec![release("2.0.0"), release("1.5.0")];
        package.release_index = 1;
        app.packages = vec![package];
        // Pretend metadata was already loaded so `v` does not spawn a fetch.
        app.metadata.insert(0, MetadataStatus::Loaded);

        // Latest forces Upgrade and locks `m`; turning it off restores Compatible.
        press(&mut app, KeyCode::Char('u'));
        assert!(!app.upgrading());
        assert_eq!(app.effective_target().as_deref(), Some("1.1.0"));
        press(&mut app, KeyCode::Char('l'));
        assert!(app.upgrading());
        assert_eq!(app.effective_target().as_deref(), Some("2.0.0"));
        press(&mut app, KeyCode::Char('m'));
        assert!(!app.upgrade);
        press(&mut app, KeyCode::Char('l'));
        assert!(!app.upgrading());
        assert_eq!(app.effective_target().as_deref(), Some("1.1.0"));

        // Scrolling is bounded by the last draw and reset on reopen.
        app.confirm().unwrap().scroll_max.set(2);
        for _ in 0..5 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(scroll(&app), 2);
        press(&mut app, KeyCode::Esc);
        assert!(app.confirm().is_none());
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(scroll(&app), 0);
        press(&mut app, KeyCode::Esc);

        // A picked version forces Upgrade only until the confirmation is dismissed.
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.confirm().unwrap().pinned.as_deref(), Some("1.5.0"));
        assert!(app.upgrading());
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('m'));
        assert!(!app.target_latest);
        assert!(!app.upgrade);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.confirm().unwrap().pinned, None);
        assert!(!app.upgrading());
    }

    #[test]
    fn confirm_scroll_accepts_every_nav_key_and_ignores_ctrl_letters() {
        let mut app = app();
        app.packages = vec![package()];
        app.open_confirm(None);
        app.confirm().unwrap().scroll_max.set(10);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
        for k in [plain(KeyCode::Down), plain(KeyCode::Char('j')), ctrl('n')] {
            app.handle_key(k).unwrap();
        }
        assert_eq!(scroll(&app), 3);
        app.handle_key(ctrl('p')).unwrap();
        assert_eq!(scroll(&app), 2);
        // Ctrl+n scrolls instead of cancelling; Ctrl+y/l/m do not act as y/l/m.
        for c in ['y', 'l', 'm'] {
            app.handle_key(ctrl(c)).unwrap();
        }
        assert!(app.confirm().is_some());
        assert!(!app.updating);
        assert!(!app.target_latest);
        assert!(!app.upgrade);
        app.handle_key(plain(KeyCode::Char('k'))).unwrap();
        app.handle_key(plain(KeyCode::Up)).unwrap();
        assert_eq!(scroll(&app), 0);
        app.handle_key(plain(KeyCode::Char('n'))).unwrap();
        assert!(app.confirm().is_none());
    }
}
