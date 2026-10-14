//! Rendering. Every function here only reads `App`; state changes live in `app`.

mod confirm;
mod details;
mod list;
mod text;
mod update_prompt;
mod workspace;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
    Frame,
};

use crate::app::{App, Mode};

const MIN_WIDTH: u16 = 50;
const MIN_HEIGHT: u16 = 12;
/// From this width the list and details sit side by side.
const WIDE_LAYOUT_WIDTH: u16 = 120;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(
            Paragraph::new(format!(
                "Terminal too small (minimum {MIN_WIDTH} x {MIN_HEIGHT})"
            ))
            .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }
    if let Some(prompt) = &app.update_prompt {
        update_prompt::draw(f, prompt);
        return;
    }
    let [header_area, content_area, footer_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        // Two hint lines plus borders; the second line carries the status.
        Constraint::Length(4),
    ])
    .areas(area);
    draw_header(f, app, header_area);
    draw_content(f, app, content_area);
    draw_footer(f, app, footer_area);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let state = if app.updating {
        "UPDATING"
    } else if app.loading {
        "LOADING"
    } else if app.error.is_some() {
        "ERROR"
    } else {
        "READY"
    };
    let project = &app.project;
    let member = project
        .workspace
        .as_ref()
        .map(|workspace| match workspace.member.as_str() {
            "" => " (workspace: root)".to_string(),
            member => format!(" (workspace: {member})"),
        })
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(format!(
            " depee · {}{member} · {} {} · {} outdated / {} total · {state} ",
            project.name,
            project.pm.name(),
            project.pm_version,
            app.packages.len(),
            project.total,
        ))
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_content(f: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Confirm(_) => return confirm::draw(f, app, area),
        Mode::WorkspacePicker(picker) => return workspace::draw(f, app, picker, area),
        _ => {}
    }
    let [list_area, details_area] = if area.width >= WIDE_LAYOUT_WIDTH {
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area)
    };
    list::draw(f, app, list_area);
    details::draw(f, app, details_area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new(footer_hints(app).to_vec())
            .style(Style::default().fg(Color::Gray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1)),
            ),
        area,
    );
}

fn footer_hints(app: &App) -> [Line<'static>; 2] {
    match &app.mode {
        Mode::Confirm(confirm) => {
            let controls = if confirm.pinned.is_some() {
                "Enter confirm · explicit version uses Upgrade · Esc cancel"
            } else if app.target_latest {
                "Enter confirm · Latest uses Upgrade (m locked) · l Wanted · Esc cancel"
            } else {
                "Enter confirm · l Wanted/Latest · m Compatible/Upgrade · Esc cancel"
            };
            let retry = if app.selected_metadata_error().is_some() {
                " · r retry metadata"
            } else {
                ""
            };
            [
                Line::from(controls),
                Line::from(format!("↑/↓ or j/k scroll confirmation{retry}")),
            ]
        }
        Mode::VersionPicker => [
            Line::from("↑/↓ or j/k select version · Enter choose · Esc cancel"),
            Line::from("Selected version is only applied after update confirmation."),
        ],
        Mode::Search(query) => [
            Line::from(format!(
                "Search: {query} · type to filter · Enter/Esc finish"
            )),
            Line::from("↑/↓ or Ctrl+n/p moves through matching packages."),
        ],
        Mode::WorkspacePicker(picker) => [
            Line::from(if picker.startup {
                "↑/↓ or j/k select member · Enter inspect · Esc use root · q quit"
            } else {
                "↑/↓ or j/k select member · Enter switch · Esc cancel"
            }),
            Line::from(vec![
                Span::raw("Scans and updates run in the chosen member. │ "),
                status(app),
            ]),
        ],
        Mode::Browse => {
            let workspace = if app.project.workspace.is_some() {
                "w Workspace · "
            } else {
                ""
            };
            [
                Line::from(
                    "↑↓ j/k Move · Enter Details · / Search · s Sort · v Versions · u Update",
                ),
                Line::from(vec![
                    Span::raw(format!(
                        "S Reverse · c Changelog · r Refresh · {workspace}q Quit │ "
                    )),
                    status(app),
                ]),
            ]
        }
    }
}

