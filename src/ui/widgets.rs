//! Building blocks shared between the tabs.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline};

use super::theme::{Health, Theme};

/// Three-row digits, for the one number on the screen that should be readable
/// from across the room.
const BIG_ROWS: usize = 3;

fn big_glyph(ch: char) -> [&'static str; BIG_ROWS] {
    match ch {
        '0' => ["┏━┓", "┃ ┃", "┗━┛"],
        '1' => [" ╻ ", " ┃ ", " ╹ "],
        '2' => ["┏━┓", "┏━┛", "┗━╸"],
        '3' => ["┏━┓", " ━┫", "┗━┛"],
        '4' => ["╻ ╻", "┗━┫", "  ╹"],
        '5' => ["┏━╸", "┗━┓", "┗━┛"],
        '6' => ["┏━╸", "┣━┓", "┗━┛"],
        '7' => ["┏━┓", "  ┃", "  ╹"],
        '8' => ["┏━┓", "┣━┫", "┗━┛"],
        '9' => ["┏━┓", "┗━┫", "┗━┛"],
        '.' => ["   ", "   ", " ▄ "],
        ',' => ["   ", "   ", " ▄ "],
        ':' => ["   ", " ▪ ", " ▪ "],
        '-' => ["   ", "╺━╸", "   "],
        '?' => ["┏━┓", " ━┫", " ╹ "],
        _ => ["   ", "   ", "   "],
    }
}

/// Render `text` as three rows of large digits.
pub fn big_text(text: &str) -> [String; BIG_ROWS] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (index, ch) in text.chars().enumerate() {
        let glyph = big_glyph(ch);
        for (row, part) in rows.iter_mut().zip(glyph) {
            if index > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    rows
}

/// The width `big_text` needs for `text`.
pub fn big_text_width(text: &str) -> u16 {
    let count = text.chars().count() as u16;
    count.saturating_mul(4).saturating_sub(1)
}

/// A headline reading: a large number, its unit, and a caption underneath.
///
/// The value and the caption may each be given more than once, widest first.
/// A tile is only ever a quarter of the screen, and at eighty columns that is
/// sixteen characters — narrow enough that `19.98` will not fit as large
/// digits and `min 4.2  max 46.8` will not fit at all. Rather than truncate
/// mid-number, the tile takes the first form that fits.
pub struct Stat<'a> {
    values: Vec<String>,
    unit: &'a str,
    captions: Vec<Line<'a>>,
    health: Health,
    /// Recent values behind the number, drawn as a sparkline when there is room.
    trend: Vec<f64>,
}

impl<'a> Stat<'a> {
    pub fn new(value: impl Into<String>, unit: &'a str, health: Health) -> Self {
        Self {
            values: vec![value.into()],
            unit,
            captions: Vec::new(),
            health,
            trend: Vec::new(),
        }
    }

    /// A shorter rendering of the same number, used when the tile is narrow.
    pub fn or_value(mut self, value: impl Into<String>) -> Self {
        self.values.push(value.into());
        self
    }

    pub fn caption(mut self, caption: impl Into<Line<'a>>) -> Self {
        self.captions.push(caption.into());
        self
    }

    /// A shorter caption, used when the widest one would be cut off.
    pub fn or_caption(mut self, caption: impl Into<Line<'a>>) -> Self {
        self.captions.push(caption.into());
        self
    }

    pub fn trend(mut self, trend: Vec<f64>) -> Self {
        self.trend = trend;
        self
    }

    /// The widest value that can be set in large digits, with the unit beside
    /// it, and whether any could.
    fn fitting_value(&self, area: Rect) -> (&str, bool) {
        let unit = self.unit.chars().count() as u16 + 1;

        if area.height > BIG_ROWS as u16
            && let Some(value) = self.values.iter().find(|value| {
                // A placeholder has no digits to enlarge; setting it in the big
                // face would just leave an empty block where a number goes.
                value.chars().any(|ch| ch.is_ascii_digit())
                    && big_text_width(value) + unit <= area.width
            })
        {
            return (value, true);
        }

        let small = self
            .values
            .iter()
            .find(|value| value.chars().count() as u16 + unit <= area.width)
            .or_else(|| self.values.last());

        (small.map_or("", String::as_str), false)
    }

    fn fitting_caption(&self, width: u16) -> Line<'a> {
        self.captions
            .iter()
            .find(|caption| caption.width() as u16 <= width)
            .or_else(|| self.captions.last())
            .cloned()
            .unwrap_or_default()
    }
}

