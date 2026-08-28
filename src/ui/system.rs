//! The System tab: what the machine and the JVM are doing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::App;
use crate::format;

use super::Unavailable;
use super::theme::{Health, Theme};
use super::widgets;

/// Rows a panel needs before a trend sparkline is worth the space it takes.
const TREND_MARGIN: u16 = 3;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let [top, bottom] =
        Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(area);
    let [cpu, memory] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(top);
    let [heap, collector] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(bottom);

    draw_cpu(frame, cpu, app, theme);
    draw_memory(frame, memory, app, theme);
    draw_heap(frame, heap, app, theme);
    draw_collector(frame, collector, app, theme);
}

fn draw_cpu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Processor");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(stats) = &app.process else {
        unavailable(frame, inner, app, theme);
        return;
    };

    let width = inner.width;
    let fraction = stats.cpu_fraction();
    let health = theme.load_health(Some(fraction));

    let mut lines = vec![
        widgets::field(
            "Server process",
            format!("{:.1}% of one core", stats.cpu_percent),
            width,
            theme,
            health,
        ),
        widgets::field(
            "Share of machine",
            format::percent(fraction),
            width,
            theme,
            health,
        ),
        widgets::meter(fraction, width, theme, health),
        Line::default(),
        widgets::field(
            "Cores",
            stats.cores.to_string(),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Threads",
            format::optional(stats.threads, |threads| threads.to_string()),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Uptime",
            format::duration(stats.uptime),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field("PID", stats.pid.to_string(), width, theme, Health::Unknown),
    ];

    if let Some(load) = stats.load_average {
        lines.push(Line::default());
        for (label, value) in [("1m", load[0]), ("5m", load[1]), ("15m", load[2])] {
            // A load average above the core count means work is queueing, which
            // shows up in game as tick times that will not come down.
            let health = theme.load_health(Some(value / stats.cores.max(1) as f64));
            lines.push(widgets::field(
                format!("Load average {label}"),
                format!("{value:.2}"),
                width,
                theme,
                health,
            ));
        }
    }

    trend(
        frame,
        inner,
        theme,
        &lines,
        app.cpu_history.tail(inner.width as usize),
        health,
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_memory(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Memory");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(stats) = &app.process else {
        unavailable(frame, inner, app, theme);
        return;
    };

    let width = inner.width;
    let (used, total) = stats.system_memory;
    let system_fraction = used as f64 / total.max(1) as f64;
    let system_health = theme.load_health(Some(system_fraction));

    let lines = vec![
        widgets::field(
            "Resident (RSS)",
            format::bytes(stats.rss),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Virtual",
            format::bytes(stats.virtual_size),
            width,
            theme,
            Health::Unknown,
        ),
        // RSS runs well above the heap: the JVM's own structures, mapped chunk
        // files, and direct buffers all live outside it.
        widgets::field(
            "Beyond the heap",
            format::optional(
                app.heap.used.map(|heap| stats.rss.saturating_sub(heap)),
                format::bytes,
            ),
            width,
            theme,
            Health::Unknown,
        ),
        Line::default(),
        widgets::field(
            "Machine memory",
            format!("{} / {}", format::bytes(used), format::bytes(total)),
            width,
            theme,
            system_health,
        ),
        widgets::meter(system_fraction, width, theme, system_health),
        Line::default(),
        widgets::field(
            "Server share",
            format::percent(stats.rss as f64 / total.max(1) as f64),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_heap(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Java heap");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.heap.used.is_none() {
        heap_unavailable(frame, inner, app, theme);
        return;
    }

    let width = inner.width;
    let pressure = app.heap.pressure();
    let health = theme.load_health(pressure);

    let mut lines = vec![
        widgets::field(
            "In use now",
            format::optional(app.heap.used, format::bytes),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            if app.heap.after_gc_measured {
                "After collection"
            } else {
                "After collection (est.)"
            },
            format::optional(app.heap.after_gc, format::bytes),
            width,
            theme,
            health,
        ),
        widgets::field(
            "Committed",
            format::optional(app.heap.committed, format::bytes),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Ceiling (-Xmx)",
            format::optional(app.heap.max, format::bytes),
            width,
            theme,
            Health::Unknown,
        ),
        Line::default(),
        widgets::field(
            "Occupancy after GC",
            format::optional(pressure, format::percent),
            width,
            theme,
            health,
        ),
        widgets::meter(pressure.unwrap_or(0.0), width, theme, health),
        Line::default(),
        widgets::field(
            "Non-heap (metaspace)",
            format::optional(app.heap.non_heap, format::bytes),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    if !app.heap.after_gc_measured {
        lines.push(Line::default());
        lines.push(Line::styled(
            "No collection has been seen recently, so this floor may sit above",
            theme.label(),
        ));
        lines.push(Line::styled(
            "what a collection would actually leave behind.",
            theme.label(),
        ));
    }

    trend(
        frame,
        inner,
        theme,
        &lines,
        app.heap_history.tail(inner.width as usize),
        health,
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_collector(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = theme.panel("Garbage collector");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.heap.young_collections.is_none() && app.heap.gc_seconds.is_none() {
        widgets::placeholder(frame, inner, theme, "no collector statistics available");
        return;
    }

    let width = inner.width;
    // Time in the collector is time not spent ticking the world, so this is the
    // number that connects a memory problem to a gameplay one.
    let load_health = match app.heap.gc_load {
        Some(load) if load > 0.10 => Health::Bad,
        Some(load) if load > 0.03 => Health::Warn,
        Some(_) => Health::Good,
        None => Health::Unknown,
    };

    let mut lines = vec![
        widgets::field(
            "Recent GC load",
            format::optional(app.heap.gc_load, format::percent),
            width,
            theme,
            load_health,
        ),
        widgets::meter(app.heap.gc_load.unwrap_or(0.0), width, theme, load_health),
        Line::default(),
        widgets::field(
            "Young collections",
            format::optional(app.heap.young_collections, format::count),
            width,
            theme,
            Health::Unknown,
        ),
        widgets::field(
            "Full collections",
            format::optional(app.heap.full_collections, format::count),
            width,
            theme,
            // Repeated full collections on a modern collector mean the heap is
            // genuinely too small for the world being ticked.
            match app.heap.full_collections {
                Some(count) if count > 20 => Health::Warn,
                Some(_) => Health::Good,
                None => Health::Unknown,
            },
        ),
        widgets::field(
            "Total time collecting",
            format::optional(app.heap.gc_seconds, |seconds| {
                format::duration(std::time::Duration::from_secs_f64(seconds.max(0.0)))
            }),
            width,
            theme,
            Health::Unknown,
        ),
    ];

    if let Some(stats) = &app.process
        && let Some(gc_seconds) = app.heap.gc_seconds
    {
        let uptime = stats.uptime.as_secs_f64().max(1.0);
        lines.push(widgets::field(
            "Share of lifetime",
            format::percent(gc_seconds / uptime),
            width,
            theme,
            Health::Unknown,
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draw a sparkline in whatever room is left under `lines`, captioned so it is
/// not mistaken for part of the table above it.
fn trend(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    lines: &[Line<'_>],
    values: Vec<f64>,
    health: Health,
) {
    let used = lines.len() as u16 + 1;
    if values.is_empty() || area.height < used + TREND_MARGIN {
        return;
    }

    let caption = Rect {
        y: area.y + used,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Line::styled("recent", theme.label())),
        caption,
    );

    let chart = Rect {
        y: caption.y + 1,
        height: area.height - used - 1,
        ..area
    };
    widgets::sparkline(frame, chart, theme, &values, health);
}

/// Explain an empty heap panel. The interesting case is the last one: the
/// process was found, so CPU and memory are fine, and only the JDK tools are
/// being refused — almost always because they must run as the user that owns
/// the server process, which group membership does not confer.
fn heap_unavailable(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut lines = vec![Line::default()];

    match super::heap_unavailable(app) {
        Unavailable::SamplingOff => lines.push(Line::styled(
            "Process sampling is disabled in the config.",
            theme.value(),
        )),
        Unavailable::HeapOff => lines.push(Line::styled(
            "Heap readings are disabled in the config.",
            theme.value(),
        )),
        Unavailable::NoProcess => lines.push(Line::styled(
            "No server process to read a heap from.",
            theme.value(),
        )),
        Unavailable::ToolFailed(error) => {
            lines.push(Line::styled(
                format!(
                    "Process {} was found, but its heap could not be read.",
                    pid(app)
                ),
                theme.value(),
            ));
            lines.push(Line::default());
            if let Some(error) = error {
                lines.push(Line::styled(error, theme.style(Health::Warn)));
                lines.push(Line::default());
            }
            // The remedy goes above the explanation: on a short terminal the
            // panel is clipped from the bottom, and the command is the part
            // worth keeping.
            lines.push(Line::styled(
                "Run mctop as the user that owns the server:",
                theme.value(),
            ));
            lines.push(Line::styled(
                "    sudo -u <server user> mctop",
                Style::default().fg(theme.info),
            ));
            lines.push(Line::default());
            lines.push(Line::styled(
                "jstat and jcmd attach through a file owned by that user and readable \
                 only by them, so belonging to their group is not enough. Set \
                 [jvm].enabled = false to stop asking.",
                theme.label(),
            ));
        }
    }

    // The panel is half the screen wide and the message is prose, so it has to
    // wrap rather than run off the edge.
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn pid(app: &App) -> String {
    app.process
        .as_ref()
        .map_or_else(|| "?".to_owned(), |stats| stats.pid.to_string())
}

fn unavailable(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let message = match super::process_unavailable(app) {
        Unavailable::SamplingOff => "Process sampling is disabled in the config.",
        _ => "No matching Java process on this machine.",
    };

    let lines = vec![
        Line::default(),
        Line::styled(message, theme.value()),
        Line::default(),
        Line::styled(
            "CPU and memory are read from the local machine, not over RCON. Run mctop on \
             the server's own host, widen [process].match_pattern, or set [process].pid to \
             pin the PID.",
            theme.label(),
        ),
    ];

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
