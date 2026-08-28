//! Drawing. One module per tab, plus the frame around them all.

pub mod console;
pub mod help;
pub mod overview;
pub mod regions;
pub mod system;
pub mod theme;
pub mod widgets;
pub mod worlds;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Tabs};

use crate::app::{App, Tab};
use crate::format;
use crate::metrics::Link;

use theme::{Health, Theme};

/// Draw a whole frame.
///
/// `app` is taken mutably because the region table remembers where it is
/// scrolled to, which is state the widget produces rather than consumes.
pub fn draw(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app, theme);
    draw_tabs(frame, tabs, app, theme);

    match app.tab {
        Tab::Regions => regions::draw(frame, body, app, theme),
        Tab::Overview => overview::draw(frame, body, app, theme),
        Tab::System => system::draw(frame, body, app, theme),
        Tab::Worlds => worlds::draw(frame, body, app, theme),
        Tab::Console => console::draw(frame, body, app, theme),
    }

    draw_footer(frame, footer, app, theme);

    if app.show_help {
        help::draw(frame, frame.area(), theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let separator = || Span::styled("  ·  ", Style::default().fg(theme.border));

    let (indicator, link_text, link_style) = match &app.link {
        Link::Up => ("●", "connected".to_owned(), theme.style(Health::Good)),
        Link::Connecting => ("◌", "connecting".to_owned(), theme.style(Health::Warn)),
        Link::Down(reason) => ("●", format!("down — {reason}"), theme.style(Health::Bad)),
    };

    let mut left = vec![
        Span::styled(
            " mctop ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
        Span::raw(" "),
        Span::styled(app.config.display_name(), theme.strong()),
        separator(),
        Span::styled(app.identity.summary(), Style::default().fg(theme.info)),
        separator(),
        Span::styled(format!("{indicator} "), link_style),
        Span::styled(link_text, link_style),
    ];

    // Uptime is the first thing to go when the header runs out of room; it is
    // also on the System tab.
    if let Some(stats) = &app.process
        && area.width >= 100
    {
        left.push(separator());
        left.push(Span::styled(
            format!("up {}", format::duration(stats.uptime)),
            theme.label(),
        ));
    }

    // Before the first player list arrives, "0 players" would be a claim mctop
    // has no basis for.
    let players = if app.player_history.is_empty() {
        "— players".to_owned()
    } else {
        match app.players.max {
            Some(max) => format!("{} / {max} players", app.players.online),
            None => format!("{} players", app.players.online),
        }
    };

    let mut right = vec![Span::styled(players, theme.value())];
    if app.paused {
        right.push(separator());
        right.push(Span::styled(
            "PAUSED",
            theme.style(Health::Warn).add_modifier(Modifier::BOLD),
        ));
    } else if let Some(staleness) = app.staleness()
        && staleness.as_secs() >= 10
    {
        // A display that has quietly stopped updating is worse than no display.
        right.push(separator());
        right.push(Span::styled(
            format!("stale {}", format::ago(staleness)),
            theme.style(Health::Warn),
        ));
    }
    right.push(Span::raw(" "));

    // Two extra columns so the two runs cannot butt up against each other on a
    // narrow terminal.
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(line_width(&right) + 2),
    ])
    .areas(area);

    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        right_area,
    );
}

fn line_width(spans: &[Span<'_>]) -> u16 {
    spans
        .iter()
        .map(|span| span.content.chars().count() as u16)
        .sum()
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let available = match tab {
                // Regions is dimmed rather than hidden: its absence is itself
                // information, and hiding it would make the tab bar jump about
                // when a server reconnects.
                Tab::Regions => app.has_regions(),
                _ => true,
            };
            Line::from(vec![
                Span::styled(format!("{} ", index + 1), Style::default().fg(theme.border)),
                Span::styled(
                    tab.title(),
                    if available {
                        Style::default()
                    } else {
                        Style::default().fg(theme.dim)
                    },
                ),
            ])
        })
        .collect();

    frame.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .style(Style::default().fg(theme.dim))
            .highlight_style(
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(Span::styled("│", Style::default().fg(theme.border)))
            .padding(" ", " ")
            .block(Block::new()),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut keys: Vec<(&str, &str)> = vec![
        ("q", "quit"),
        ("↹", "tab"),
        ("r", "refresh"),
        ("p", if app.paused { "resume" } else { "pause" }),
    ];

    match app.tab {
        Tab::Regions => {
            keys.push(("↑↓", "select"));
            keys.push(("s", "sort"));
        }
        // The console owns the keyboard while its command line has the focus,
        // so the footer has to show a different set of keys — telling someone
        // to press `q` when `q` types a letter is worse than saying nothing.
        Tab::Console if app.input_focused => {
            // `?` types a question mark here, so offering it as the help key
            // would be a lie. This set replaces the defaults outright.
            return draw_keys(
                frame,
                area,
                theme,
                &[
                    ("^C", "quit"),
                    ("↹", "tab"),
                    ("⏎", "send"),
                    ("↑↓", "history"),
                    ("PgUp/Dn", "scroll"),
                    ("Esc", "keys back"),
                ],
            );
        }
        Tab::Console => {
            keys.push(("⏎", "type a command"));
            keys.push(("↑↓", "scroll"));
            keys.push(("v", if app.show_raw { "console" } else { "raw" }));
        }
        _ => {}
    }
    keys.push(("?", "help"));
    draw_keys(frame, area, theme, &keys);
}

fn draw_keys(frame: &mut Frame, area: Rect, theme: &Theme, keys: &[(&str, &str)]) {
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, description)) in keys.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(
            *key,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {description}"), theme.label()));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Why a locally sampled metric has nothing to show.
///
/// The Overview tiles and the System panels word this differently — one has
/// sixteen columns, the other has a paragraph — but they must never disagree
/// about which case they are in, so the reasoning lives here once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// Local sampling is switched off in the config.
    SamplingOff,
    /// Heap readings specifically are switched off.
    HeapOff,
    /// No process matched, so there is nothing to sample.
    NoProcess,
    /// The process was found, but the JDK tools could not read its heap.
    ToolFailed(Option<String>),
}

impl Unavailable {
    /// A few words, for a quarter-width tile.
    pub fn short(&self) -> &'static str {
        match self {
            Self::SamplingOff => "disabled in config",
            Self::HeapOff => "heap readings off",
            Self::NoProcess => "no server process found",
            Self::ToolFailed(_) => "jstat cannot attach",
        }
    }
}

/// Why there is no CPU or memory reading.
pub fn process_unavailable(app: &App) -> Unavailable {
    if !app.config.process.enabled {
        Unavailable::SamplingOff
    } else {
        Unavailable::NoProcess
    }
}

/// Why there is no heap reading. Distinct from [`process_unavailable`]: a heap
/// can be missing even when the process was found perfectly well, and saying
/// "no server process found" in that case sends the reader hunting for the
/// wrong problem.
pub fn heap_unavailable(app: &App) -> Unavailable {
    if !app.config.process.enabled {
        Unavailable::SamplingOff
    } else if !app.config.jvm.enabled {
        Unavailable::HeapOff
    } else if app.process.is_none() {
        Unavailable::NoProcess
    } else {
        Unavailable::ToolFailed(app.heap_error.clone())
    }
}

/// Split an area into `count` columns of equal width.
pub fn columns<const N: usize>(area: Rect) -> [Rect; N] {
    Layout::horizontal([Constraint::Ratio(1, N as u32); N]).areas(area)
}
