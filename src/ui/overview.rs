//! The Overview tab: is the server healthy, and if not, where to look next.
//!
//! Four headline readings across the top, the two histories that matter under
//! them, and a summary column on the right. Everything more detailed lives on
//! its own tab, so this screen stays the same size whether the server ticks one
//! region or four hundred.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};

use crate::app::App;
use crate::format;
use crate::metrics::History;

use super::theme::{Health, Theme};
use super::widgets::{self, Stat};

/// Below this height the summary column is dropped rather than squeezed.
const NARROW_HEIGHT: u16 = 18;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [headline, rest] =
        Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).areas(area);

    draw_headline(frame, headline, app, theme);

    if area.height < NARROW_HEIGHT {
        draw_charts(frame, rest, app, theme);
        return;
    }

    // The summary column is a fixed width so the charts do not reflow every
    // time a number gains a digit.
    let [charts, summary] =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(34)]).areas(rest);

    draw_charts(frame, charts, app, theme);
    draw_summary(frame, summary, app, theme);
}

fn draw_headline(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let tiles: [Rect; 4] = super::columns(area);

    tps_tile(frame, tiles[0], app, theme);
    mspt_tile(frame, tiles[1], app, theme);
    cpu_tile(frame, tiles[2], app, theme);
    heap_tile(frame, tiles[3], app, theme);
}

