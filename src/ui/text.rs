//! Text building blocks shared by the detail panes, and value formatting.

use chrono::{DateTime, Utc};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Width of the label column in `field` rows.
const LABEL_WIDTH: usize = 19;

/// Accumulates the lines of a detail pane, wrapping values to `width`.
pub struct DetailLines {
    lines: Vec<Line<'static>>,
    width: usize,
}

impl DetailLines {
    pub fn new(width: usize) -> Self {
        Self {
            lines: vec![],
            width,
        }
    }

    pub fn push(&mut self, line: impl Into<Line<'static>>) {
        self.lines.push(line.into());
    }

    pub fn blank(&mut self) {
        self.push("");
    }

    pub fn bold(&mut self, text: impl Into<String>) {
        self.push(Span::styled(
            text.into(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    pub fn colored(&mut self, text: impl Into<String>, color: Color) {
        self.push(Span::styled(text.into(), Style::default().fg(color)));
    }

    pub fn section(&mut self, title: &str) {
        self.heading(title, Color::Cyan);
    }

    pub fn subsection(&mut self, title: &str) {
        self.heading(title, Color::Gray);
    }

    fn heading(&mut self, title: &str, color: Color) {
        self.push(Span::styled(
            format!(" {title}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    /// A gray label followed by `value`, wrapped under itself.
    pub fn field(&mut self, label: &str, value: &str) {
        let prefix = format!(" {label:<LABEL_WIDTH$}");
        let prefix_width = Span::raw(prefix.as_str()).width();
        let value_width = self.width.saturating_sub(prefix_width + 1).max(1);
        let continuation = " ".repeat(prefix_width + 1);
        for (index, part) in wrap(value, value_width).into_iter().enumerate() {
            self.push(if index == 0 {
                Line::from(vec![
                    Span::styled(prefix.clone(), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::raw(part),
                ])
            } else {
                Line::from(vec![Span::raw(continuation.clone()), Span::raw(part)])
            });
        }
    }

    /// An indented paragraph.
    pub fn value(&mut self, value: &str) {
        let prefix = " ";
        for part in wrap(value, self.width.saturating_sub(prefix.len()).max(1)) {
            self.push(Line::from(vec![Span::raw(prefix), Span::raw(part)]));
        }
    }

    /// Terminal rows these lines occupy once the paragraph wraps them.
    pub fn height(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(self.width))
            .sum()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn into_lines(self) -> Vec<Line<'static>> {
        self.lines
    }
}

/// Wraps at spaces by display width, breaking words longer than `width`.
pub fn wrap(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in value.split_whitespace() {
        let word_width = Span::raw(word).width();
        if current_width > 0 && current_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if current_width > 0 {
            current.push(' ');
            current_width += 1;
        }
        for character in word.chars() {
            let character = character.to_string();
            let character_width = Span::raw(character.as_str()).width();
            if current_width > 0 && current_width + character_width > width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push_str(&character);
            current_width += character_width;
        }
    }
    if current_width > 0 || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Pads `text` with spaces to `width` display columns.
pub fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(Span::raw(text).width()))
    )
}

pub fn or_na(value: Option<&str>) -> &str {
    value.unwrap_or("N/A")
}

pub fn ago(date: Option<DateTime<Utc>>) -> String {
    let Some(date) = date else {
        return "N/A".into();
    };
    match Utc::now().signed_duration_since(date).num_days() {
        ..1 => "today".into(),
        1 => "1 day ago".into(),
        days => format!("{days} days ago"),
    }
}

pub fn count(n: Option<u64>) -> String {
    match n {
        None => "N/A".into(),
        Some(n) if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        Some(n) if n >= 1000 => format!("{:.1}K", n as f64 / 1e3),
        Some(n) => n.to_string(),
    }
}

/// Strips a leading `^` or `~` range operator.
pub fn short(spec: &str) -> &str {
    spec.strip_prefix('^')
        .or_else(|| spec.strip_prefix('~'))
        .unwrap_or(spec)
}

#[cfg(test)]
pub fn plain_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn wraps_words_and_breaks_long_ones() {
        assert_eq!(wrap("a bb ccc", 4), ["a bb", "ccc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("", 5), [""]);
        // Wide characters count as two columns.
        assert_eq!(wrap("日本語", 4), ["日本", "語"]);
    }

    #[test]
    fn fields_wrap_under_their_value_column() {
        // 21 columns of label leave 10 for the value.
        let mut lines = DetailLines::new(31);
        lines.field("Description", "one two three four");
        let text = plain_text(&lines.into_lines());
        assert_eq!(
            text,
            " Description         one two\n                     three four"
        );
    }

    #[test]
    fn formats_values() {
        assert_eq!(count(Some(999)), "999");
        assert_eq!(count(Some(1_500)), "1.5K");
        assert_eq!(count(Some(2_340_000)), "2.3M");
        assert_eq!(count(None), "N/A");
        assert_eq!(ago(None), "N/A");
        assert_eq!(ago(Some(Utc::now())), "today");
        assert_eq!(ago(Some(Utc::now() - Duration::hours(30))), "1 day ago");
        assert_eq!(ago(Some(Utc::now() - Duration::days(9))), "9 days ago");
        assert_eq!(short("^1.2.3"), "1.2.3");
        assert_eq!(short("~1.2.3"), "1.2.3");
        assert_eq!(short(">=1"), ">=1");
        assert_eq!(pad("✓ ok", 6), "✓ ok  ");
    }
}
