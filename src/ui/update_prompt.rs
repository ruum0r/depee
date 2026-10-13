//! Full-screen prompt offering a newer depee release.

use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{UpdateChoice, UpdatePrompt};

pub fn draw(f: &mut Frame, prompt: &UpdatePrompt) {
    let area = f.area();
    f.render_widget(
        Block::default().borders(Borders::ALL).title(" depee "),
        area,
    );
    let mut lines = vec![
        Line::from(Span::styled(
            "Update available",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "v{} → v{}",
            env!("CARGO_PKG_VERSION"),
            prompt.version
        )),
        Line::from(""),
    ];
    for (i, choice) in UpdateChoice::ALL.iter().enumerate() {
        lines.push(if i == prompt.selected {
            Line::from(Span::styled(
                format!("> {}", choice.label()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(format!("  {}", choice.label()))
        });
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ or j/k select · Enter confirm · Esc later · q quit",
        Style::default().fg(Color::DarkGray),
    )));
    let [body] = Layout::vertical([Constraint::Length(lines.len() as u16)])
        .flex(Flex::Center)
        .areas(area);
    let [body] = Layout::horizontal([Constraint::Length(48)])
        .flex(Flex::Center)
        .areas(body);
    f.render_widget(Paragraph::new(lines), body);
}
