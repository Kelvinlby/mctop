//! Application state: everything the interface draws, and how keys change it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::metrics::{
    DiskUsage, HeapStats, History, Link, MsptReading, Players, ProcessStats, Region, RegionReport,
    ServerIdentity, TpsReading,
};
use crate::source::{Kind, LogEntry, Update};

/// The screens mctop offers.
///
/// The split exists because a Folia server can hold hundreds of regions, and a
/// wall of them would bury the handful of numbers an operator actually watches.
/// Overview answers "is the server healthy"; the rest answer "why not".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tab {
    Overview,
    Regions,
    System,
    Worlds,
    Console,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Overview,
        Tab::Regions,
        Tab::System,
        Tab::Worlds,
        Tab::Console,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Regions => "Regions",
            Tab::System => "System",
            Tab::Worlds => "Worlds",
            Tab::Console => "Console",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&tab| tab == self).unwrap_or(0)
    }

    fn step(self, delta: isize) -> Self {
        let count = Self::ALL.len() as isize;
        let index = (self.index() as isize + delta).rem_euclid(count);
        Self::ALL[index as usize]
    }
}

/// How the region table is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionSort {
    Pressure,
    Tps,
    Mspt,
    Players,
    Entities,
    Chunks,
    World,
}

impl RegionSort {
    const ALL: [RegionSort; 7] = [
        RegionSort::Pressure,
        RegionSort::Tps,
        RegionSort::Mspt,
        RegionSort::Players,
        RegionSort::Entities,
        RegionSort::Chunks,
        RegionSort::World,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RegionSort::Pressure => "load",
            RegionSort::Tps => "TPS",
            RegionSort::Mspt => "MSPT",
            RegionSort::Players => "players",
            RegionSort::Entities => "entities",
            RegionSort::Chunks => "chunks",
            RegionSort::World => "world",
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL.iter().position(|&sort| sort == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// What the interface asks of the outside world after handling a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    SetPaused(bool),
    /// Run this command on the server.
    Send(String),
}

/// A single line of editable text.
///
/// Held as characters rather than bytes so that moving the cursor cannot land
/// between the halves of a multi-byte character — player names and chat
/// messages are not all ASCII.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    chars: Vec<char>,
    /// Where the next character goes, counted in characters, `0..=len`.
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Cursor offset in characters, for placing the terminal's own cursor.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert(&mut self, ch: char) {
        self.chars.insert(self.cursor, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    /// Delete back to the start of the current word, as Ctrl-W does in a shell.
    pub fn delete_word(&mut self) {
        let mut end = self.cursor;
        while end > 0 && self.chars[end - 1].is_whitespace() {
            end -= 1;
        }
        while end > 0 && !self.chars[end - 1].is_whitespace() {
            end -= 1;
        }
        self.chars.drain(end..self.cursor);
        self.cursor = end;
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Take the text and leave the field empty.
    pub fn take(&mut self) -> String {
        let text = self.text();
        self.clear();
        text
    }
}

/// Everything on screen.
pub struct App {
    pub config: Arc<Config>,
    pub tab: Tab,
    pub link: Link,
    pub identity: ServerIdentity,
    pub tps: TpsReading,
    pub mspt: MsptReading,
    pub regions: RegionReport,
    pub players: Players,
    pub process: Option<ProcessStats>,
    pub heap: HeapStats,
    /// Why the heap could not be read, when it could not be.
    pub heap_error: Option<String>,
    pub disk: DiskUsage,

    /// Series behind the charts.
    pub tps_history: History,
    pub mspt_history: History,
    pub cpu_history: History,
    pub heap_history: History,
    pub player_history: History,

    pub log: VecDeque<LogEntry>,
    /// The most recent raw poll responses, newest first. A diagnostic view,
    /// kept out of the console scrollback so that a command running once a
    /// second does not bury what the operator typed.
    pub raw: VecDeque<(String, String)>,
    pub show_raw: bool,

    /// The console's command line.
    pub input: Input,
    /// Whether typing goes to the command line rather than the global keys.
    /// True whenever the Console tab is open, until Esc says otherwise.
    pub input_focused: bool,
    /// Commands sent this session, oldest first, walked with Up and Down.
    pub history: Vec<String>,
    /// Where in `history` the Up key has reached; `None` means the live line.
    pub history_index: Option<usize>,

    pub region_sort: RegionSort,
    pub region_selected: usize,
    /// First visible row of the region table. Owned here rather than rebuilt
    /// each frame so that moving the selection scrolls the view by a row
    /// instead of jumping it to put the selection at the edge.
    pub region_offset: usize,
    pub log_scroll: usize,

    pub paused: bool,
    pub show_help: bool,
    pub started: Instant,
    /// When a reading last arrived, so a frozen display is visible as frozen.
    pub last_update: Option<Instant>,
}

impl App {
    pub fn new(config: Arc<Config>) -> Self {
        let capacity = config.ui.history.clamp(16, 4096);
        Self {
            tab: Tab::Overview,
            link: Link::Connecting,
            identity: ServerIdentity::default(),
            tps: TpsReading::default(),
            mspt: MsptReading::default(),
            regions: RegionReport::default(),
            players: Players::default(),
            process: None,
            heap: HeapStats::default(),
            heap_error: None,
            disk: DiskUsage::default(),
            tps_history: History::new(capacity),
            mspt_history: History::new(capacity),
            cpu_history: History::new(capacity),
            heap_history: History::new(capacity),
            player_history: History::new(capacity),
            log: VecDeque::new(),
            raw: VecDeque::new(),
            show_raw: false,
            input: Input::default(),
            input_focused: true,
            history: Vec::new(),
            history_index: None,
            region_sort: RegionSort::Pressure,
            region_selected: 0,
            region_offset: 0,
            log_scroll: 0,
            paused: false,
            show_help: false,
            started: Instant::now(),
            last_update: None,
            config,
        }
    }

    /// Fold one collector reading into the state.
    pub fn apply(&mut self, update: Update) {
        self.last_update = Some(Instant::now());

        match update {
            Update::Link(link) => self.link = link,
            Update::Identity(identity) => self.identity = identity,
            Update::Tps(tps) => {
                if let Some(value) = tps.current() {
                    self.tps_history.push(value);
                }
                self.tps = tps;
            }
            Update::Mspt(mspt) => {
                if let Some(window) = mspt.current() {
                    self.mspt_history.push(window.average);
                }
                self.mspt = mspt;
            }
            Update::Regions(regions) => {
                self.regions = regions;
                self.clamp_region_selection();
            }
            Update::Players(players) => {
                self.player_history.push(f64::from(players.online));
                self.players = players;
            }
            Update::Process(process) => {
                if let Some(stats) = &process {
                    self.cpu_history.push(stats.cpu_percent);
                }
                self.process = process;
            }
            Update::Heap { stats: heap, error } => {
                self.heap_error = error;
                let heap = heap.unwrap_or_default();
                if let Some(used) = heap.used {
                    self.heap_history.push(used as f64);
                }
                self.heap = heap;
            }
            Update::Disk(disk) => self.disk = disk,
            Update::DiskScanStarted => self.disk.scanning = true,
            Update::Log(entry) => self.push_log(entry),
            Update::Raw { command, response } => {
                self.raw.push_front((command, response));
                self.raw.truncate(32);
            }
        }
    }

    fn push_log(&mut self, entry: LogEntry) {
        // Collapse a complaint repeated back to back; a server that is down
        // otherwise fills the scrollback with one line over and over. Console
        // traffic is never collapsed: running `list` twice should show twice,
        // and a reply that repeats a line is the server's business, not ours.
        let noisy = matches!(entry.kind, Kind::Warn | Kind::Error);
        if noisy
            && self
                .log
                .back()
                .is_some_and(|last| last.message == entry.message && last.kind == entry.kind)
        {
            return;
        }

        // Someone reading back through the scrollback should stay where they
        // are when a new line lands, not be dragged forward by it.
        if self.log_scroll > 0 {
            self.log_scroll += 1;
        }

        self.log.push_back(entry);
        while self.log.len() > self.config.ui.log_lines.max(16) {
            self.log.pop_front();
            self.log_scroll = self.log_scroll.saturating_sub(1);
        }
    }

    pub fn note(&mut self, kind: Kind, message: impl Into<String>) {
        self.push_log(LogEntry::new(kind, message));
    }

    /// Regions in the order the table shows them.
    pub fn sorted_regions(&self) -> Vec<&Region> {
        let mut regions: Vec<&Region> = self.regions.regions.iter().collect();
        match self.region_sort {
            RegionSort::Pressure => {
                regions.sort_by(|a, b| b.pressure().total_cmp(&a.pressure()));
            }
            // Least healthy first, so the interesting rows are always on top.
            RegionSort::Tps => regions.sort_by(|a, b| {
                a.tps
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b.tps.unwrap_or(f64::MAX))
            }),
            RegionSort::Mspt => {
                regions.sort_by(|a, b| b.mspt.unwrap_or(0.0).total_cmp(&a.mspt.unwrap_or(0.0)))
            }
            RegionSort::Players => regions.sort_by_key(|region| std::cmp::Reverse(region.players)),
            RegionSort::Entities => {
                regions.sort_by_key(|region| std::cmp::Reverse(region.entities))
            }
            RegionSort::Chunks => regions.sort_by_key(|region| std::cmp::Reverse(region.chunks)),
            RegionSort::World => regions.sort_by_key(|region| region.label()),
        }
        regions
    }

    pub fn selected_region(&self) -> Option<Region> {
        self.sorted_regions()
            .get(self.region_selected)
            .map(|&region| region.clone())
    }

    fn clamp_region_selection(&mut self) {
        let count = self.regions.regions.len();
        self.region_selected = self.region_selected.min(count.saturating_sub(1));
        if count == 0 {
            self.region_selected = 0;
            self.region_offset = 0;
        }
        self.region_offset = self.region_offset.min(self.region_selected);
    }

    /// Whether the Regions tab has anything to say. Paper ticks one region per
    /// world, so only Folia fills this in.
    pub fn has_regions(&self) -> bool {
        !self.regions.regions.is_empty() || self.regions.total.is_some()
    }

    /// Time since the last reading, for the staleness indicator.
    pub fn staleness(&self) -> Option<Duration> {
        self.last_update.map(|at| at.elapsed())
    }

    /// Handle a key press. Returns what the caller should do about it.
    pub fn on_key(&mut self, key: Key) -> Action {
        // While the help is up, any key dismisses it and does nothing else —
        // which is what the overlay itself promises. Ctrl-C still quits, since
        // an escape hatch that a modal can swallow is not an escape hatch.
        if self.show_help {
            self.show_help = false;
            return if key == Key::Ctrl('c') {
                Action::Quit
            } else {
                Action::None
            };
        }

        // On the Console tab the command line owns the keyboard, so that typing
        // `stop` does not quit on the first keystroke.
        if self.tab == Tab::Console && self.input_focused {
            return self.on_console_key(key);
        }

        match key {
            Key::Char('q') | Key::Ctrl('c') | Key::Esc => Action::Quit,
            Key::Char('?') | Key::Char('h') | Key::F(1) => {
                self.show_help = true;
                Action::None
            }
            Key::Tab | Key::Right => {
                self.go_to(self.tab.step(1));
                Action::None
            }
            Key::BackTab | Key::Left => {
                self.go_to(self.tab.step(-1));
                Action::None
            }
            Key::Char(digit @ '1'..='5') => {
                let index = digit as usize - '1' as usize;
                self.go_to(Tab::ALL[index]);
                Action::None
            }
            Key::Char('r') => Action::Refresh,
            Key::Char('p') | Key::Char(' ') => {
                self.paused = !self.paused;
                Action::SetPaused(self.paused)
            }
            Key::Char('s') if self.tab == Tab::Regions => {
                self.region_sort = self.region_sort.next();
                self.region_selected = 0;
                self.region_offset = 0;
                Action::None
            }
            Key::Char('v') if self.tab == Tab::Console => {
                self.show_raw = !self.show_raw;
                self.log_scroll = 0;
                Action::None
            }
            // Anything that would type a character puts the focus back.
            Key::Enter | Key::Char('i') if self.tab == Tab::Console => {
                self.input_focused = true;
                Action::None
            }
            Key::Down | Key::Char('j') => {
                self.scroll(1);
                Action::None
            }
            Key::Up | Key::Char('k') => {
                self.scroll(-1);
                Action::None
            }
            Key::PageDown => {
                self.scroll(10);
                Action::None
            }
            Key::PageUp => {
                self.scroll(-10);
                Action::None
            }
            Key::Home => {
                self.scroll_to_start();
                Action::None
            }
            Key::End => {
                self.scroll_to_end();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Move to `tab`, focusing the command line when it is the console. A
    /// console you have to click into is a console that gets typed at while
    /// nothing is listening.
    fn go_to(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Console {
            self.input_focused = true;
        }
    }

    /// Keys while the command line has the focus.
    fn on_console_key(&mut self, key: Key) -> Action {
        match key {
            // The one global that a text field must never swallow.
            Key::Ctrl('c') => Action::Quit,

            Key::Enter => match self.input.take() {
                command if command.trim().is_empty() => Action::None,
                command => {
                    let command = command.trim().to_owned();
                    // Repeating the last command should not double the history.
                    if self.history.last() != Some(&command) {
                        self.history.push(command.clone());
                    }
                    self.history_index = None;
                    self.log_scroll = 0;
                    Action::Send(command)
                }
            },

            // Esc clears a half-typed line first, and only then gives the
            // keyboard back — so it always undoes the smaller thing first.
            Key::Esc => {
                if self.input.is_empty() {
                    self.input_focused = false;
                } else {
                    self.input.clear();
                    self.history_index = None;
                }
                Action::None
            }

            Key::Tab => {
                self.go_to(self.tab.step(1));
                Action::None
            }
            Key::BackTab => {
                self.go_to(self.tab.step(-1));
                Action::None
            }

            Key::Up => {
                self.recall(-1);
                Action::None
            }
            Key::Down => {
                self.recall(1);
                Action::None
            }

            Key::Left => {
                self.input.left();
                Action::None
            }
            Key::Right => {
                self.input.right();
                Action::None
            }
            Key::Home | Key::Ctrl('a') => {
                self.input.home();
                Action::None
            }
            Key::End | Key::Ctrl('e') => {
                self.input.end();
                Action::None
            }

            Key::Backspace => {
                self.input.backspace();
                Action::None
            }
            Key::Delete => {
                self.input.delete();
                Action::None
            }
            Key::Ctrl('u') => {
                self.input.clear();
                Action::None
            }
            Key::Ctrl('w') => {
                self.input.delete_word();
                Action::None
            }

            // Scrolling the output stays available while typing.
            Key::PageUp => {
                self.scroll(10);
                Action::None
            }
            Key::PageDown => {
                self.scroll(-10);
                Action::None
            }

            Key::Char(ch) => {
                self.input.insert(ch);
                Action::None
            }

            _ => Action::None,
        }
    }

    /// Step through the command history. `-1` is older, `1` is newer.
    fn recall(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }

        let index = match (self.history_index, delta) {
            // First press of Up lands on the most recent command.
            (None, -1) => self.history.len() - 1,
            (None, _) => return,
            (Some(index), -1) => index.saturating_sub(1),
            (Some(index), _) => index + 1,
        };

        // Walking past the newest entry returns to the empty line.
        if index >= self.history.len() {
            self.history_index = None;
            self.input.clear();
            return;
        }

        self.history_index = Some(index);
        let recalled = self.history[index].clone();
        self.input.set(&recalled);
    }

    fn scroll(&mut self, delta: isize) {
        match self.tab {
            Tab::Regions => {
                let count = self.regions.regions.len();
                if count == 0 {
                    return;
                }
                let index = (self.region_selected as isize + delta).clamp(0, count as isize - 1);
                self.region_selected = index as usize;
            }
            Tab::Console => {
                let count = self.scrollback_len();
                let index = (self.log_scroll as isize + delta).clamp(0, count as isize);
                self.log_scroll = index as usize;
            }
            _ => {}
        }
    }

    fn scroll_to_start(&mut self) {
        match self.tab {
            Tab::Regions => self.region_selected = 0,
            Tab::Console => self.log_scroll = self.scrollback_len(),
            _ => {}
        }
    }

    fn scroll_to_end(&mut self) {
        match self.tab {
            Tab::Regions => {
                self.region_selected = self.regions.regions.len().saturating_sub(1);
            }
            Tab::Console => self.log_scroll = 0,
            _ => {}
        }
    }

    fn scrollback_len(&self) -> usize {
        if self.show_raw {
            self.raw.len().saturating_sub(1)
        } else {
            self.log.len().saturating_sub(1)
        }
    }
}

/// A key press, kept independent of the terminal backend so that state changes
/// can be tested without a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    F(u8),
    Backspace,
    Delete,
    Tab,
    BackTab,
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Region;

    fn app() -> App {
        App::new(Arc::new(Config::default()))
    }

    #[test]
    fn tabs_wrap_in_both_directions() {
        let mut app = app();
        assert_eq!(app.tab, Tab::Overview);

        for _ in 0..Tab::ALL.len() {
            app.on_key(Key::Tab);
        }
        assert_eq!(app.tab, Tab::Overview);

        app.on_key(Key::BackTab);
        assert_eq!(app.tab, Tab::Console);
    }

    #[test]
    fn number_keys_jump_to_a_tab() {
        let mut app = app();
        app.on_key(Key::Char('3'));
        assert_eq!(app.tab, Tab::System);
        app.on_key(Key::Char('1'));
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn quitting_and_refreshing_are_reported_to_the_caller() {
        let mut app = app();
        assert_eq!(app.on_key(Key::Char('r')), Action::Refresh);
        assert_eq!(app.on_key(Key::Char('q')), Action::Quit);
        assert_eq!(app.on_key(Key::Ctrl('c')), Action::Quit);
    }

    #[test]
    fn pausing_toggles_and_reports() {
        let mut app = app();
        assert_eq!(app.on_key(Key::Char('p')), Action::SetPaused(true));
        assert!(app.paused);
        assert_eq!(app.on_key(Key::Char('p')), Action::SetPaused(false));
        assert!(!app.paused);
    }

    #[test]
    fn help_swallows_keys_until_dismissed() {
        let mut app = app();
        app.on_key(Key::Char('?'));
        assert!(app.show_help);

        // A key that would normally switch tabs only closes the help.
        assert_eq!(app.on_key(Key::Char('3')), Action::None);
        assert_eq!(app.tab, Tab::Overview);
        assert!(!app.show_help);

        app.on_key(Key::Char('?'));
        assert_eq!(app.on_key(Key::Ctrl('c')), Action::Quit);
    }

    #[test]
    fn sorting_only_applies_on_the_regions_tab() {
        let mut app = app();
        app.on_key(Key::Char('s'));
        assert_eq!(app.region_sort, RegionSort::Pressure);

        app.tab = Tab::Regions;
        app.on_key(Key::Char('s'));
        assert_eq!(app.region_sort, RegionSort::Tps);
    }

    fn region(world: &str, tps: f64, mspt: f64, players: u32) -> Region {
        Region {
            world: Some(world.into()),
            tps: Some(tps),
            mspt: Some(mspt),
            players: Some(players),
            ..Region::default()
        }
    }

    #[test]
    fn regions_sort_worst_first() {
        let mut app = app();
        app.apply(Update::Regions(RegionReport {
            regions: vec![
                region("calm", 20.0, 2.0, 1),
                region("busy", 14.0, 48.0, 9),
                region("middling", 19.0, 20.0, 4),
            ],
            ..RegionReport::default()
        }));

        app.region_sort = RegionSort::Pressure;
        assert_eq!(app.sorted_regions()[0].world.as_deref(), Some("busy"));

        app.region_sort = RegionSort::Tps;
        assert_eq!(app.sorted_regions()[0].world.as_deref(), Some("busy"));

        app.region_sort = RegionSort::Players;
        assert_eq!(app.sorted_regions()[0].world.as_deref(), Some("busy"));

        app.region_sort = RegionSort::World;
        assert_eq!(app.sorted_regions()[0].world.as_deref(), Some("busy"));
    }

    #[test]
    fn selection_moves_and_stays_in_range() {
        let mut app = app();
        app.tab = Tab::Regions;
        app.apply(Update::Regions(RegionReport {
            regions: vec![region("a", 20.0, 1.0, 0), region("b", 19.0, 2.0, 0)],
            ..RegionReport::default()
        }));

        app.on_key(Key::Down);
        assert_eq!(app.region_selected, 1);
        app.on_key(Key::Down);
        assert_eq!(app.region_selected, 1, "selection stops at the last row");
        app.on_key(Key::PageUp);
        assert_eq!(app.region_selected, 0);
        assert!(app.selected_region().is_some());
    }

    #[test]
    fn the_region_table_offset_never_hides_the_selection() {
        let mut app = app();
        app.tab = Tab::Regions;
        app.apply(Update::Regions(RegionReport {
            regions: (0..40)
                .map(|i| region(&format!("w{i}"), 20.0 - i as f64 * 0.1, 1.0, 0))
                .collect(),
            ..RegionReport::default()
        }));

        // Pretend the table has been scrolled well down the list.
        app.region_offset = 25;
        app.region_selected = 30;

        // Moving the selection above the window pulls the window up with it.
        app.region_selected = 3;
        app.apply(Update::Regions(RegionReport {
            regions: app.regions.regions.clone(),
            ..RegionReport::default()
        }));
        assert!(app.region_offset <= app.region_selected);
    }

    #[test]
    fn a_shrinking_region_list_pulls_the_selection_back() {
        let mut app = app();
        app.apply(Update::Regions(RegionReport {
            regions: vec![region("a", 20.0, 1.0, 0), region("b", 19.0, 2.0, 0)],
            ..RegionReport::default()
        }));
        app.region_selected = 1;

        app.apply(Update::Regions(RegionReport {
            regions: vec![region("a", 20.0, 1.0, 0)],
            ..RegionReport::default()
        }));
        assert_eq!(app.region_selected, 0);

        app.apply(Update::Regions(RegionReport::default()));
        assert_eq!(app.region_selected, 0);
        assert!(app.selected_region().is_none());
    }

    #[test]
    fn readings_feed_the_history_series() {
        let mut app = app();
        app.apply(Update::Tps(TpsReading {
            windows: vec![("1m".into(), 19.5)],
        }));
        app.apply(Update::Tps(TpsReading {
            windows: vec![("1m".into(), 18.5)],
        }));

        assert_eq!(app.tps_history.tail(4), vec![19.5, 18.5]);
        assert_eq!(app.tps.current(), Some(18.5));
        assert!(app.staleness().is_some());
    }

    #[test]
    fn a_refused_heap_is_not_reported_as_a_missing_process() {
        use crate::metrics::ProcessStats;
        use crate::ui::{Unavailable, heap_unavailable, process_unavailable};
        use std::time::Duration;

        let mut app = app();
        app.apply(Update::Process(Some(ProcessStats {
            pid: 4242,
            cpu_percent: 100.0,
            rss: 1 << 30,
            virtual_size: 2 << 30,
            threads: Some(64),
            uptime: Duration::from_secs(60),
            cores: 8,
            load_average: None,
            system_memory: (1 << 30, 8 << 30),
        })));
        app.apply(Update::Heap {
            stats: None,
            error: Some("jstat: Could not attach to 4242".into()),
        });

        // The process is there, so only the heap is missing, and the reason
        // must point at the tools rather than sending the reader hunting for a
        // process that was found perfectly well.
        assert!(matches!(
            heap_unavailable(&app),
            Unavailable::ToolFailed(Some(_))
        ));
        assert_eq!(
            app.heap_error.as_deref(),
            Some("jstat: Could not attach to 4242")
        );

        // With no process at all, it really is the process that is missing.
        app.apply(Update::Process(None));
        assert_eq!(heap_unavailable(&app), Unavailable::NoProcess);
        assert_eq!(process_unavailable(&app), Unavailable::NoProcess);
    }

    #[test]
    fn a_disabled_collector_says_so_rather_than_blaming_the_process() {
        use crate::config::{Config, JvmConfig};
        use crate::ui::{Unavailable, heap_unavailable};

        let app = App::new(Arc::new(Config {
            jvm: JvmConfig {
                enabled: false,
                ..JvmConfig::default()
            },
            ..Config::default()
        }));
        assert_eq!(heap_unavailable(&app), Unavailable::HeapOff);
    }

    #[test]
    fn repeated_log_lines_are_collapsed() {
        let mut app = app();
        app.note(Kind::Warn, "disconnected");
        app.note(Kind::Warn, "disconnected");
        app.note(Kind::Warn, "reconnected");
        assert_eq!(app.log.len(), 2);
    }

    #[test]
    fn the_log_is_bounded() {
        let mut app = app();
        for index in 0..(app.config.ui.log_lines + 50) {
            app.note(Kind::Info, format!("line {index}"));
        }
        assert_eq!(app.log.len(), app.config.ui.log_lines);
    }

    #[test]
    fn raw_responses_are_kept_newest_first_and_bounded() {
        let mut app = app();
        for index in 0..40 {
            app.apply(Update::Raw {
                command: format!("cmd{index}"),
                response: "ok".into(),
            });
        }
        assert_eq!(app.raw.len(), 32);
        assert_eq!(app.raw[0].0, "cmd39");
    }

    /// Type a string into the console, one key at a time.
    fn type_in(app: &mut App, text: &str) -> Vec<Action> {
        text.chars().map(|ch| app.on_key(Key::Char(ch))).collect()
    }

    fn console() -> App {
        let mut app = app();
        app.on_key(Key::Char('5'));
        app
    }

    #[test]
    fn the_console_takes_the_keyboard_when_it_is_open() {
        let mut app = console();
        assert_eq!(app.tab, Tab::Console);
        assert!(
            app.input_focused,
            "the command line should be ready to type"
        );

        // Keys that are global everywhere else are just letters here.
        let actions = type_in(&mut app, "stop");
        assert!(actions.iter().all(|action| *action == Action::None));
        assert_eq!(app.input.text(), "stop");
        assert_eq!(
            app.tab,
            Tab::Console,
            "`p` and `s` must not act as commands"
        );
    }

    #[test]
    fn enter_sends_the_line_and_clears_it() {
        let mut app = console();
        type_in(&mut app, "say hello");

        assert_eq!(app.on_key(Key::Enter), Action::Send("say hello".to_owned()));
        assert!(app.input.is_empty());
        assert_eq!(app.history, ["say hello"]);
    }

    #[test]
    fn an_empty_line_sends_nothing() {
        let mut app = console();
        assert_eq!(app.on_key(Key::Enter), Action::None);

        type_in(&mut app, "   ");
        assert_eq!(app.on_key(Key::Enter), Action::None);
        assert!(app.history.is_empty());
    }

    #[test]
    fn ctrl_c_still_quits_from_inside_the_command_line() {
        let mut app = console();
        type_in(&mut app, "half a command");
        assert_eq!(app.on_key(Key::Ctrl('c')), Action::Quit);
    }

    #[test]
    fn escape_clears_the_line_before_giving_back_the_keys() {
        let mut app = console();
        type_in(&mut app, "oops");

        app.on_key(Key::Esc);
        assert!(app.input.is_empty(), "the first Esc clears");
        assert!(app.input_focused, "and keeps the focus");

        app.on_key(Key::Esc);
        assert!(!app.input_focused, "the second gives the keys back");

        // Now the global keys work again.
        app.on_key(Key::Char('v'));
        assert!(app.show_raw);
        assert_eq!(app.on_key(Key::Char('q')), Action::Quit);
    }

    #[test]
    fn returning_to_the_console_refocuses_the_line() {
        let mut app = console();
        app.on_key(Key::Esc);
        assert!(!app.input_focused);

        app.on_key(Key::Char('1'));
        assert_eq!(app.tab, Tab::Overview);
        app.on_key(Key::Char('5'));
        assert!(app.input_focused);
    }

    #[test]
    fn tab_still_changes_tab_while_typing() {
        let mut app = console();
        type_in(&mut app, "list");

        app.on_key(Key::Tab);
        assert_eq!(
            app.tab,
            Tab::Overview,
            "console is the last tab, so it wraps"
        );
        assert_eq!(app.input.text(), "list", "the half-typed line survives");
    }

    #[test]
    fn the_history_walks_back_and_forward() {
        let mut app = console();
        for command in ["list", "tps", "save-all"] {
            type_in(&mut app, command);
            app.on_key(Key::Enter);
        }

        app.on_key(Key::Up);
        assert_eq!(app.input.text(), "save-all");
        app.on_key(Key::Up);
        assert_eq!(app.input.text(), "tps");
        app.on_key(Key::Up);
        assert_eq!(app.input.text(), "list");
        app.on_key(Key::Up);
        assert_eq!(app.input.text(), "list", "it stops at the oldest");

        app.on_key(Key::Down);
        assert_eq!(app.input.text(), "tps");
        app.on_key(Key::Down);
        assert_eq!(app.input.text(), "save-all");
        app.on_key(Key::Down);
        assert!(
            app.input.is_empty(),
            "walking past the newest empties the line"
        );
    }

    #[test]
    fn the_history_does_not_repeat_the_same_command() {
        let mut app = console();
        for _ in 0..3 {
            type_in(&mut app, "list");
            app.on_key(Key::Enter);
        }
        assert_eq!(app.history, ["list"]);
    }

    #[test]
    fn the_line_can_be_edited_in_the_middle() {
        let mut app = console();
        type_in(&mut app, "sy hello");

        for _ in 0..7 {
            app.on_key(Key::Left);
        }
        app.on_key(Key::Char('a'));
        assert_eq!(app.input.text(), "say hello");

        app.on_key(Key::End);
        app.on_key(Key::Backspace);
        assert_eq!(app.input.text(), "say hell");

        app.on_key(Key::Home);
        app.on_key(Key::Delete);
        assert_eq!(app.input.text(), "ay hell");

        app.on_key(Key::Ctrl('w'));
        assert_eq!(
            app.input.text(),
            "ay hell",
            "nothing to delete before the cursor"
        );
        app.on_key(Key::End);
        app.on_key(Key::Ctrl('w'));
        assert_eq!(app.input.text(), "ay ");

        app.on_key(Key::Ctrl('u'));
        assert!(app.input.is_empty());
    }

    #[test]
    fn the_line_handles_characters_wider_than_a_byte() {
        let mut app = console();
        type_in(&mut app, "say héllo → ✓");

        app.on_key(Key::Home);
        app.on_key(Key::Right);
        app.on_key(Key::Delete);
        assert_eq!(app.input.text(), "sy héllo → ✓");

        app.on_key(Key::End);
        app.on_key(Key::Backspace);
        assert_eq!(app.input.text(), "sy héllo → ");
        assert_eq!(app.input.cursor(), app.input.text().chars().count());
    }

    #[test]
    fn scrolling_back_holds_position_as_new_lines_arrive() {
        let mut app = console();
        for index in 0..20 {
            app.note(Kind::Received, format!("line {index}"));
        }

        app.on_key(Key::PageUp);
        assert_eq!(app.log_scroll, 10);
        let anchored = app.log.len() - app.log_scroll;

        app.note(Kind::Received, "line 20");
        assert_eq!(
            app.log.len() - app.log_scroll,
            anchored,
            "the visible window should not drift"
        );

        // Sending a command returns to the bottom, where the reply will appear.
        type_in(&mut app, "list");
        app.on_key(Key::Enter);
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn console_traffic_is_never_collapsed_but_repeated_complaints_are() {
        let mut app = app();
        for _ in 0..3 {
            app.note(Kind::Sent, "list");
            app.note(Kind::Received, "There are 0 of a max of 20 players online:");
        }
        assert_eq!(app.log.len(), 6, "every command and reply must be shown");

        app.note(Kind::Error, "connection refused");
        app.note(Kind::Error, "connection refused");
        assert_eq!(app.log.len(), 7, "a repeated complaint is collapsed");
    }

    #[test]
    fn regions_are_absent_until_the_server_reports_some() {
        let mut app = app();
        assert!(!app.has_regions());

        app.apply(Update::Regions(RegionReport {
            total: Some(12),
            ..RegionReport::default()
        }));
        assert!(app.has_regions());
    }
}