/// Draw a headline reading inside `area`, which should already be a panel's
/// interior. Falls back to a plain number when the panel is too short for the
/// large digits.
pub fn stat(frame: &mut Frame, area: Rect, theme: &Theme, stat: &Stat<'_>) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let style = theme.style(stat.health).add_modifier(Modifier::BOLD);
    let (value, big_fits) = stat.fitting_value(area);

    let rows = if big_fits {
        Layout::vertical([
            Constraint::Length(BIG_ROWS as u16),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area)
    } else {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area)
    };

    if big_fits {
        let width = big_text_width(value);
        let [number, unit] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(rows[0]);

        let lines: Vec<Line> = big_text(value)
            .into_iter()
            .map(|row| Line::styled(row, style))
            .collect();
        frame.render_widget(Paragraph::new(lines), number);

        if !stat.unit.is_empty() && unit.width > 0 {
            // The unit sits on the baseline of the digits rather than beside
            // their midpoint, which reads as a subscript.
            frame.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::default(),
                    Line::styled(format!(" {}", stat.unit), theme.label()),
                ]),
                unit,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(value.to_owned(), style),
                Span::styled(format!(" {}", stat.unit), theme.label()),
            ])),
            rows[0],
        );
    }

    frame.render_widget(Paragraph::new(stat.fitting_caption(area.width)), rows[1]);

    if !stat.trend.is_empty() && rows[2].height > 0 {
        sparkline(frame, rows[2], theme, &stat.trend, stat.health);
    }
}

/// A sparkline scaled to the data it holds, so small movements stay visible.
pub fn sparkline(frame: &mut Frame, area: Rect, theme: &Theme, values: &[f64], health: Health) {
    if area.width == 0 || area.height == 0 || values.is_empty() {
        return;
    }

    let tail: Vec<f64> = values
        .iter()
        .rev()
        .take(area.width as usize)
        .rev()
        .copied()
        .collect();

    let low = tail.iter().copied().fold(f64::INFINITY, f64::min);
    let high = tail.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    // A flat series would otherwise draw as either nothing or a solid block.
    let (low, high) = if (high - low).abs() < f64::EPSILON {
        (low - 1.0, high + 1.0)
    } else {
        (low, high)
    };

    const STEPS: f64 = 1000.0;
    let data: Vec<u64> = tail
        .iter()
        .map(|&value| (((value - low) / (high - low)) * STEPS).clamp(0.0, STEPS) as u64)
        .collect();

    frame.render_widget(
        Sparkline::default()
            .data(data)
            .max(STEPS as u64)
            .style(theme.style(health)),
        area,
    );
}

/// A horizontal meter drawn from eighth-blocks, so it moves smoothly even in a
/// narrow column.
pub fn meter(fraction: f64, width: u16, theme: &Theme, health: Health) -> Line<'static> {
    const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

    let width = width as usize;
    if width == 0 {
        return Line::default();
    }

    let filled = (fraction.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let full = filled / 8;
    let remainder = filled % 8;

    let mut bar = String::new();
    for _ in 0..full.min(width) {
        bar.push('█');
    }
    if full < width && remainder > 0 {
        bar.push(EIGHTHS[remainder - 1]);
    }

    let drawn = bar.chars().count();
    let mut spans = vec![Span::styled(bar, theme.style(health))];
    if drawn < width {
        spans.push(Span::styled(
            "░".repeat(width - drawn),
            Style::default().fg(theme.border),
        ));
    }

    Line::from(spans)
}

/// A `label  value` row, with the value pushed to the right edge.
pub fn field(
    label: impl Into<String>,
    value: impl Into<String>,
    width: u16,
    theme: &Theme,
    health: Health,
) -> Line<'static> {
    let label = label.into();
    let value = value.into();
    let used = label.chars().count() + value.chars().count();
    let gap = (width as usize).saturating_sub(used).max(1);

    Line::from(vec![
        Span::styled(label, theme.label()),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            value,
            if health == Health::Unknown {
                theme.value()
            } else {
                theme.style(health)
            },
        ),
    ])
}

