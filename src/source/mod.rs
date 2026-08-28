//! Collectors: everything that produces a number, and the loop that drives them.
//!
//! Each metric has its own clock. TPS is worth a reading every second; the
//! world size is not, and a disk walk that took thirty seconds would otherwise
//! stall the graph that matters most. So the collectors run independently and
//! report as they finish, over a channel the interface drains on every frame.
//!
//! The console collectors and the local ones run as two separate tasks, because
//! a server that has stopped answering RCON is precisely the server whose CPU
//! and memory you want to look at. Sharing one loop would mean a console
//! command waiting out its timeout took the System tab down with it.

pub mod disk;
pub mod jvm;
pub mod parse;
pub mod process;
pub mod rcon;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

use crate::config::Config;
use crate::metrics::{
    DiskUsage, HeapStats, Link, MsptReading, Players, ProcessStats, RegionReport, ServerIdentity,
    TpsReading,
};

use jvm::HeapWatcher;
use process::ProcessWatcher;
use rcon::RconClient;

/// What a line in the console scrollback is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something mctop did.
    Info,
    Warn,
    Error,
    /// A command the operator typed.
    Sent,
    /// A line the server sent back in reply to one.
    Received,
}

/// One line of mctop's own log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: SystemTime,
    pub kind: Kind,
    pub message: String,
}

impl LogEntry {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            at: SystemTime::now(),
            kind,
            message: message.into(),
        }
    }
}

/// A reading, on its way to the interface.
#[derive(Debug)]
pub enum Update {
    Link(Link),
    Tps(TpsReading),
    Mspt(MsptReading),
    Regions(RegionReport),
    Players(Players),
    Identity(ServerIdentity),
    Process(Option<ProcessStats>),
    /// A heap reading, or the reason there is not one. The reason travels with
    /// the reading so the interface can say *why* a panel is empty rather than
    /// guessing at it.
    Heap {
        stats: Option<HeapStats>,
        error: Option<String>,
    },
    Disk(DiskUsage),
    DiskScanStarted,
    Log(LogEntry),
    /// The unparsed response to a console command, kept so that a parser that
    /// does not recognise a fork's wording can be diagnosed from the Log tab
    /// rather than guessed at.
    Raw {
        command: String,
        response: String,
    },
}

/// An instruction from the interface to the collectors.
#[derive(Debug, Clone)]
pub enum Control {
    /// Collect everything now, without waiting for the next interval.
    RefreshNow,
    /// Stop or resume collecting.
    SetPaused(bool),
    /// Run a console command the operator typed, and report what came back.
    Send(String),
}

/// Start the collectors. They run until `updates` is dropped.
pub fn spawn(
    config: Arc<Config>,
    updates: mpsc::Sender<Update>,
    mut controls: mpsc::Receiver<Control>,
) {
    let (console_controls, console_rx) = mpsc::channel(8);
    let (local_controls, local_rx) = mpsc::channel(8);

    tokio::spawn(async move {
        while let Some(control) = controls.recv().await {
            // A typed command concerns the server alone; everything else goes
            // to both. A closed side means that loop has stopped; the other
            // carries on regardless.
            if !matches!(control, Control::Send(_)) {
                let _ = local_controls.send(control.clone()).await;
            }
            let _ = console_controls.send(control).await;
        }
    });

    tokio::spawn(console_loop(
        Arc::clone(&config),
        updates.clone(),
        console_rx,
    ));
    tokio::spawn(local_loop(config, updates, local_rx));
}

