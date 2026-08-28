//! The Worlds tab: what each world is costing on disk.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use crate::app::App;
use crate::format;

use super::theme::{Health, Theme};
use super::widgets;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [table, summary] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(9)]).areas(area);

    draw_table(frame, table, app, theme);
    draw_summary(frame, summary, app, theme);
}

fn draw_table(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let title = if app.disk.scanning {
        "Worlds — scanning…".to_owned()
    } else {
        match app.disk.scanned_at.map(|at| at.elapsed()) {
            Some(Ok(elapsed)) => format!("Worlds — measured {}", format::ago(elapsed)),
            _ => "Worlds".to_owned(),
        }
    };

    let block = theme.panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.disk.worlds.is_empty() {
        no_worlds(frame, inner, app, theme);
        return;
    }

    let total = app.disk.total().max(1);
    let meter_width = inner.width.saturating_sub(64).clamp(6, 24);

    let header = Row::new(vec![
        Cell::from("WORLD"),
        Cell::from(Line::from("TOTAL").right_aligned()),
        Cell::from(Line::from("REGION").right_aligned()),
        Cell::from(Line::from("ENTITIES").right_aligned()),
        Cell::from(Line::from("POI").right_aligned()),
        Cell::from(Line::from("FILES").right_aligned()),
        Cell::from(Line::from("SHARE").right_aligned()),
        Cell::from(""),
    ])
    .style(Style::default().fg(theme.dim).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app
        .disk
        .worlds
        .iter()
        .map(|world| {
            let share = world.bytes as f64 / total as f64;
            let name = if world.partial {
                // A partial walk is flagged rather than silently under-reported.
                format!("{} *", world.name)
            } else {
                world.name.clone()
            };

            Row::new(vec![
                Cell::from(Span::styled(name, theme.value())),
                Cell::from(Line::from(format::bytes(world.bytes)).right_aligned()),
                Cell::from(Line::from(format::bytes(world.region_bytes)).right_aligned()),
                Cell::from(Line::from(format::bytes(world.entity_bytes)).right_aligned()),
                Cell::from(Line::from(format::bytes(world.poi_bytes)).right_aligned()),
                Cell::from(Line::from(format::count(world.files)).right_aligned()),
                Cell::from(Line::from(format::percent(share)).right_aligned()),
                Cell::from(widgets::meter(share, meter_width, theme, Health::Good)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(16),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(meter_width),
    ];

    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .style(theme.value())
            .column_spacing(1),
        inner,
    );
}

fn no_worlds(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let lines = if app.disk.scanning {
        vec![
            Line::default(),
            Line::styled("Measuring…", theme.value()),
            Line::default(),
            Line::styled(
                "A busy overworld is hundreds of thousands of files, so the first",
                theme.label(),
            ),
            Line::styled("scan can take a while.", theme.label()),
        ]
    } else {
        vec![
            Line::default(),
            Line::styled("No world folders are configured.", theme.value()),
            Line::default(),
            Line::styled(
                "Set [server].directory to the server's folder and mctop will find the",
                theme.label(),
            ),
            Line::styled(
                "worlds under it, or list them explicitly as [[world]] entries:",
                theme.label(),
            ),
            Line::default(),
            Line::styled("    [[world]]", Style::default().fg(theme.info)),
            Line::styled(
                "    path = \"/srv/minecraft/world\"",
                Style::default().fg(theme.info),
            ),
        ]
    };

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_summary(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Storage");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [left, right] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(inner);
    let width = left.width.saturating_sub(2);

    let files: u64 = app.disk.worlds.iter().map(|world| world.files).sum();
    let mut left_lines = vec![
        widgets::field(
            "Worlds measured",
            app.disk.worlds.len().to_string(),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Total on disk",
            format::bytes(app.disk.total()),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field("Files", format::count(files), width, theme, Health::Unknown),
        widgets::field(
            "Rescan every",
            format::duration(app.config.refresh.disk()),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    if app.disk.worlds.iter().any(|world| world.partial) {
        left_lines.push(Line::default());
        left_lines.push(Line::styled(
            "* some files could not be read; totals are a floor",
            theme.style(Health::Warn),
        ));
    }

    let right_lines = match app.disk.free {
        Some((free, total)) => {
            let used = total.saturating_sub(free);
            let fraction = used as f64 / total.max(1) as f64;
            let health = theme.load_health(Some(fraction));
            vec![
                widgets::field(
                    "Filesystem",
                    format::bytes(total),
                    width,
                    theme,
                    Health::Unknown,
                ),
                widgets::field("Used", format::bytes(used), width, theme, health),
                widgets::field("Free", format::bytes(free), width, theme, health),
                widgets::meter(fraction, width, theme, health),
                Line::default(),
                widgets::field(
                    "Worlds' share of it",
                    format::percent(app.disk.total() as f64 / total.max(1) as f64),
                    width,
                    theme,
                    Health::Unknown,
                ),
            ]
        }
        None => vec![Line::styled("Filesystem usage unavailable", theme.label())],
    };

    frame.render_widget(Paragraph::new(left_lines), left);
    frame.render_widget(Paragraph::new(right_lines), right);
}
