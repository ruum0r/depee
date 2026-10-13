//! The update confirmation pane: what will run, and whether the target looks compatible.

use ratatui::{layout::Rect, style::Color, text::Span, widgets::Paragraph, Frame};

use super::{
    details::{draw_empty, pane_block},
    text::{ago, count, or_na, pad, DetailLines},
};
use crate::{
    app::{App, MetadataStatus},
    compat::{dependency_diff, requirement_matches},
    package::{Change, Package, Release},
    pm::compatible_update_rewrites_range,
};

/// Peer dependencies listed before the rest are summarized.
const PEER_LIMIT: usize = 5;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let block = pane_block(" UPDATE CONFIRMATION ");
    let (Some(p), Some(confirm)) = (app.selected_package(), app.confirm()) else {
        draw_empty(f, app, block, area);
        return;
    };
    let inner = block.inner(area);
    let lines = confirmation_lines(app, p, inner.width.max(1) as usize);
    let scroll_max =
        u16::try_from(lines.len().saturating_sub(inner.height as usize)).unwrap_or(u16::MAX);
    confirm.scroll_max.set(scroll_max);
    f.render_widget(
        Paragraph::new(lines.into_lines())
            .scroll((confirm.scroll.min(scroll_max), 0))
            .block(block),
        area,
    );
}

fn confirmation_lines(app: &App, p: &Package, width: usize) -> DetailLines {
    let mut lines = DetailLines::new(width);
    let Some(request) = app.update_request() else {
        return lines;
    };
    let pm = app.project.pm;
    let target = &request.requested_target;
    let effective_target = &request.effective_target;
    let target_change = Change::between(&p.current, Some(effective_target)).label();
    let target_source = if app.pinned_version().is_some() {
        "Selected version"
    } else if app.target_latest {
        "Latest"
    } else {
        "Wanted"
    };
    let package_json_change = if request.upgrade {
        format!("{} → updated for {target}", p.current_spec)
    } else if compatible_update_rewrites_range(pm) {
        format!(
            "{} → lower bound raised to {effective_target} (still in range)",
            p.current_spec
        )
    } else {
        format!("{} (unchanged)", p.current_spec)
    };

    lines.bold(format!("Update {}?", p.name));
    lines.blank();
    lines.section("UPDATE IMPACT");
    lines.bold(format!(
        " {} → {effective_target}   {target_change}",
        p.current
    ));
    lines.field("Selected target", &format!("{target_source} · {target}"));
    lines.field(
        "Effective update",
        if request.upgrade {
            "Selected target is passed to command"
        } else {
            "Wanted · within existing range"
        },
    );
    lines.field(
        "Mode",
        if request.upgrade {
            "Upgrade"
        } else {
            "Compatible Update"
        },
    );
    lines.field("package.json", &package_json_change);
    lines.blank();
    lines.section("VERSION");
    lines.field("Current", &p.current);
    lines.field("Wanted", or_na(p.wanted.as_deref()));
    lines.field("Latest", or_na(p.latest.as_deref()));
    lines.field("Target", effective_target);
    lines.field("Change", target_change);
    lines.field("Latest change", p.change.label());
    lines.blank();
    lines.section("PUBLICATION");
    lines.field("Current published", &ago(p.current_published));
    lines.field("Latest published", &ago(p.published));
    lines.blank();
    lines.section("PACKAGE");
    lines.field("Downloads", &format!("{} / week", count(p.downloads)));
    lines.field("License", or_na(p.license.as_deref()));
    lines.field("Repository", or_na(p.repo.as_deref()));
    lines.field("Homepage", or_na(p.homepage.as_deref()));
    lines.blank();
    lines.section("UPDATE");
    lines.field("Package manager", pm.name());
    lines.field("Command", &request.command_line(pm));
    push_inspection(&mut lines, app, p);
    lines
}