/// Everything that has to be asked of the server over RCON.
async fn console_loop(
    config: Arc<Config>,
    updates: mpsc::Sender<Update>,
    mut controls: mpsc::Receiver<Control>,
) {
    let mut engine = match Engine::new(&config, updates.clone()) {
        Ok(engine) => engine,
        Err(error) => {
            log(&updates, Kind::Error, format!("startup failed: {error:#}")).await;
            let _ = updates
                .send(Update::Link(Link::Down(format!("{error}"))))
                .await;
            return;
        }
    };

    let mut tick = interval(config.refresh.tick());
    let mut region = interval(config.refresh.region());
    let mut roster = interval(config.refresh.roster());
    let mut paused = false;

    loop {
        tokio::select! {
            control = controls.recv() => match control {
                Some(Control::RefreshNow) => {
                    engine.poll_ticks().await;
                    engine.poll_regions().await;
                    engine.poll_roster().await;
                }
                Some(Control::SetPaused(value)) => {
                    paused = value;
                    engine.log(
                        Kind::Info,
                        if paused { "collection paused" } else { "collection resumed" },
                    ).await;
                }
                // A typed command runs even while polling is paused: pausing
                // stops mctop from asking questions, not the operator.
                Some(Control::Send(command)) => engine.execute(&command).await,
                // The interface is gone; nothing left to collect for.
                None => return,
            },
            _ = tick.tick(), if !paused => engine.poll_ticks().await,
            _ = region.tick(), if !paused => engine.poll_regions().await,
            _ = roster.tick(), if !paused => engine.poll_roster().await,
        }

        if updates.is_closed() {
            return;
        }
    }
}

/// Everything read from the machine mctop is running on.
async fn local_loop(
    config: Arc<Config>,
    updates: mpsc::Sender<Update>,
    mut controls: mpsc::Receiver<Control>,
) {
    let mut sampler = match Sampler::new(&config, updates.clone()) {
        Ok(sampler) => sampler,
        Err(error) => {
            log(
                &updates,
                Kind::Error,
                format!("local sampling is off: {error:#}"),
            )
            .await;
            return;
        }
    };

    let mut process = interval(config.refresh.process());
    let mut disk = interval(config.refresh.disk());
    let mut paused = false;

    loop {
        tokio::select! {
            control = controls.recv() => match control {
                Some(Control::RefreshNow) => {
                    sampler.sample().await;
                    sampler.start_disk_scan(&config);
                }
                Some(Control::SetPaused(value)) => paused = value,
                // Console commands never reach this loop.
                Some(Control::Send(_)) => {}
                None => return,
            },
            _ = process.tick(), if !paused => sampler.sample().await,
            _ = disk.tick(), if !paused => sampler.start_disk_scan(&config),
        }

        if updates.is_closed() {
            return;
        }
    }
}

async fn send(updates: &mpsc::Sender<Update>, update: Update) {
    let _ = updates.send(update).await;
}

async fn log(updates: &mpsc::Sender<Update>, kind: Kind, message: impl Into<String>) {
    send(updates, Update::Log(LogEntry::new(kind, message))).await;
}