/// Centre a fixed-size box inside `area`, for overlays.
pub fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A single line of placeholder text, for panels with nothing to show yet.
pub fn placeholder(frame: &mut Frame, area: Rect, theme: &Theme, message: &str) {
    if area.height == 0 {
        return;
    }
    let target = Rect {
        y: area.y + area.height / 2,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Line::styled(message, theme.label())).alignment(Alignment::Center),
        target,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UiConfig;

    #[test]
    fn big_text_is_three_rows_of_equal_width() {
        let rows = big_text("19.9");
        assert_eq!(rows.len(), 3);
        let widths: Vec<usize> = rows.iter().map(|row| row.chars().count()).collect();
        assert_eq!(widths[0], widths[1]);
        assert_eq!(widths[1], widths[2]);
        assert_eq!(widths[0], big_text_width("19.9") as usize);
    }

    #[test]
    fn big_text_handles_an_empty_string() {
        let rows = big_text("");
        assert!(rows.iter().all(String::is_empty));
        assert_eq!(big_text_width(""), 0);
    }

    #[test]
    fn unknown_characters_become_blanks_rather_than_panicking() {
        let rows = big_text("2x");
        assert_eq!(rows[0].chars().count(), big_text_width("2x") as usize);
    }

    #[test]
    fn a_placeholder_is_never_set_in_large_digits() {
        let stat = Stat::new("—", "tps", Health::Unknown);
        let area = Rect::new(0, 0, 40, 10);
        let (value, big) = stat.fitting_value(area);
        assert_eq!(value, "—");
        assert!(!big, "a value with no digits should stay small");

        let stat = Stat::new("19.98", "", Health::Good);
        assert!(stat.fitting_value(area).1, "a number should be enlarged");
    }

    #[test]
    fn a_narrow_tile_falls_back_to_the_shorter_value_and_caption() {
        let stat = Stat::new("19.98", "", Health::Good)
            .or_value("20.0")
            .caption(Line::raw("1m 19.7 5m 19.9 15m 20.0"))
            .or_caption(Line::raw("1m 19.7"));

        // Wide enough for the long form.
        assert_eq!(stat.fitting_value(Rect::new(0, 0, 40, 10)).0, "19.98");
        assert_eq!(stat.fitting_caption(40).width(), 24);

        // Not wide enough; both fall back.
        assert_eq!(stat.fitting_value(Rect::new(0, 0, 16, 10)).0, "20.0");
        assert_eq!(stat.fitting_caption(16).width(), 7);
    }

    #[test]
    fn the_meter_fills_proportionally() {
        let theme = Theme::new(&UiConfig::default());
        let full = meter(1.0, 10, &theme, Health::Good);
        assert_eq!(full.spans.len(), 1);
        assert_eq!(full.spans[0].content.chars().count(), 10);

        let empty = meter(0.0, 10, &theme, Health::Good);
        assert_eq!(empty.spans[1].content.chars().count(), 10);

        let half = meter(0.5, 10, &theme, Health::Good);
        assert_eq!(half.spans[0].content.chars().count(), 5);
    }

    #[test]
    fn the_meter_clamps_out_of_range_fractions() {
        let theme = Theme::new(&UiConfig::default());
        let over = meter(4.0, 6, &theme, Health::Bad);
        assert_eq!(over.spans[0].content.chars().count(), 6);

        let under = meter(-1.0, 6, &theme, Health::Bad);
        assert_eq!(under.spans[1].content.chars().count(), 6);

        assert!(meter(0.5, 0, &theme, Health::Good).spans.is_empty());
    }

    #[test]
    fn fields_pad_between_label_and_value() {
        let theme = Theme::new(&UiConfig::default());
        let line = field("Players", "12 / 60", 24, &theme, Health::Unknown);
        assert_eq!(line.width(), 24);
    }

    #[test]
    fn a_field_too_wide_for_its_column_keeps_one_space() {
        let theme = Theme::new(&UiConfig::default());
        let line = field(
            "a-very-long-label",
            "and-a-long-value",
            4,
            &theme,
            Health::Good,
        );
        assert_eq!(line.spans[1].content.as_ref(), " ");
    }

    #[test]
    fn centring_never_leaves_the_area() {
        let area = Rect::new(0, 0, 80, 24);
        let inner = centred(area, 40, 10);
        assert_eq!(
            (inner.x, inner.y, inner.width, inner.height),
            (20, 7, 40, 10)
        );

        // An overlay larger than the screen is clipped to it.
        let clipped = centred(area, 200, 200);
        assert_eq!((clipped.width, clipped.height), (80, 24));
    }
}
