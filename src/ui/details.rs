//! The right-hand pane: package details, or the version picker.

use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Row, Table},
    Frame,
};

use super::text::{ago, count, or_na, DetailLines};
use crate::{
    app::{App, Mode},
    package::Package,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let picking = matches!(app.mode, Mode::VersionPicker);
    let block = pane_block(if picking {
        " SELECT TARGET VERSION "
    } else {
        " PACKAGE DETAILS "
    });
    match app.selected_package() {
        None => draw_empty(f, app, block, area),
        Some(p) if picking => draw_version_picker(f, p, block, area),
        Some(p) => draw_package(f, p, block, area),
    }
}

pub fn pane_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .title(title)
}

pub fn draw_empty(f: &mut Frame, app: &App, block: Block, area: Rect) {
    let message = if app.loading {
        "Loading dependencies…"
    } else {
        "No outdated packages"
    };
    f.render_widget(Paragraph::new(message).block(block), area);
}

fn draw_version_picker(f: &mut Frame, p: &Package, block: Block, area: Rect) {
    let items: Vec<ListItem> = p
        .releases
        .iter()
        .enumerate()
        .map(|(i, r)| {
            ListItem::new(format!(
                "{}  {}  {}{}",
                r.version,
                ago(r.published),
                r.change.label(),
                if i == p.release_index {
                    "  ← selected"
                } else {
                    ""
                }
            ))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn draw_package(f: &mut Frame, p: &Package, block: Block, area: Rect) {
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.max(1) as usize;
    // Tall panes get blank lines between sections.
    let expanded = inner.height >= 20;
    let gap = |lines: &mut DetailLines| {
        if expanded {
            lines.blank();
        }
    };
    let wanted = or_na(p.wanted.as_deref());
    let latest = or_na(p.latest.as_deref());
    let current_published = ago(p.current_published);
    let latest_published = ago(p.published);

    let mut lines = DetailLines::new(width);
    lines.bold(p.name.clone());
    if width < 70 {
        lines.field("Current", &p.current);
        lines.field("Wanted", wanted);
        lines.field("Latest", latest);
        lines.field("Change", p.change.label());
        lines.field("Current published", &current_published);
        lines.field("Latest published", &latest_published);
    } else if expanded {
        lines.blank();
        lines.section("VERSION");
        lines.field("Current", &p.current);
        lines.field("Wanted", wanted);
        lines.field("Latest", latest);
        lines.field("Change", p.change.label());
    } else {
        lines.push(format!(" Current: {} · Wanted: {wanted}", p.current));
        lines.push(format!(" Latest: {latest} · Change: {}", p.change.label()));
    }
    // The summary always stays visible; release history yields space to it.
    let summary_height = lines.height();

    gap(&mut lines);
    lines.section("PUBLICATION");
    lines.field("Current published", &current_published);
    lines.field("Latest published", &latest_published);
    gap(&mut lines);
    lines.section("PACKAGE");
    lines.field("Downloads", &format!("{} / week", count(p.downloads)));
    lines.field("License", or_na(p.license.as_deref()));
    lines.field("Repository", or_na(p.repo.as_deref()));
    lines.field("Homepage", or_na(p.homepage.as_deref()));
    lines.field("Description", or_na(p.description.as_deref()));
    if let Some(release) = p.releases.get(p.release_index) {
        gap(&mut lines);
        lines.section(&format!(
            "TARGET DEPENDENCIES ({})",
            release.dependencies.len()
        ));
        lines.value(&if release.dependencies.is_empty() {
            "N/A".to_owned()
        } else {
            release.dependencies.join(", ")
        });
    }

    let desired_releases = if inner.height >= 30 { 10 } else { 5 };
    let history_gap = u16::from(expanded);
    let release_count = desired_releases.min(p.releases.len()).min(
        inner
            .height
            .saturating_sub(summary_height as u16 + history_gap + 2) as usize,
    );
    let history_reserve = if release_count > 0 {
        history_gap + 2 + release_count as u16
    } else {
        0
    };
    let info_height = lines
        .height()
        .min(inner.height.saturating_sub(history_reserve) as usize) as u16;
    f.render_widget(
        Paragraph::new(lines.into_lines()),
        Rect {
            height: info_height,
            ..inner
        },
    );

    let history_y = inner.y + info_height + history_gap;
    let history_height = inner
        .height
        .saturating_sub(info_height)
        .min(2 + release_count as u16);
    if history_height > 0 {
        f.render_widget(
            Paragraph::new(" RELEASE HISTORY").style(Style::default().add_modifier(Modifier::BOLD)),
            Rect {
                y: history_y,
                height: 1,
                ..inner
            },
        );
    }
    if history_height > 1 {
        draw_release_history(
            f,
            p,
            release_count,
            Rect {
                x: inner.x + 1,
                y: history_y + 1,
                width: inner.width.saturating_sub(1),
                height: history_height - 1,
            },
        );
    }
}

fn draw_release_history(f: &mut Frame, p: &Package, release_count: usize, area: Rect) {
    let rows = p.releases.iter().take(release_count).map(|release| {
        Row::new([
            release.version.clone(),
            ago(release.published),
            format!(
                "{}{}",
                release.change.label(),
                if release.prerelease {
                    " · prerelease"
                } else {
                    ""
                }
            ),
        ])
    });
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(40),
                Constraint::Percentage(30),
                Constraint::Percentage(30),
            ],
        )
        .header(
            Row::new(["Version", "Published", "Change"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        ),
        area,
    );
}