/// Update results start with `✔` (succeeded) or `✘` (failed); the color
/// follows the mark.
fn status(app: &App) -> Span<'static> {
    let span = Span::raw(app.status.clone());
    match app.status.chars().next() {
        Some('✔') => span.style(Style::default().fg(Color::Green)),
        Some('✘') => span.style(Style::default().fg(Color::Red)),
        _ => span,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, Terminal};

    use std::path::PathBuf;

    use super::*;
    use crate::{
        app::{fixtures::app, MetadataStatus, UpdatePrompt, Workspace},
        package::fixtures::{package, release},
        workspace::Member,
    };

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn loaded_app() -> App {
        let mut app = app();
        let mut package = package();
        package.releases = vec![release("2.0.0"), release("1.1.0"), release("1.0.0")];
        app.packages = vec![package];
        app.metadata.insert(0, MetadataStatus::Loaded);
        app
    }

    #[test]
    fn every_mode_renders_at_every_supported_size() {
        let modes: [fn(&mut App); 5] = [
            |_| {},
            |app| app.mode = Mode::Search("li".into()),
            |app| app.mode = Mode::VersionPicker,
            |app| app.open_confirm(None),
            |app| app.open_confirm(Some("1.1.0".into())),
        ];
        for set_mode in modes {
            for (width, height) in [(160, 50), (120, 30), (80, 24), (50, 12), (30, 8)] {
                let mut app = loaded_app();
                set_mode(&mut app);
                render(&app, width, height);
            }
        }
    }

    #[test]
    fn header_shows_the_workspace_member() {
        let mut app = loaded_app();
        assert!(!render(&app, 120, 30).contains("workspace:"));
        app.project.workspace = Some(workspace("packages/web"));
        let screen = render(&app, 120, 30);
        assert!(screen.contains(" depee · test (workspace: packages/web) · npm "));
        assert!(screen.contains("w Workspace · q Quit"));
        app.project.workspace = Some(workspace(""));
        assert!(render(&app, 120, 30).contains(" depee · test (workspace: root) · npm "));
    }

    fn workspace(member: &str) -> Workspace {
        let member_at = |path: &str, name: Option<&str>| Member {
            dir: PathBuf::from("repo").join(path),
            path: path.into(),
            name: name.map(str::to_string),
        };
        Workspace {
            root: "repo".into(),
            member: member.into(),
            members: vec![
                member_at("", Some("mono")),
                member_at("packages/api", None),
                member_at("packages/web", Some("web")),
            ],
        }
    }

    #[test]
    fn workspace_picker_lists_members() {
        let mut app = loaded_app();
        app.project.workspace = Some(workspace("packages/web"));
        app.open_workspace_picker(false);
        for (width, height) in [(160, 50), (80, 24), (50, 12)] {
            let screen = render(&app, width, height);
            assert!(screen.contains("WORKSPACE MEMBERS · 3"), "{screen}");
            // The selected row stays visible even when the list is cut.
            assert!(screen.contains("❯ ● packages/web"), "{screen}");
            assert!(!screen.contains("PACKAGES"));
        }
        let screen = render(&app, 80, 24);
        assert!(screen.contains("Enter switch · Esc cancel"));
        assert!(screen.contains("(root)"));
        assert!(screen.contains("mono"));
        assert!(screen.contains("packages/api"));
        assert!(screen.contains("N/A"));
        // The current member is selected and marked.
        let selected = screen.lines().find(|l| l.contains("❯")).unwrap();
        assert!(selected.contains("● packages/web") && selected.contains("web"));
        app.open_workspace_picker(true);
        assert!(render(&app, 80, 24).contains("Esc use root"));
    }

    #[test]
    fn small_terminals_get_a_notice() {
        assert!(render(&loaded_app(), 49, 20).contains("Terminal too small"));
    }

    #[test]
    fn confirmation_records_scroll_limit_and_shows_command() {
        let mut app = loaded_app();
        app.open_confirm(None);
        let screen = render(&app, 120, 20);
        assert!(screen.contains("UPDATE CONFIRMATION"));
        assert!(screen.contains("Enter confirm · l Wanted/Latest"));
        assert!(app.confirm().unwrap().scroll_max.get() > 0);
        let screen = render(&app, 120, 200);
        assert!(screen.contains("Command             npm update lib"));
        assert_eq!(app.confirm().unwrap().scroll_max.get(), 0);
    }

    #[test]
    fn update_status_color_follows_its_mark() {
        let mut app = loaded_app();
        app.status = "✔ npm install lib@2.0.0 succeeded · requested target reached".into();
        assert_eq!(status(&app).style.fg, Some(Color::Green));
        app.status = "✘ Update failed · press e for error".into();
        assert_eq!(status(&app).style.fg, Some(Color::Red));
        app.status = "1 outdated".into();
        assert_eq!(status(&app).style.fg, None);
    }

    #[test]
    fn update_prompt_replaces_the_main_screen() {
        let mut app = loaded_app();
        app.update_prompt = Some(UpdatePrompt {
            version: "9.9.9".into(),
            selected: 1,
        });
        let screen = render(&app, 80, 24);
        assert!(screen.contains("> Skip this version"));
        assert!(!screen.contains("PACKAGES"));
    }
}
