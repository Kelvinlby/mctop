//! The Regions tab: Folia's per-region detail, kept off the main screen.
//!
//! A Folia server regionises each world and ticks the pieces independently, so
//! a single slow region is invisible in the global average yet very obvious to
//! the players standing in it. This tab is where that shows up: the table is
//! sorted worst-first by default, and the panel underneath explains the row
//! under the cursor.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use crate::app::App;
use crate::format;

use super::overview::truncate;
use super::theme::{Health, Theme};
use super::widgets;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if !app.has_regions() {
        let block = theme.panel("Regions");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        explain_absence(frame, inner, app, theme);
        return;
    }

    let [table, detail] = Layout::vertical([Constraint::Min(6), Constraint::Length(8)]).areas(area);

    draw_table(frame, table, app, theme);
    draw_detail(frame, detail, app, theme);
}

/// An empty Regions tab is a fact about the server, not a failure, and saying
/// which of the two it is saves an operator a hunt through the config.
fn explain_absence(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut lines = vec![Line::default()];

    if app.identity.is_folia() {
        lines.push(Line::styled(
            "This server reports itself as Folia, but its region report could not be read.",
            theme.value(),
        ));
        lines.push(Line::default());
        lines.push(Line::styled(
            format!(
                "mctop asked it to run `{}`. Check the Log tab for the raw response, and set",
                app.config.commands.regions
            ),
            theme.label(),
        ));
        lines.push(Line::styled(
            "[commands].regions in the config to whatever this build answers to.",
            theme.label(),
        ));
    } else {
        lines.push(Line::styled(
            "No per-region detail is available.",
            theme.value(),
        ));
        lines.push(Line::default());
        lines.push(Line::styled(
            "Regions are a Folia feature: it splits each world into slices that tick on",
            theme.label(),
        ));
        lines.push(Line::styled(
            "separate threads. Paper, Spigot, and vanilla tick one thread per server, so",
            theme.label(),
        ));
        lines.push(Line::styled(
            "the global TPS and MSPT on the Overview tab already tell the whole story.",
            theme.label(),
        ));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_table(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let regions = app.sorted_regions();

    let title = format!(
        "Regions — {} shown, sorted by {}",
        regions.len(),
        app.region_sort.label()
    );
    let block = theme.panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if regions.is_empty() {
        widgets::placeholder(frame, inner, theme, "the server reported no region detail");
        return;
    }

    // The numeric columns are fixed, the region name gets what it needs, and
    // the load meter takes what is left — it is the one column that still says
    // something useful at six characters, and the name is not.
    const FIXED: u16 = 45 + 7 + 1; // numeric columns, spacing, selection mark
    const NAME: u16 = 30; // what a region label typically needs
    let meter_width = inner.width.saturating_sub(FIXED + NAME).min(40);
    let meter_width = if meter_width < 6 { 0 } else { meter_width };
    let name_width = inner
        .width
        .saturating_sub(FIXED + meter_width)
        .clamp(10, 44);

    let header = Row::new(vec![
        Cell::from("REGION"),
        Cell::from(Line::from("TPS").right_aligned()),
        Cell::from(Line::from("MSPT").right_aligned()),
        Cell::from(Line::from("PLR").right_aligned()),
        Cell::from(Line::from("ENTITIES").right_aligned()),
        Cell::from(Line::from("CHUNKS").right_aligned()),
        Cell::from(Line::from("LOAD").right_aligned()),
        Cell::from(""),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = regions
        .iter()
        .map(|region| {
            let pressure = region.pressure();
            let load_health = theme.load_health(Some(pressure));

            Row::new(vec![
                Cell::from(Span::styled(truncate(&region.label(), 30), theme.value())),
                Cell::from(
                    Line::from(Span::styled(
                        format::optional(region.tps, |tps| format!("{tps:.2}")),
                        theme.style(theme.tps_health(region.tps)),
                    ))
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(Span::styled(
                        format::optional(region.mspt, |mspt| format!("{mspt:.1}")),
                        theme.style(theme.mspt_health(region.mspt)),
                    ))
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(format::optional(region.players, |count| count.to_string()))
                        .right_aligned(),
                ),
                Cell::from(
                    Line::from(format::optional(region.entities, |count| {
                        format::count(u64::from(count))
                    }))
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(format::optional(region.chunks, |count| {
                        format::count(u64::from(count))
                    }))
                    .right_aligned(),
                ),
                Cell::from(
                    Line::from(Span::styled(
                        format::percent(pressure),
                        theme.style(load_health),
                    ))
                    .right_aligned(),
                ),
                Cell::from(widgets::meter(pressure, meter_width, theme, load_health)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(name_width),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(5),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(meter_width),
    ];

    // Carrying the offset in and out is what makes the table scroll a row at a
    // time rather than snapping the selection to an edge on every keystroke.
    let mut state = TableState::default()
        .with_selected(Some(app.region_selected))
        .with_offset(app.region_offset);
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .style(theme.value())
            .row_highlight_style(Style::default().bg(theme.highlight))
            .highlight_symbol(Span::styled("▎", Style::default().fg(theme.accent)))
            .column_spacing(1),
        inner,
        &mut state,
    );
    app.region_offset = state.offset();
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Selected region");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(region) = app.selected_region() else {
        widgets::placeholder(frame, inner, theme, "no region selected");
        return;
    };

    let [left, right] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(inner);

    let width = left.width.saturating_sub(2);
    let pressure = region.pressure();

    let left_lines = vec![
        Line::styled(region.label(), theme.strong()),
        Line::default(),
        widgets::field(
            "World",
            region.world.clone().unwrap_or_else(|| "—".into()),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Centre chunk",
            format::optional(region.chunk, |(x, z)| format!("{x}, {z}")),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Block position",
            // A chunk is sixteen blocks square; operators think in blocks when
            // they go to look at the problem.
            format::optional(region.chunk, |(x, z)| format!("{}, {}", x * 16, z * 16)),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    let right_lines = vec![
        widgets::field(
            "Tick rate",
            format::optional(region.tps, |tps| format!("{tps:.2} tps")),
            width,
            theme,
            theme.tps_health(region.tps),
        ),
        widgets::field(
            "Tick time",
            format::optional(region.mspt, |mspt| format!("{mspt:.2} ms")),
            width,
            theme,
            theme.mspt_health(region.mspt),
        ),
        widgets::field(
            "Tick budget used",
            format::percent(pressure),
            width,
            theme,
            theme.load_health(Some(pressure)),
        ),
        widgets::meter(pressure, width, theme, theme.load_health(Some(pressure))),
        widgets::field(
            "Players · entities · chunks",
            format!(
                "{} · {} · {}",
                format::optional(region.players, |count| count.to_string()),
                format::optional(region.entities, |count| count.to_string()),
                format::optional(region.chunks, |count| count.to_string()),
            ),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    frame.render_widget(Paragraph::new(left_lines), left);
    frame.render_widget(Paragraph::new(right_lines), right);
}
