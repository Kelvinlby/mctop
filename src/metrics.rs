//! The shape of everything mctop measures, plus the ring buffers that keep a
//! little history of each number so the charts have something to draw.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// A fixed-capacity series of timestamped samples.
#[derive(Debug, Clone)]
pub struct History {
    samples: VecDeque<(Instant, f64)>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(2),
        }
    }

    pub fn push(&mut self, value: f64) {
        self.push_at(Instant::now(), value);
    }

    pub fn push_at(&mut self, at: Instant, value: f64) {
        if !value.is_finite() {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back((at, value));
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn last(&self) -> Option<f64> {
        self.samples.back().map(|&(_, value)| value)
    }

    /// The most recent `count` values, oldest first.
    pub fn tail(&self, count: usize) -> Vec<f64> {
        let skip = self.samples.len().saturating_sub(count);
        self.samples
            .iter()
            .skip(skip)
            .map(|&(_, value)| value)
            .collect()
    }

    /// Points for a line chart, x running from `-age_in_seconds` to 0.
    pub fn points(&self, count: usize) -> Vec<(f64, f64)> {
        let Some(&(newest, _)) = self.samples.back() else {
            return Vec::new();
        };
        let skip = self.samples.len().saturating_sub(count);
        self.samples
            .iter()
            .skip(skip)
            .map(|&(at, value)| (-(newest.saturating_duration_since(at).as_secs_f64()), value))
            .collect()
    }

    /// Smallest and largest value held, if any.
    pub fn bounds(&self) -> Option<(f64, f64)> {
        self.samples.iter().fold(None, |bounds, &(_, value)| {
            Some(match bounds {
                None => (value, value),
                Some((low, high)) => (low.min(value), high.max(value)),
            })
        })
    }

    /// Smallest value within `window` of the newest sample.
    pub fn min_over(&self, window: Duration) -> Option<f64> {
        let &(newest, _) = self.samples.back()?;
        let cutoff = newest.checked_sub(window)?;
        self.samples
            .iter()
            .rev()
            .take_while(|&&(at, _)| at >= cutoff)
            .map(|&(_, value)| value)
            .fold(None, |min: Option<f64>, value| {
                Some(min.map_or(value, |min| min.min(value)))
            })
    }
}

/// Tick rate, averaged over one or more windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TpsReading {
    /// Window label and rate, e.g. `("1m", 19.98)`, oldest window last.
    pub windows: Vec<(String, f64)>,
}

impl TpsReading {
    /// The shortest window's rate, which is the most responsive one.
    pub fn current(&self) -> Option<f64> {
        self.windows.first().map(|&(_, value)| value)
    }

    pub fn window(&self, label: &str) -> Option<f64> {
        self.windows
            .iter()
            .find(|(name, _)| name == label)
            .map(|&(_, value)| value)
    }
}

/// Milliseconds per tick over one window, as Paper and Folia report it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsptWindow {
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
}

/// Tick durations, averaged over one or more windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MsptReading {
    /// Window label and durations, shortest window first.
    pub windows: Vec<(String, MsptWindow)>,
}

impl MsptReading {
    pub fn current(&self) -> Option<MsptWindow> {
        self.windows.first().map(|&(_, window)| window)
    }
}

/// One Folia region: an independently ticking slice of a world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Region {
    /// World the region belongs to, when the server names it.
    pub world: Option<String>,
    /// Centre chunk of the region, when the server reports it.
    pub chunk: Option<(i64, i64)>,
    pub tps: Option<f64>,
    pub mspt: Option<f64>,
    /// Fraction of the tick budget consumed, 0.0 to 1.0 and occasionally above.
    pub utilisation: Option<f64>,
    pub players: Option<u32>,
    pub entities: Option<u32>,
    pub chunks: Option<u32>,
}

impl Region {
    /// A stable label for tables and charts.
    pub fn label(&self) -> String {
        match (&self.world, self.chunk) {
            (Some(world), Some((x, z))) => format!("{world} ({x}, {z})"),
            (Some(world), None) => world.clone(),
            (None, Some((x, z))) => format!("({x}, {z})"),
            (None, None) => "region".into(),
        }
    }