// Status marks, so states never rely on color alone:
// ✓ compatible · ! needs review · ✗ incompatible · ? unknown · - not specified
fn push_inspection(lines: &mut DetailLines, app: &App, p: &Package) {
    lines.blank();
    lines.section("VERSION COMPATIBILITY");
    match app.metadata.get(&app.selected) {
        Some(MetadataStatus::Loading) => {
            lines.push(" … Loading npm Registry metadata");
            return;
        }
        Some(MetadataStatus::Failed(error)) => {
            lines.colored(
                format!(" ✗ Failed to load npm Registry metadata · {error}"),
                Color::Red,
            );
            lines.push(" Press r to retry. Compatibility is not checked.");
            return;
        }
        Some(MetadataStatus::Loaded) | None => {}
    }
    let Some(target) = app
        .effective_target()
        .and_then(|version| p.release(&version))
    else {
        lines.push(" ? Target version metadata unavailable");
        return;
    };
    let current = p.release(&p.current);
    push_node_check(lines, target, &app.project.node_version);
    push_peer_checks(lines, app, target);
    push_deprecation(lines, target);
    push_license_change(lines, current, target);
    lines.blank();
    lines.subsection(&format!(
        "Dependency changes · {} → {}",
        p.current, target.version
    ));
    match current {
        Some(current) => push_dependency_changes(lines, current, target),
        None => lines.push(" ? Current version metadata unavailable"),
    }
}

fn push_node_check(lines: &mut DetailLines, target: &Release, node_version: &str) {
    lines.subsection("Node.js compatibility");
    let Some(range) = target.engines_node.as_deref() else {
        lines.push(" - Node.js requirement not specified");
        return;
    };
    match requirement_matches(range, node_version) {
        Some(true) => lines.colored(
            format!(" ✓ Node.js {node_version} satisfies {range}"),
            Color::Green,
        ),
        Some(false) => lines.colored(
            format!(" ✗ Requires Node.js {range} (current {node_version})"),
            Color::Red,
        ),
        None => lines.colored(
            format!(" ? Cannot check Node.js requirement {range} (current {node_version})"),
            Color::Gray,
        ),
    }
}

/// One row of the peer dependency table.
struct PeerCheck {
    result: &'static str,
    name: String,
    range: String,
    installed: String,
    color: Color,
}

fn push_peer_checks(lines: &mut DetailLines, app: &App, target: &Release) {
    lines.subsection("Peer dependencies");
    let peers = &target.dependency_specs.peer;
    if peers.is_empty() {
        lines.push(" - No peer dependencies");
        return;
    }
    let mut names: Vec<_> = peers.keys().collect();
    names.sort();
    let rows: Vec<PeerCheck> = names
        .into_iter()
        .take(PEER_LIMIT)
        .map(|name| {
            let range = &peers[name];
            let installed = app.installed_version(name);
            let (result, color) = match &installed {
                None => ("? unresolved", Color::Gray),
                Some(version) => match requirement_matches(range, version) {
                    Some(true) => ("✓ satisfies", Color::Green),
                    Some(false) => ("✗ requires", Color::Red),
                    None => ("? cannot check", Color::Gray),
                },
            };
            PeerCheck {
                result,
                name: name.clone(),
                range: range.clone(),
                installed: installed.unwrap_or_else(|| "N/A".into()),
                color,
            }
        })
        .collect();
    // Pad by display width, not bytes, so `✓` lines up with ASCII marks.
    let column_width = |header: &str, cell: fn(&PeerCheck) -> &str| {
        rows.iter()
            .map(|row| Span::raw(cell(row)).width())
            .chain([header.len()])
            .max()
            .unwrap_or(0)
    };
    let result_width = column_width("Result", |row| row.result);
    let name_width = column_width("Package", |row| &row.name);
    let range_width = column_width("Required", |row| &row.range);
    lines.push(format!(
        " {}  {}  {}  Current",
        pad("Result", result_width),
        pad("Package", name_width),
        pad("Required", range_width),
    ));
    for row in &rows {
        lines.colored(
            format!(
                " {}  {}  {}  {}",
                pad(row.result, result_width),
                pad(&row.name, name_width),
                pad(&row.range, range_width),
                row.installed,
            ),
            row.color,
        );
    }
    if peers.len() > PEER_LIMIT {
        lines.push(format!(
            " … and {} more peer dependencies",
            peers.len() - PEER_LIMIT
        ));
    }
}

fn push_deprecation(lines: &mut DetailLines, target: &Release) {
    lines.subsection("Deprecation");
    match target.deprecated.as_deref() {
        Some(message) => lines.colored(format!(" ! Deprecated · {message}"), Color::Yellow),
        None => lines.colored(" ✓ Not deprecated", Color::Green),
    }
}