/// An interval that does not try to catch up after a slow poll. Bunching four
/// missed ticks together would hammer a server that is already struggling.
fn interval(period: std::time::Duration) -> time::Interval {
    let mut interval = time::interval(period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval
}

/// The console side: one RCON connection and the commands run over it.
struct Engine {
    rcon: RconClient,
    updates: mpsc::Sender<Update>,
    commands: crate::config::CommandConfig,
    /// Whether the region command is worth running. Turned off after the server
    /// gives no region detail, so a Paper server is not asked every two seconds
    /// for a report only Folia has.
    regions_supported: bool,
    identity_known: bool,
    last_link: Option<Link>,
}

impl Engine {
    fn new(config: &Config, updates: mpsc::Sender<Update>) -> anyhow::Result<Self> {
        Ok(Self {
            rcon: RconClient::new(&config.rcon)?,
            updates,
            commands: config.commands.clone(),
            regions_supported: true,
            identity_known: false,
            last_link: None,
        })
    }

    async fn send(&self, update: Update) {
        send(&self.updates, update).await;
    }

    async fn log(&self, kind: Kind, message: impl Into<String>) {
        log(&self.updates, kind, message).await;
    }

    /// Run a console command, reporting the raw response and any failure.
    async fn run(&mut self, command: &str) -> Option<String> {
        if command.trim().is_empty() {
            return None;
        }

        let result = self.rcon.command(command).await;
        self.report_link().await;

        match result {
            Ok(response) => {
                self.send(Update::Raw {
                    command: command.to_owned(),
                    response: response.clone(),
                })
                .await;
                Some(response)
            }
            Err(error) => {
                // A refusal to retry is expected while the server is down and
                // is not worth a log line of its own each second.
                if !error.to_string().starts_with("waiting to reconnect") {
                    self.log(Kind::Error, format!("{error:#}")).await;
                }
                None
            }
        }
    }

    /// Publish the link state, but only when it has actually changed.
    async fn report_link(&mut self) {
        let link = self.rcon.link().clone();
        if self.last_link.as_ref() == Some(&link) {
            return;
        }

        match &link {
            Link::Up => {
                self.log(Kind::Info, format!("connected to {}", self.rcon.address()))
                    .await;
                // A reconnected server may be a different build entirely.
                self.identity_known = false;
                self.regions_supported = true;
            }
            Link::Down(reason) => {
                self.log(Kind::Warn, format!("disconnected: {reason}"))
                    .await;
            }
            Link::Connecting => {}
        }

        self.last_link = Some(link.clone());
        self.send(Update::Link(link)).await;
    }

    async fn poll_ticks(&mut self) {
        let command = self.commands.tps.clone();
        if let Some(response) = self.run(&command).await {
            let tps = parse::parse_tps(&response);
            if tps.windows.is_empty() {
                self.warn_unparsed(&command, &response).await;
            } else {
                self.send(Update::Tps(tps)).await;
            }

            // On Folia the region breakdown rides along in the same report, so
            // when both point at one command it is read here rather than asked
            // for a second time. A server that is struggling is exactly the one
            // that should not be asked twice.
            if self.commands.regions == command {
                self.publish_regions(&command, &response).await;
            }
        }

        let command = self.commands.mspt.clone();
        if let Some(response) = self.run(&command).await {
            let mspt = parse::parse_mspt(&response);
            if mspt.windows.is_empty() {
                self.warn_unparsed(&command, &response).await;
            } else {
                self.send(Update::Mspt(mspt)).await;
            }
        }
    }

    async fn poll_regions(&mut self) {
        let command = self.commands.regions.clone();
        // Already read alongside the tick rate.
        if command == self.commands.tps || !self.regions_supported {
            return;
        }

        if let Some(response) = self.run(&command).await {
            self.publish_regions(&command, &response).await;
        }
    }

    /// Report the region breakdown, or turn the tab off when this server has
    /// none to give.
    async fn publish_regions(&mut self, command: &str, response: &str) {
        if !self.regions_supported {
            return;
        }

        let report = parse::parse_regions(response);
        if report.regions.is_empty() && report.total.is_none() && report.threads.is_none() {
            self.regions_supported = false;
            self.log(
                Kind::Info,
                format!(
                    "`{command}` reported no regions; the Regions tab is off. \
                     This is expected on Paper, which ticks one region per world."
                ),
            )
            .await;
            return;
        }

        self.send(Update::Regions(report)).await;
    }

    async fn poll_roster(&mut self) {
        if let Some(response) = self.run(&self.commands.players.clone()).await {
            self.send(Update::Players(parse::parse_players(&response)))
                .await;
        }

        if !self.identity_known
            && let Some(response) = self.run(&self.commands.version.clone()).await
        {
            let identity = parse::parse_version(&response);
            self.identity_known = identity.flavour.is_some();
            self.send(Update::Identity(identity)).await;
        }
    }

    /// Run a command the operator typed, putting both it and its reply into
    /// the console scrollback.
    ///
    /// Deliberately separate from [`Engine::run`]: a typed command's reply
    /// belongs on screen in full, not in the ring of raw poll responses, and a
    /// failure here is worth saying out loud rather than swallowing as the
    /// pollers do.
    async fn execute(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        self.log(Kind::Sent, command).await;
        let result = self.rcon.command(command).await;
        self.report_link().await;

        match result {
            Ok(response) => {
                let text = crate::source::parse::strip_formatting(&response);
                let text = text.trim_end();
                if text.is_empty() {
                    // Plenty of commands succeed silently; saying so beats an
                    // interface that looks like it dropped the command.
                    self.log(Kind::Received, "(no output)").await;
                } else {
                    // One entry per line, so scrolling counts lines rather than
                    // responses and a long reply can be paged through.
                    for line in text.lines() {
                        self.log(Kind::Received, line).await;
                    }
                }
            }
            Err(error) => self.log(Kind::Error, format!("{error:#}")).await,
        }
    }

    async fn warn_unparsed(&self, command: &str, response: &str) {
        let excerpt = parse::strip_formatting(response);
        let excerpt = excerpt.lines().next().unwrap_or("").trim();
        self.log(
            Kind::Warn,
            format!(
                "could not read `{command}`: {}. Set [commands] in the config to a command \
                 this server understands.",
                if excerpt.is_empty() {
                    "empty response".to_owned()
                } else {
                    format!("got `{excerpt}`")
                }
            ),
        )
        .await;
    }
}

/// The local side: the server's own process, its heap, and its worlds on disk.
struct Sampler {
    process: Option<ProcessWatcher>,
    heap: Option<HeapWatcher>,
    updates: mpsc::Sender<Update>,
    /// Set while a disk scan is in flight, so scans cannot pile up behind a
    /// world that takes longer to walk than the interval between scans.
    scanning: Arc<AtomicBool>,
}

impl Sampler {
    fn new(config: &Config, updates: mpsc::Sender<Update>) -> anyhow::Result<Self> {
        Ok(Self {
            process: config
                .process
                .enabled
                .then(|| ProcessWatcher::new(&config.process))
                .transpose()?,
            heap: (config.jvm.enabled && config.process.enabled)
                .then(|| HeapWatcher::new(&config.jvm)),
            updates,
            scanning: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn sample(&mut self) {
        let Some(watcher) = self.process.as_mut() else {
            return;
        };

        let stats = watcher.sample();
        let pid = stats.as_ref().map(|stats| stats.pid);
        send(&self.updates, Update::Process(stats)).await;

        let Some(pid) = pid else {
            send(
                &self.updates,
                Update::Heap {
                    stats: None,
                    error: None,
                },
            )
            .await;
            return;
        };

        if let Some(heap) = self.heap.as_mut() {
            let previous_error = heap.last_error.clone();
            let sample = heap.sample(pid).await;
            let error = heap.last_error.clone();
            send(
                &self.updates,
                Update::Heap {
                    stats: sample,
                    error: error.clone(),
                },
            )
            .await;

            // Report a new tool failure once rather than on every sample.
            if let Some(error) = error
                && previous_error.as_deref() != Some(error.as_str())
            {
                log(
                    &self.updates,
                    Kind::Warn,
                    format!("heap unavailable: {error}"),
                )
                .await;
            }
        }
    }

    fn start_disk_scan(&self, config: &Config) {
        let worlds = config.resolved_worlds();
        if worlds.is_empty() {
            return;
        }

        if self.scanning.swap(true, Ordering::SeqCst) {
            return;
        }

        let updates = self.updates.clone();
        let scanning = Arc::clone(&self.scanning);
        tokio::spawn(async move {
            send(&updates, Update::DiskScanStarted).await;
            let usage = tokio::task::spawn_blocking(move || disk::scan(&worlds)).await;
            scanning.store(false, Ordering::SeqCst);

            match usage {
                Ok(usage) => send(&updates, Update::Disk(usage)).await,
                Err(error) => {
                    log(&updates, Kind::Warn, format!("world scan failed: {error}")).await;
                }
            }
        });
    }
}