    /// How close this region is to falling behind, 0.0 (idle) upwards. Derived
    /// from whichever of utilisation, MSPT, or TPS the server reported.
    pub fn pressure(&self) -> f64 {
        if let Some(utilisation) = self.utilisation {
            return utilisation;
        }
        if let Some(mspt) = self.mspt {
            return mspt / 50.0;
        }
        match self.tps {
            Some(tps) if tps > 0.0 => (20.0 / tps).min(4.0),
            _ => 0.0,
        }
    }
}

/// The server's own account of its regions.
#[derive(Debug, Clone, Default)]
pub struct RegionReport {
    pub regions: Vec<Region>,
    /// Region count as reported, which may exceed the number detailed above.
    pub total: Option<u32>,
    /// Threads the server dedicates to ticking regions.
    pub threads: Option<u32>,
}

impl RegionReport {
    pub fn worst(&self) -> Option<&Region> {
        self.regions
            .iter()
            .max_by(|a, b| a.pressure().total_cmp(&b.pressure()))
    }
}

/// Who is connected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Players {
    pub online: u32,
    pub max: Option<u32>,
    pub names: Vec<String>,
}

/// CPU, memory, and lifetime of the server process.
#[derive(Debug, Clone)]
pub struct ProcessStats {
    pub pid: u32,
    /// CPU use as a percentage of one core, so 400.0 means four cores busy.
    pub cpu_percent: f64,
    /// Resident set size in bytes.
    pub rss: u64,
    /// Virtual size in bytes.
    pub virtual_size: u64,
    pub threads: Option<u32>,
    pub uptime: Duration,
    /// Cores visible to the machine, for normalising `cpu_percent`.
    pub cores: usize,
    /// System-wide load averages, where the platform provides them.
    pub load_average: Option<[f64; 3]>,
    /// Memory used across the whole machine, and its total.
    pub system_memory: (u64, u64),
}

impl ProcessStats {
    /// CPU use as a fraction of the whole machine, 0.0 to 1.0.
    pub fn cpu_fraction(&self) -> f64 {
        if self.cores == 0 {
            return 0.0;
        }
        self.cpu_percent / (100.0 * self.cores as f64)
    }
}

/// Java heap occupancy and collector activity.
#[derive(Debug, Clone, Default)]
pub struct HeapStats {
    /// Bytes in use at the moment of sampling.
    pub used: Option<u64>,
    /// Bytes the JVM has reserved from the operating system.
    pub committed: Option<u64>,
    /// The `-Xmx` ceiling.
    pub max: Option<u64>,
    /// Occupancy after the most recent collection: the number that says whether
    /// the server is genuinely running out of room.
    pub after_gc: Option<u64>,
    /// Whether `after_gc` was observed right after a collection, or inferred
    /// from the heap's low-water mark.
    pub after_gc_measured: bool,
    /// Young and full collection counts since the JVM started.
    pub young_collections: Option<u64>,
    pub full_collections: Option<u64>,
    /// Total seconds spent collecting since the JVM started.
    pub gc_seconds: Option<f64>,
    /// Share of recent wall-clock time spent collecting, 0.0 to 1.0.
    pub gc_load: Option<f64>,
    /// Non-heap (metaspace and friends) usage in bytes.
    pub non_heap: Option<u64>,
}

impl HeapStats {
    /// Occupancy after the last collection as a fraction of the ceiling.
    pub fn pressure(&self) -> Option<f64> {
        let after_gc = self.after_gc? as f64;
        let max = self.max? as f64;
        (max > 0.0).then_some(after_gc / max)
    }
}

/// One world folder's footprint.
#[derive(Debug, Clone)]
pub struct WorldUsage {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    /// Bytes held by `region/`, `entities/`, and `poi/` specifically.
    pub region_bytes: u64,
    pub entity_bytes: u64,
    pub poi_bytes: u64,
    /// Whether the walk finished or was cut short by an unreadable directory.
    pub partial: bool,
}

/// Disk footprint of every world, plus the free space they share.
#[derive(Debug, Clone, Default)]
pub struct DiskUsage {
    pub worlds: Vec<WorldUsage>,
    /// Free and total bytes on the filesystem holding the first world.
    pub free: Option<(u64, u64)>,
    pub scanned_at: Option<SystemTime>,
    pub scanning: bool,
}

impl DiskUsage {
    pub fn total(&self) -> u64 {
        self.worlds.iter().map(|world| world.bytes).sum()
    }
}

