//! The outdated package table.

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Padding, Row, Table, TableState},
    Frame,
};

use super::text::{ago, short};
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let ids = app.visible();
    let rows: Vec<Row> = ids
        .iter()
        .map(|i| {
            let p = &app.packages[*i];
            let (patch, minor, major) = p.version_lanes();
            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(short(&p.current).to_string()),
                Cell::from(patch).style(Style::default().fg(Color::Green)),
                Cell::from(minor).style(Style::default().fg(Color::Yellow)),
                Cell::from(major).style(Style::default().fg(Color::Red)),
                Cell::from(ago(p.published)),
            ])
        })
        .collect();
    let mut state = TableState::default();
    state.select(ids.iter().position(|i| *i == app.selected));
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(19),
            Constraint::Percentage(17),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(22),
        ],
    )
    .header(
        Row::new([
            "Package",
            "Current",
            "Patch",
            "Minor",
            "Major",
            "Latest pub",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .title(title(app)),
    )
    .row_highlight_style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .bg(Color::DarkGray),
    )
    .highlight_symbol("❯ ");
    f.render_stateful_widget(table, area, &mut state);
}

fn title(app: &App) -> String {
    let direction = if app.descending { "↓" } else { "↑" };
    let mut title = format!(" PACKAGES · sort: {} {direction}", app.sort.label());
    let active = app.active_sort();
    if active != app.sort {
        title.push_str(&format!(" (active: {})", active.label()));
    }
    if let Some(query) = app.query() {
        title.push_str(&format!(" · filter: {query}"));
    }
    title.push(' ');
    title
}
