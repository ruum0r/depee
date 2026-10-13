//! The workspace member picker.

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Padding, Row, Table, TableState},
    Frame,
};

use super::text::or_na;
use crate::app::{App, WorkspacePicker};

pub fn draw(f: &mut Frame, app: &App, picker: &WorkspacePicker, area: Rect) {
    let Some(workspace) = &app.project.workspace else {
        return;
    };
    let rows: Vec<Row> = workspace
        .members
        .iter()
        .map(|member| {
            // The current member is marked by text, not only by color.
            let current = if member.path == workspace.member {
                "●"
            } else {
                ""
            };
            Row::new(vec![
                Cell::from(current),
                Cell::from(member.label().to_string()),
                Cell::from(or_na(member.name.as_deref()).to_string()),
            ])
        })
        .collect();
    let mut state = TableState::default();
    state.select(Some(picker.selected));
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Percentage(55),
            Constraint::Percentage(45),
        ],
    )
    .header(Row::new(["", "Path", "Package"]).style(Style::default().add_modifier(Modifier::BOLD)))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .title(format!(" WORKSPACE MEMBERS · {} ", workspace.members.len())),
    )
    .row_highlight_style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .bg(Color::DarkGray),
    )
    .highlight_symbol("❯ ");
    f.render_stateful_widget(table, area, &mut state);
}