/// What the server says it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerIdentity {
    /// `Folia`, `Paper`, and so on, when recognisable.
    pub flavour: Option<String>,
    pub minecraft_version: Option<String>,
    /// The full, unparsed version line.
    pub raw: Option<String>,
}

impl ServerIdentity {
    pub fn is_folia(&self) -> bool {
        self.flavour
            .as_deref()
            .is_some_and(|flavour| flavour.eq_ignore_ascii_case("folia"))
    }

    pub fn summary(&self) -> String {
        match (&self.flavour, &self.minecraft_version) {
            (Some(flavour), Some(version)) => format!("{flavour} {version}"),
            (Some(flavour), None) => flavour.clone(),
            (None, Some(version)) => version.clone(),
            (None, None) => "unknown".into(),
        }
    }
}

/// Connection state of the RCON link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Connecting,
    Up,
    Down(String),
}

impl Link {
    pub fn is_up(&self) -> bool {
        matches!(self, Self::Up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_drops_the_oldest_sample() {
        let mut history = History::new(3);
        for value in [1.0, 2.0, 3.0, 4.0] {
            history.push(value);
        }
        assert_eq!(history.tail(10), vec![2.0, 3.0, 4.0]);
        assert_eq!(history.last(), Some(4.0));
        assert_eq!(history.bounds(), Some((2.0, 4.0)));
    }

    #[test]
    fn history_ignores_nonfinite_values() {
        let mut history = History::new(4);
        history.push(f64::NAN);
        history.push(f64::INFINITY);
        assert!(history.is_empty());
    }

    #[test]
    fn history_points_run_backwards_from_zero() {
        let mut history = History::new(4);
        let now = Instant::now();
        history.push_at(now - Duration::from_secs(2), 1.0);
        history.push_at(now, 2.0);

        let points = history.points(10);
        assert_eq!(points.len(), 2);
        assert!((points[0].0 + 2.0).abs() < 0.05);
        assert_eq!(points[1].0, 0.0);
    }

    #[test]
    fn history_min_over_a_window_ignores_older_samples() {
        let mut history = History::new(8);
        let now = Instant::now();
        history.push_at(now - Duration::from_secs(600), 1.0);
        history.push_at(now - Duration::from_secs(10), 5.0);
        history.push_at(now, 7.0);

        assert_eq!(history.min_over(Duration::from_secs(60)), Some(5.0));
        assert_eq!(history.min_over(Duration::from_secs(3600)), Some(1.0));
    }

    #[test]
    fn region_pressure_prefers_utilisation_then_mspt_then_tps() {
        let region = Region {
            utilisation: Some(0.8),
            mspt: Some(50.0),
            tps: Some(10.0),
            ..Region::default()
        };
        assert_eq!(region.pressure(), 0.8);

        let region = Region {
            mspt: Some(25.0),
            tps: Some(10.0),
            ..Region::default()
        };
        assert_eq!(region.pressure(), 0.5);

        let region = Region {
            tps: Some(10.0),
            ..Region::default()
        };
        assert_eq!(region.pressure(), 2.0);
        assert_eq!(Region::default().pressure(), 0.0);
    }

    #[test]
    fn region_labels_degrade_gracefully() {
        let region = Region {
            world: Some("world".into()),
            chunk: Some((-12, 44)),
            ..Region::default()
        };
        assert_eq!(region.label(), "world (-12, 44)");
        assert_eq!(Region::default().label(), "region");
    }

    #[test]
    fn worst_region_is_the_one_under_most_pressure() {
        let report = RegionReport {
            regions: vec![
                Region {
                    world: Some("a".into()),
                    utilisation: Some(0.2),
                    ..Region::default()
                },
                Region {
                    world: Some("b".into()),
                    utilisation: Some(0.9),
                    ..Region::default()
                },
            ],
            ..RegionReport::default()
        };
        assert_eq!(report.worst().unwrap().world.as_deref(), Some("b"));
    }

    #[test]
    fn heap_pressure_needs_both_ends() {
        let heap = HeapStats {
            after_gc: Some(2 << 30),
            max: Some(4 << 30),
            ..HeapStats::default()
        };
        assert_eq!(heap.pressure(), Some(0.5));
        assert_eq!(HeapStats::default().pressure(), None);
    }
}
