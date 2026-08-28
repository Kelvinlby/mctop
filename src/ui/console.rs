//! The Console tab: what has happened, and a line to make something happen.
//!
//! The scrollback carries mctop's own notes alongside the commands the operator
//! has typed and the server's replies to them. What it deliberately leaves out
//! is the polling traffic: a `tps` running once a second would bury everything
//! else within a minute. Those responses live in the raw view behind `v`, which
//! exists for the narrower job of working out why a parser came up empty.

use std::time::UNIX_EPOCH;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::App;
use crate::source::Kind;
use crate::ui::widgets;

use super::theme::{Health, Theme};

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [output, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(area);

    if app.show_raw {
        draw_raw(frame, output, app, theme);
    } else {
        draw_scrollback(frame, output, app, theme);
    }

    draw_input(frame, input, app, theme);
}

fn draw_scrollback(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let title = if app.log_scroll > 0 {
        format!("Console — scrolled back {} lines", app.log_scroll)
    } else {
        "Console".to_owned()
    };

    let block = theme.panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.log.is_empty() {
        widgets::placeholder(frame, inner, theme, "nothing yet — type a command below");
        return;
    }

    // Fill from the bottom, the way a terminal does: the newest line sits just
    // above the command box, and an empty console starts empty at the top
    // rather than leaving the operator typing into the middle of the screen.
    let width = inner.width.max(1) as usize;
    let visible = inner.height as usize;
    let end = app.log.len().saturating_sub(app.log_scroll);

    let mut chosen = Vec::new();
    let mut used = 0usize;
    for entry in app.log.iter().take(end).rev() {
        // A long reply wraps, so counting entries would overrun the box and
        // push the newest lines off the bottom.
        let height = wrapped_height(&entry.message, width);
        if used + height > visible && !chosen.is_empty() {
            break;
        }
        used += height;
        chosen.push(entry);
    }
    chosen.reverse();

    let mut lines: Vec<Line> = vec![Line::default(); visible.saturating_sub(used)];
    lines.extend(chosen.into_iter().map(|entry| {
        let (marker, style) = match entry.kind {
            Kind::Info => ("info ", theme.label()),
            Kind::Warn => ("warn ", theme.style(Health::Warn)),
            Kind::Error => ("error", theme.style(Health::Bad)),
            // A typed command is set apart from everything else on the
            // screen, because it is the only line the operator caused.
            Kind::Sent => (
                "  » ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Kind::Received => ("    ", Style::default().fg(theme.info)),
        };

        Line::from(vec![
            Span::styled(
                format!("{} ", clock(entry.at)),
                Style::default().fg(theme.border),
            ),
            Span::styled(format!("{marker:<5}"), style),
            Span::raw(" "),
            Span::styled(
                entry.message.clone(),
                match entry.kind {
                    Kind::Sent => style,
                    Kind::Received => Style::default().fg(theme.text),
                    _ => theme.value(),
                },
            ),
        ])
    }));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Rows a scrollback entry takes once the box has wrapped it.
fn wrapped_height(message: &str, width: usize) -> usize {
    // Time stamp, the padded kind marker, and the spaces around them.
    const GUTTER: usize = 9 + 5 + 1;

    let columns = width.saturating_sub(GUTTER).max(1);
    message.chars().count().div_ceil(columns).max(1)
}

fn draw_raw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Raw poll responses — press v for the console");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.raw.is_empty() {
        widgets::placeholder(frame, inner, theme, "no responses received yet");
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (command, response) in app.raw.iter().skip(app.log_scroll) {
        lines.push(Line::from(vec![
            Span::styled(
                "› ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                command.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let text = crate::source::parse::strip_formatting(response);
        if text.trim().is_empty() {
            lines.push(Line::styled("  (empty response)", theme.label()));
        } else {
            for line in text.lines() {
                lines.push(Line::styled(format!("  {line}"), theme.value()));
            }
        }
        lines.push(Line::default());

        if lines.len() > inner.height as usize {
            break;
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let connected = app.link.is_up();
    let title = if !app.input_focused {
        "Command — press Enter to type"
    } else if connected {
        "Command"
    } else {
        "Command — not connected"
    };

    let block = theme.panel(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let prompt = "> ";
    let text = app.input.text();

    // Scroll the field horizontally once the line outruns the box, keeping the
    // cursor in view rather than letting it run off the edge.
    let room = inner.width.saturating_sub(prompt.len() as u16) as usize;
    let offset = app.input.cursor().saturating_sub(room.saturating_sub(1));
    let visible: String = text.chars().skip(offset).take(room).collect();

    let body = if text.is_empty() && app.input_focused {
        Span::styled("say something, or `list`, or `save-all`", theme.label())
    } else {
        Span::styled(visible, theme.value())
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                prompt,
                Style::default()
                    .fg(if app.input_focused {
                        theme.accent
                    } else {
                        theme.border
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            body,
        ])),
        inner,
    );

    if app.input_focused {
        // A real terminal cursor, so the field behaves like every other one the
        // operator has ever typed into.
        let column = prompt.len() + app.input.cursor().saturating_sub(offset);
        frame.set_cursor_position(Position::new(
            inner.x + (column as u16).min(inner.width.saturating_sub(1)),
            inner.y,
        ));
    }
}

/// Wall-clock `HH:MM:SS` in UTC, which needs no timezone database.
fn clock(at: std::time::SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let day = seconds % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_clock_wraps_at_midnight() {
        let at = UNIX_EPOCH + Duration::from_secs(86_400 + 3_661);
        assert_eq!(clock(at), "01:01:01");
        assert_eq!(clock(UNIX_EPOCH), "00:00:00");
    }

    #[test]
    fn wrapped_height_counts_the_rows_a_long_reply_needs() {
        // Eighty columns leaves sixty-five for the message.
        assert_eq!(wrapped_height("short", 80), 1);
        assert_eq!(wrapped_height(&"x".repeat(65), 80), 1);
        assert_eq!(wrapped_height(&"x".repeat(66), 80), 2);
        assert_eq!(wrapped_height(&"x".repeat(130), 80), 2);
        assert_eq!(wrapped_height(&"x".repeat(131), 80), 3);

        // An empty message and an absurdly narrow box still take one row.
        assert_eq!(wrapped_height("", 80), 1);
        assert_eq!(wrapped_height("something", 4), 9);
    }

    #[test]
    fn every_kind_marker_is_the_same_width() {
        // The markers are padded to five so the message column lines up
        // regardless of what kind of line it is.
        for marker in ["info ", "warn ", "error", "  » ", "    "] {
            assert!(marker.chars().count() <= 5, "{marker:?} is too wide");
        }
    }
}