fn push_license_change(lines: &mut DetailLines, current: Option<&Release>, target: &Release) {
    lines.subsection("License");
    let old = current.and_then(|release| release.license.as_deref());
    let (text, color) = match (old, target.license.as_deref()) {
        (Some(old), Some(new)) if old == new => (format!(" ✓ {new} (unchanged)"), Color::Green),
        (Some(old), Some(new)) => (format!(" ! {old} → {new} (changed)"), Color::Yellow),
        (None, Some(new)) => (format!(" ? unknown → {new} (current unknown)"), Color::Gray),
        (Some(old), None) => (format!(" ? {old} → unknown (target unknown)"), Color::Gray),
        (None, None) => (" ? License information unavailable".into(), Color::Gray),
    };
    lines.colored(text, color);
}

fn push_dependency_changes(lines: &mut DetailLines, current: &Release, target: &Release) {
    let changes = dependency_diff(current, target);
    if changes.is_empty() {
        lines.push(" - No declared dependency changes");
    }
    for change in changes {
        let (name, group) = (&change.name, change.group);
        if let Some(old) = &change.old {
            lines.colored(format!(" - {name} {old} [{group}]"), Color::Red);
        }
        if let Some(new) = &change.new {
            lines.colored(format!(" + {name} {new} [{group}]"), Color::Green);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        app::fixtures::app,
        package::fixtures::{package, release},
        ui::text::plain_text,
    };

    fn inspection_text(app: &App) -> String {
        let mut lines = DetailLines::new(80);
        push_inspection(&mut lines, app, &app.packages[0]);
        plain_text(&lines.into_lines())
    }

    #[test]
    fn inspection_distinguishes_loading_failure_and_missing_metadata() {
        let mut app = app();
        app.packages = vec![package()];

        app.metadata.insert(0, MetadataStatus::Loading);
        let text = inspection_text(&app);
        assert!(text.contains("Loading npm Registry metadata"));
        assert!(!text.contains("unavailable"));

        app.metadata
            .insert(0, MetadataStatus::Failed("timed out".into()));
        let text = inspection_text(&app);
        assert!(text.contains("Failed to load npm Registry metadata · timed out"));
        assert!(!text.contains("unavailable"));
        assert!(!text.contains('✓'));

        app.metadata.insert(0, MetadataStatus::Loaded);
        let text = inspection_text(&app);
        assert!(text.contains("Target version metadata unavailable"));
    }

    #[test]
    fn inspection_marks_incompatible_and_unknown_states() {
        let mut app = app();
        app.project.node_version = "v18.20.0".into();
        let mut package = package();
        let mut current = release("1.0.0");
        current.license = Some("MIT".into());
        let mut target = release("1.1.0");
        target.engines_node = Some(">=20".into());
        target.deprecated = Some("use lib2".into());
        target.dependency_specs.peer =
            HashMap::from([("not-installed-peer".into(), "^1.0.0".into())]);
        package.releases = vec![target, current];
        app.packages = vec![package];
        let dir = tempfile::tempdir().unwrap();
        app.project.cwd = dir.path().to_path_buf();
        let text = inspection_text(&app);
        assert!(text.contains("✗ Requires Node.js >=20 (current v18.20.0)"));
        assert!(text.contains("? unresolved"));
        assert!(text.contains("! Deprecated · use lib2"));
        assert!(text.contains("? MIT → unknown (target unknown)"));
    }

    #[test]
    fn inspection_lists_dependency_changes_as_diff() {
        let mut app = app();
        let mut package = package();
        let mut current = release("1.0.0");
        current.dependency_specs.dependencies = HashMap::from([("a".into(), "^1".into())]);
        let mut target = release("1.1.0");
        target.dependency_specs.dependencies = HashMap::from([("a".into(), "^2".into())]);
        package.releases = vec![target, current];
        app.packages = vec![package];
        let text = inspection_text(&app);
        assert!(text.contains(" - a ^1 [dependencies]\n + a ^2 [dependencies]"));
        assert!(text.contains("✓ Not deprecated"));
        assert!(text.contains(" - No peer dependencies"));
    }
}