fn tps_tile(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let tps = app.tps.current();
    let health = theme.tps_health(tps);

    let block = theme.panel("TPS");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The longer windows say whether a dip is a blip or a trend.
    let caption = if app.tps.windows.len() > 1 {
        Line::from(
            app.tps
                .windows
                .iter()
                .skip(1)
                .take(3)
                .flat_map(|(label, value)| {
                    vec![
                        Span::styled(format!("{label} "), theme.label()),
                        Span::styled(
                            format!("{value:.1} "),
                            theme.style(theme.tps_health(Some(*value))),
                        ),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    } else {
        Line::styled("waiting for a reading", theme.label())
    };

    let short_caption = Line::from(
        app.tps
            .windows
            .iter()
            .skip(1)
            .take(1)
            .flat_map(|(label, value)| {
                vec![
                    Span::styled(format!("{label} "), theme.label()),
                    Span::styled(
                        format!("{value:.1}"),
                        theme.style(theme.tps_health(Some(*value))),
                    ),
                ]
            })
            .collect::<Vec<_>>(),
    );

    widgets::stat(
        frame,
        inner,
        theme,
        // The panel is already titled TPS, so the unit would only cost the
        // columns that decide whether the large digits fit at all.
        &Stat::new(
            tps.map_or_else(|| "—".into(), |tps| format!("{tps:.2}")),
            "",
            health,
        )
        .or_value(tps.map_or_else(|| "—".into(), |tps| format!("{tps:.1}")))
        .caption(caption)
        .or_caption(short_caption)
        .trend(app.tps_history.tail(inner.width as usize)),
    );
}

fn mspt_tile(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let window = app.mspt.current();
    let average = window.map(|window| window.average);
    let health = theme.mspt_health(average);

    let block = theme.panel("MSPT");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let caption = match window {
        // The maximum is the number that explains a stutter players felt but
        // the average smoothed away.
        Some(window) => Line::from(vec![
            Span::styled("min ", theme.label()),
            Span::styled(format!("{:.1}  ", window.minimum), theme.value()),
            Span::styled("max ", theme.label()),
            Span::styled(
                format!("{:.1}", window.maximum),
                theme.style(theme.mspt_health(Some(window.maximum))),
            ),
        ]),
        None => Line::styled("waiting for a reading", theme.label()),
    };

    let short_caption = match window {
        Some(window) => Line::from(vec![
            Span::styled("max ", theme.label()),
            Span::styled(
                format!("{:.0}", window.maximum),
                theme.style(theme.mspt_health(Some(window.maximum))),
            ),
        ]),
        None => Line::styled("waiting", theme.label()),
    };

    widgets::stat(
        frame,
        inner,
        theme,
        &Stat::new(
            average.map_or_else(|| "—".into(), |value| format!("{value:.1}")),
            "ms",
            health,
        )
        .or_value(average.map_or_else(|| "—".into(), |value| format!("{value:.0}")))
        .caption(caption)
        .or_caption(short_caption)
        .trend(app.mspt_history.tail(inner.width as usize)),
    );
}

fn cpu_tile(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("CPU");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(stats) = &app.process else {
        widgets::placeholder(frame, inner, theme, super::process_unavailable(app).short());
        return;
    };

    let fraction = stats.cpu_fraction();

    widgets::stat(
        frame,
        inner,
        theme,
        &Stat::new(
            format!("{:.0}", stats.cpu_percent),
            "%",
            theme.load_health(Some(fraction)),
        )
        .caption(Line::styled(
            format!("{} of {} cores", format::percent(fraction), stats.cores),
            theme.label(),
        ))
        .or_caption(Line::styled(
            format!("{} of host", format::percent(fraction)),
            theme.label(),
        ))
        .trend(app.cpu_history.tail(inner.width as usize)),
    );
}

fn heap_tile(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Heap");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(used) = app.heap.used else {
        widgets::placeholder(frame, inner, theme, super::heap_unavailable(app).short());
        return;
    };

    // Occupancy after a collection is the honest measure of heap pressure;
    // current usage includes garbage that is about to be given back.
    let health = theme.load_health(app.heap.pressure());
    let ceiling = app.heap.max.map(format::bytes_parts);
    let caption = Line::from(vec![
        Span::styled(
            if app.heap.after_gc_measured {
                "after GC "
            } else {
                "after GC ≈"
            },
            theme.label(),
        ),
        Span::styled(
            format::optional(app.heap.after_gc, |after_gc| {
                let (number, unit) = format::bytes_parts(after_gc);
                // The unit is stated once, on the ceiling, when both agree.
                match &ceiling {
                    Some((_, ceiling_unit)) if *ceiling_unit == unit => number,
                    _ => format!("{number} {unit}"),
                }
            }),
            theme.style(health),
        ),
        Span::styled(
            format!(
                " / {}",
                ceiling.as_ref().map_or_else(
                    || "—".to_owned(),
                    |(number, unit)| format!("{number} {unit}")
                )
            ),
            theme.label(),
        ),
    ]);

    let short_caption = Line::from(vec![
        Span::styled("after GC ", theme.label()),
        Span::styled(
            format::optional(app.heap.pressure(), format::percent),
            theme.style(health),
        ),
    ]);

    let (number, unit) = format::bytes_parts(used);
    widgets::stat(
        frame,
        inner,
        theme,
        &Stat::new(number, unit, health)
            .caption(caption)
            .or_caption(short_caption)
            .trend(app.heap_history.tail(inner.width as usize)),
    );
}

fn draw_charts(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [top, bottom] =
        Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(area);

    history_chart(
        frame,
        top,
        theme,
        &Series {
            title: "Tick rate",
            history: &app.tps_history,
            health: theme.tps_health(app.tps.current()),
            // Framing the axis on 18–20 rather than 0–20 is what makes this
            // chart worth having: a server's whole working range is the top
            // tenth of the scale, and a dip to 19 is invisible on a full one.
            // The bounds widen on their own when a reading falls below.
            baseline: Some((18.0, 20.0)),
        },
    );

    history_chart(
        frame,
        bottom,
        theme,
        &Series {
            title: "Tick time (ms)",
            history: &app.mspt_history,
            health: theme.mspt_health(app.mspt.current().map(|window| window.average)),
            // Fifty milliseconds is one tick's whole budget, so the ceiling is
            // the line the server must stay under.
            baseline: Some((0.0, 50.0)),
        },
    );
}

/// One series and how to frame it.
struct Series<'a> {
    title: &'a str,
    history: &'a History,
    health: Health,
    /// A range the y-axis always covers, so the scale does not shift under a
    /// reader every time the data happens to sit in a narrow band.
    baseline: Option<(f64, f64)>,
}

/// A line chart of one series, with the y-axis widened to fit outliers.
fn history_chart(frame: &mut Frame, area: Rect, theme: &Theme, series: &Series<'_>) {
    let Series {
        title,
        history,
        health,
        baseline,
    } = *series;

    let block = theme.panel(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if history.is_empty() || inner.height < 3 {
        widgets::placeholder(frame, inner, theme, "collecting…");
        return;
    }

    let points = history.points(inner.width as usize * 2);
    let (low, high) = history.bounds().unwrap_or((0.0, 1.0));
    let (mut low, mut high) = match baseline {
        Some((floor, ceiling)) => (low.min(floor), high.max(ceiling)),
        None => (low, high),
    };
    if (high - low).abs() < f64::EPSILON {
        high = low + 1.0;
    }
    // A little headroom stops the line grazing the top border. The floor is
    // only lowered when it is not already at a meaningful zero.
    let padding = (high - low) * 0.05;
    high += padding;
    if low > 0.0 {
        low = (low - padding).max(0.0);
    }

    let span = points.first().map_or(1.0, |&(x, _)| x.abs().max(1.0));

    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(theme.style(health))
            .data(&points),
    ];

    let axis_style = Style::default().fg(theme.border);
    let chart = Chart::new(datasets)
        .style(Style::default().fg(theme.dim))
        .x_axis(
            Axis::default()
                .style(axis_style)
                .bounds([-span, 0.0])
                .labels(vec![
                    Span::styled(format!("-{}", format::span(span)), theme.label()),
                    Span::styled("now", theme.label()),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(axis_style)
                .bounds([low, high])
                .labels(vec![
                    Span::styled(format!("{low:.0}"), theme.label()),
                    Span::styled(format!("{:.0}", (low + high) / 2.0), theme.label()),
                    Span::styled(format!("{high:.0}"), theme.label()),
                ]),
        );

    frame.render_widget(chart, inner);
}

fn draw_summary(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("At a glance");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(widgets::field(
        "Players",
        match app.players.max {
            Some(max) => format!("{} / {max}", app.players.online),
            None => app.players.online.to_string(),
        },
        width,
        theme,
        Health::Unknown,
    ));

    if app.has_regions() {
        lines.push(widgets::field(
            "Regions",
            format::optional(app.regions.total, |total| total.to_string()),
            width,
            theme,
            Health::Unknown,
        ));
        if let Some(threads) = app.regions.threads {
            lines.push(widgets::field(
                "Region threads",
                threads.to_string(),
                width,
                theme,
                Health::Unknown,
            ));
        }
        if let Some(worst) = app.regions.worst() {
            let health = theme.load_health(Some(worst.pressure()));
            // The label carries a world name and a pair of coordinates, which
            // will not share a line with a value at this width.
            lines.push(Line::styled("Busiest region", theme.label()));
            lines.push(Line::styled(
                format!(
                    "  {}",
                    truncate(&worst.label(), width.saturating_sub(2) as usize)
                ),
                theme.value(),
            ));
            lines.push(widgets::field(
                "  tick budget used",
                format::percent(worst.pressure()),
                width,
                theme,
                health,
            ));
        }
    }

    lines.push(Line::default());

    if let Some(stats) = &app.process {
        lines.push(widgets::field(
            "Process",
            format!("pid {}", stats.pid),
            width,
            theme,
            Health::Unknown,
        ));
        lines.push(widgets::field(
            "Resident",
            format::bytes(stats.rss),
            width,
            theme,
            Health::Unknown,
        ));
        lines.push(widgets::field(
            "Threads",
            format::optional(stats.threads, |threads| threads.to_string()),
            width,
            theme,
            Health::Unknown,
        ));
        if let Some(load) = stats.load_average {
            lines.push(widgets::field(
                "Load average",
                format!("{:.2} {:.2} {:.2}", load[0], load[1], load[2]),
                width,
                theme,
                Health::Unknown,
            ));
        }
    }

    if app.heap.gc_load.is_some() || app.heap.young_collections.is_some() {
        lines.push(widgets::field(
            "GC load",
            format::optional(app.heap.gc_load, format::percent),
            width,
            theme,
            // Anything above a few percent of wall-clock time in the collector
            // is felt by players as stutter.
            match app.heap.gc_load {
                Some(load) if load > 0.10 => Health::Bad,
                Some(load) if load > 0.03 => Health::Warn,
                Some(_) => Health::Good,
                None => Health::Unknown,
            },
        ));
        lines.push(widgets::field(
            "GC young / full",
            format!(
                "{} / {}",
                format::optional(app.heap.young_collections, format::count),
                format::optional(app.heap.full_collections, format::count),
            ),
            width,
            theme,
            Health::Unknown,
        ));
    }

    if !app.disk.worlds.is_empty() {
        lines.push(Line::default());
        lines.push(widgets::field(
            "Worlds on disk",
            format::bytes(app.disk.total()),
            width,
            theme,
            Health::Unknown,
        ));
        if let Some((free, total)) = app.disk.free {
            let used = total.saturating_sub(free) as f64 / total.max(1) as f64;
            lines.push(widgets::field(
                "Disk free",
                format::bytes(free),
                width,
                theme,
                theme.load_health(Some(used)),
            ));
            lines.push(widgets::meter(
                used,
                width,
                theme,
                theme.load_health(Some(used)),
            ));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Shorten to `width`, marking the cut with an ellipsis.
pub fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_owned();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_marks_where_it_cut() {
        assert_eq!(truncate("world", 10), "world");
        assert_eq!(truncate("world_the_end", 6), "world…");
        assert_eq!(truncate("world", 0), "");
    }
}
