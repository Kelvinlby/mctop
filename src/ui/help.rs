//! The help overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::theme::Theme;
use super::widgets::centred;

const KEYS: [(&str, &str); 11] = [
    ("q  Esc  Ctrl-C", "quit"),
    ("Tab  Shift-Tab", "change tab"),
    ("1 – 5", "jump straight to a tab"),
    ("↑ ↓  j k", "move the selection"),
    ("PgUp  PgDn", "move it ten rows"),
    ("Home  End", "jump to either end"),
    ("r", "collect everything now"),
    ("p  Space", "pause and resume collection"),
    ("s", "cycle the region sort (Regions tab)"),
    ("v", "raw poll responses (Console tab, unfocused)"),
    ("?  h  F1", "this help"),
];

const CONSOLE_KEYS: [(&str, &str); 6] = [
    ("Enter", "send the command"),
    ("↑ ↓", "walk the command history"),
    ("Esc", "clear the line, then give back the keys"),
    ("Ctrl-C", "quit, even while typing"),
    ("Ctrl-W  Ctrl-U", "delete a word, delete the line"),
    ("PgUp  PgDn", "scroll the output while typing"),
];

const NOTES: [&str; 5] = [
    "TPS and MSPT come over RCON; CPU, memory, and world sizes are read from",
    "the machine mctop runs on. Per-region detail is a Folia feature — on",
    "Paper the global figures already tell the whole story.",
    "",
    "Config: mctop config path shows where it lives, config init writes one.",
];

pub fn draw(frame: &mut Frame, area: Rect, theme: &Theme) {
    let width = 68.min(area.width);
    let height = (KEYS.len() + CONSOLE_KEYS.len() + NOTES.len() + 8) as u16;
    let target = centred(area, width, height.min(area.height));

    frame.render_widget(Clear, target);

    let block = theme.panel("Help");
    let inner = block.inner(target);
    frame.render_widget(block, target);

    let mut lines: Vec<Line> = KEYS
        .iter()
        .map(|(keys, description)| {
            Line::from(vec![
                Span::styled(
                    format!("{keys:<16}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*description, theme.value()),
            ])
        })
        .collect();

    lines.push(Line::default());
    lines.push(Line::styled(
        "On the Console tab the command line takes the keys:",
        theme.value(),
    ));
    lines.extend(CONSOLE_KEYS.iter().map(|(keys, description)| {
        Line::from(vec![
            Span::styled(
                format!("{keys:<16}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*description, theme.value()),
        ])
    }));

    lines.push(Line::default());
    lines.extend(NOTES.iter().map(|note| Line::styled(*note, theme.label())));
    lines.push(Line::default());
    lines.push(Line::styled(
        "any key closes this",
        Style::default().fg(theme.dim),
    ));

    frame.render_widget(Paragraph::new(lines), inner);
}
