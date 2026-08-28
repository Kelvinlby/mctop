//! Java heap readings, taken with the JDK's own tools.
//!
//! Resident set size tells you what the operating system handed the JVM, not
//! what the game is actually holding: a heap that has grown to its ceiling and
//! is mostly garbage looks identical to one that is genuinely full. The number
//! that distinguishes them is occupancy *after* a collection, so that is what
//! this module works to produce.
//!
//! Sampling on a timer never lands on the instant a collection ends, so even a
//! reading taken just after one includes whatever has been allocated since.
//! What does hold is the floor: over a window containing at least one
//! collection, the lowest occupancy seen is a fair estimate of what survived
//! it. So the figure reported is the heap's low-water mark, and `jstat`'s
//! collection counters say whether a collection actually fell inside that
//! window. When none did, the floor may sit well above what a collection would
//! leave behind, and the reading is marked as inferred so the interface can say
//! so rather than overstate how full the heap is.
//!
//! Two tools, because neither is enough on its own.
//!
//! `jstat` reads a file the JVM maps into /tmp, which is cheap and carries the
//! collector's counters — how many collections, and how long they took. But
//! `-XX:+PerfDisableSharedMem` switches that file off, and it is part of
//! Aikar's flags, which is to say part of nearly every Minecraft server's
//! startup line. On those servers jstat can never work, no matter who runs it.
//!
//! `jcmd GC.heap_info` asks the JVM directly over its attach socket, so it
//! works regardless of that flag. It costs more and reports no counters, but it
//! always gives occupancy. So jstat is tried first and jcmd stands behind it,
//! and when only jcmd answers, the interface says why the collector panel is
//! empty rather than showing an error.
//!
//! Both tools must run as the user that owns the server process.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use tokio::process::Command;
use tokio::time;

use crate::config::{JvmConfig, Tool};
use crate::metrics::{HeapStats, History};

/// Longest a JDK tool may take before it is abandoned for this tick.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a heap reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Not yet established.
    Unknown,
    /// `jstat`: cheap, and carries the collector counters.
    Jstat,
    /// `jcmd GC.heap_info`: occupancy only, but works without the perf file.
    Jcmd,
}

/// Samples the heap of one JVM over time.
pub struct HeapWatcher {
    jstat: Tool,
    jcmd: Tool,
    /// Which tool answered last, so the one that failed is not retried on
    /// every sample.
    source: Source,
    window: Duration,
    used: History,
    /// When a collection was last seen to happen, which is what makes the
    /// heap's low-water mark stand for post-collection occupancy.
    last_collection: Option<Instant>,
    /// Collection counts at the previous sample, for spotting a collection.
    previous_counts: Option<(u64, u64)>,
    /// Cumulative collector time and when it was read, for the GC load figure.
    previous_gc_time: Option<(f64, Instant)>,
    /// `-Xmx`, read once; the ceiling does not move.
    max_heap: Option<u64>,
    max_heap_checked: bool,
    /// Whether the JVM was started with `-XX:+PerfDisableSharedMem`, which is
    /// the usual reason jstat cannot attach to a Minecraft server.
    perf_disabled: bool,
    /// Why jstat is not being used, kept to explain the empty collector panel.
    jstat_error: Option<String>,
    /// Why the last attempt failed, surfaced in the log rather than swallowed.
    pub last_error: Option<String>,
}

impl HeapWatcher {
    pub fn new(config: &JvmConfig) -> Self {
        Self {
            jstat: config.jstat.clone(),
            jcmd: config.jcmd.clone(),
            source: Source::Unknown,
            window: Duration::from_secs(config.heap_after_gc_window_secs.max(10)),
            used: History::new(2048),
            last_collection: None,
            previous_counts: None,
            previous_gc_time: None,
            max_heap: None,
            max_heap_checked: false,
            perf_disabled: false,
            jstat_error: None,
            last_error: None,
        }
    }

    /// Sample the heap of `pid`. Returns `None` when neither tool can read it;
    /// the reason is left in [`Self::last_error`].
    pub async fn sample(&mut self, pid: u32) -> Option<HeapStats> {
        if !self.max_heap_checked {
            self.max_heap_checked = true;
            self.read_vm_flags(pid).await;
        }

        // jstat is preferred while it works: one file read, and it is the only
        // source of the collector counters.
        if matches!(self.source, Source::Unknown | Source::Jstat) {
            match self.sample_with_jstat(pid).await {
                Ok(stats) => {
                    self.source = Source::Jstat;
                    self.last_error = None;
                    return Some(stats);
                }
                Err(error) => {
                    // Fall through to jcmd, but remember why jstat is out so
                    // the collector panel can explain itself.
                    self.jstat_error = Some(error);
                }
            }
        }

        match self.sample_with_jcmd(pid).await {
            Ok(stats) => {
                self.source = Source::Jcmd;
                self.last_error = None;
                Some(stats)
            }
            Err(error) => {
                self.last_error = Some(match &self.jstat_error {
                    Some(jstat) => format!("{jstat}; {error}"),
                    None => error,
                });
                None
            }
        }
    }

    /// Occupancy and collector counters from `jstat -gc`.
    async fn sample_with_jstat(&mut self, pid: u32) -> Result<HeapStats, String> {
        let output = run(&self.jstat, &["-gc", &pid.to_string()])
            .await
            .map_err(|error| format!("{}: {error}", self.jstat.label()))?;

        let Some(columns) = parse_jstat(&output) else {
            return Err(format!(
                "could not read `{} -gc {pid}` output",
                self.jstat.label()
            ));
        };

        // jstat reports capacities and occupancies in kilobytes.
        let kib = |name: &str| columns.get(name).map(|&value| (value * 1024.0) as u64);
        let sum = |names: &[&str]| -> Option<u64> {
            let mut total = 0u64;
            let mut seen = false;
            for name in names {
                if let Some(value) = kib(name) {
                    total += value;
                    seen = true;
                }
            }
            seen.then_some(total)
        };

        let used = sum(&["S0U", "S1U", "EU", "OU"]);
        let committed = sum(&["S0C", "S1C", "EC", "OC"]);
        let non_heap = sum(&["MU", "CCSU"]);

        let young = columns.get("YGC").map(|&value| value as u64);
        let full = columns.get("FGC").map(|&value| value as u64);
        // Concurrent-cycle counts appear on newer collectors; fold them in so a
        // ZGC or Shenandoah cycle also counts as a collection.
        let concurrent = columns.get("CGC").map(|&value| value as u64).unwrap_or(0);
        let gc_seconds = columns.get("GCT").copied();

        let now = Instant::now();
        if let Some(used) = used {
            self.used.push_at(now, used as f64);
        }

        let counts = (young.unwrap_or(0) + concurrent, full.unwrap_or(0));
        if self
            .previous_counts
            .is_some_and(|previous| counts.0 > previous.0 || counts.1 > previous.1)
        {
            self.last_collection = Some(now);
        }
        self.previous_counts = Some(counts);

        // The floor over the window, which is only a post-collection figure if
        // a collection happened inside it.
        let after_gc = self
            .used
            .min_over(self.window)
            .map(|value| value.max(0.0) as u64)
            .or(used);
        let measured = self
            .last_collection
            .is_some_and(|at| now.saturating_duration_since(at) <= self.window);

        let gc_load = self.gc_load(gc_seconds, now);

        Ok(HeapStats {
            used,
            committed,
            max: self.max_heap.or(committed),
            after_gc,
            after_gc_measured: measured,
            young_collections: young,
            full_collections: full,
            gc_seconds,
            gc_load,
            non_heap,
            counters_available: true,
            perf_disabled: self.perf_disabled,
        })
    }

    /// Occupancy from `jcmd <pid> GC.heap_info`, which needs no perf file.
    ///
    /// There are no collector counters here, so a collection is inferred from
    /// the heap itself: occupancy only ever falls when something has been
    /// reclaimed, so a drop between samples means a collection happened.
    async fn sample_with_jcmd(&mut self, pid: u32) -> Result<HeapStats, String> {
        let output = run(&self.jcmd, &[&pid.to_string(), "GC.heap_info"])
            .await
            .map_err(|error| format!("{}: {error}", self.jcmd.label()))?;

        let Some(heap) = parse_heap_info(&output) else {
            return Err(format!(
                "could not read `{} {pid} GC.heap_info` output",
                self.jcmd.label()
            ));
        };

        let now = Instant::now();
        if let Some(used) = heap.used {
            if self
                .used
                .last()
                .is_some_and(|previous| (used as f64) < previous)
            {
                self.last_collection = Some(now);
            }
            self.used.push_at(now, used as f64);
        }

        let after_gc = self
            .used
            .min_over(self.window)
            .map(|value| value.max(0.0) as u64)
            .or(heap.used);
        let measured = self
            .last_collection
            .is_some_and(|at| now.saturating_duration_since(at) <= self.window);

        Ok(HeapStats {
            used: heap.used,
            committed: heap.committed,
            max: self.max_heap.or(heap.max).or(heap.committed),
            after_gc,
            after_gc_measured: measured,
            young_collections: None,
            full_collections: None,
            gc_seconds: None,
            gc_load: None,
            non_heap: heap.non_heap,
            counters_available: false,
            perf_disabled: self.perf_disabled,
        })
    }

    /// Share of wall-clock time spent collecting since the previous sample.
    fn gc_load(&mut self, gc_seconds: Option<f64>, now: Instant) -> Option<f64> {
        let gc_seconds = gc_seconds?;
        let load = match self.previous_gc_time {
            Some((previous_seconds, previous_at)) => {
                let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
                let spent = gc_seconds - previous_seconds;
                (elapsed > 0.0 && spent >= 0.0).then(|| (spent / elapsed).clamp(0.0, 1.0))
            }
            None => None,
        };
        self.previous_gc_time = Some((gc_seconds, now));
        load
    }

    /// Read the heap ceiling and the perf-file setting from the JVM's flags.
    ///
    /// Both are fixed for the life of the process, so this runs once. Knowing
    /// about `-XX:+PerfDisableSharedMem` up front turns "jstat could not
    /// attach" from a mystery into a sentence that names the cause.
    async fn read_vm_flags(&mut self, pid: u32) {
        match run(&self.jcmd, &[&pid.to_string(), "VM.flags"]).await {
            Ok(output) => {
                self.max_heap = parse_max_heap(&output);
                self.perf_disabled = output.contains("+PerfDisableSharedMem");
            }
            Err(_) => {
                // Not fatal: jstat may still work, and the committed size
                // stands in for a ceiling we could not read.
            }
        }
    }
}

async fn run(tool: &Tool, args: &[&str]) -> anyhow::Result<String> {
    let Some((program, leading)) = tool.parts() else {
        anyhow::bail!("no program configured");
    };

    // `output()` gives the child a null stdin, so a wrapper such as sudo cannot
    // stop to ask for a password and take the terminal with it — it fails fast
    // instead, which is what the NOPASSWD rule is for.
    let output = time::timeout(
        TOOL_TIMEOUT,
        Command::new(program).args(leading).args(args).output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out"))?
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow::anyhow!("not found on PATH; point [jvm] at your JDK or set enabled = false")
        }
        _ => anyhow::anyhow!("{error}"),
    })?;

    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.lines().next().unwrap_or("failed").trim();
        anyhow::bail!(
            "{}",
            if reason.is_empty() {
                "exited with a failure".to_owned()
            } else {
                reason.to_owned()
            }
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Turn `jstat -gc` output into a column-name to value map.
///
/// Columns are read by name rather than position: which ones appear depends on
/// the collector in use, and G1, ZGC, and Shenandoah each print a different
/// set.
pub fn parse_jstat(output: &str) -> Option<HashMap<String, f64>> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());

    let header = lines.next()?;
    let values = lines.next()?;

    let names: Vec<&str> = header.split_whitespace().collect();
    let values: Vec<&str> = values.split_whitespace().collect();
    if names.is_empty() || names.len() != values.len() {
        return None;
    }

    Some(
        names
            .into_iter()
            .zip(values)
            .filter_map(|(name, value)| Some((name.to_owned(), value.parse::<f64>().ok()?)))
            .filter(|(_, value)| value.is_finite())
            .collect(),
    )
}

/// Heap figures as `jcmd GC.heap_info` reports them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HeapInfo {
    pub used: Option<u64>,
    pub committed: Option<u64>,
    /// Only ZGC states a ceiling here; elsewhere it comes from `VM.flags`.
    pub max: Option<u64>,
    pub non_heap: Option<u64>,
}

/// Parse `jcmd <pid> GC.heap_info`.
///
/// Every collector words this differently. G1 gives one line for the whole
/// heap; Serial and Parallel give a line per generation that has to be added
/// up; ZGC gives occupancy and capacity rather than used and total. What they
/// share is that the numbers are always introduced by a keyword, so the
/// keywords are what this reads, and the per-space breakdown lines — which
/// report a percentage rather than a size — are skipped by construction.
pub fn parse_heap_info(output: &str) -> Option<HeapInfo> {
    static USED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bused\s+(\d+)([KMGB])").unwrap());
    static TOTAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:total|committed|capacity)\s+(\d+)([KMGB])").unwrap()
    });
    static MAX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bmax\s+(?:capacity|heap\s+size)\s+(\d+)([KMGB])").unwrap()
    });

    let scale = |captures: regex::Captures<'_>| -> Option<u64> {
        let value: u64 = captures.get(1)?.as_str().parse().ok()?;
        let unit = captures.get(2)?.as_str();
        Some(match unit.to_ascii_uppercase().as_str() {
            "K" => value * 1024,
            "M" => value * 1024 * 1024,
            "G" => value * 1024 * 1024 * 1024,
            _ => value,
        })
    };

    let mut info = HeapInfo::default();
    let mut used = 0u64;
    let mut committed = 0u64;
    let mut saw_heap = false;

    for line in output.lines() {
        // Class space is counted inside metaspace; counting it too would
        // double it.
        if line.contains("class space") {
            continue;
        }

        if line.contains("Metaspace") {
            info.non_heap = USED.captures(line).and_then(scale);
            continue;
        }

        // ZGC states the ceiling on the same line as the occupancy, and its
        // `max capacity` would otherwise be read a second time as `capacity`.
        let mut rest = line.to_owned();
        if let Some(found) = MAX.find(line) {
            info.max = MAX.captures(line).and_then(scale);
            rest.replace_range(found.range(), " ");
        }

        if let Some(value) = USED.captures(&rest).and_then(scale) {
            used += value;
            saw_heap = true;
        }
        if let Some(value) = TOTAL.captures(&rest).and_then(scale) {
            committed += value;
            saw_heap = true;
        }
    }

    if !saw_heap {
        return None;
    }

    info.used = Some(used);
    info.committed = (committed > 0).then_some(committed);
    Some(info)
}

/// Pull `MaxHeapSize` out of `jcmd <pid> VM.flags`.
pub fn parse_max_heap(output: &str) -> Option<u64> {
    output
        .split_whitespace()
        .find_map(|flag| flag.strip_prefix("-XX:MaxHeapSize="))
        .and_then(|value| value.parse().ok())
        .filter(|&value: &u64| value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G1_OUTPUT: &str = "\
 S0C    S1C    S0U    S1U      EC       EU        OC         OU       MC     MU    CCSC   CCSU   YGC     YGCT    FGC    FGCT    CGC    CGCT     GCT
0.0    8192.0  0.0   8192.0  1024000.0 512000.0 3145728.0  1048576.0 51200.0 49000.0 6400.0 6000.0    182    2.451     2    0.512     4    0.201    3.164";

    #[test]
    fn reads_jstat_columns_by_name() {
        let columns = parse_jstat(G1_OUTPUT).unwrap();
        assert_eq!(columns["S1U"], 8192.0);
        assert_eq!(columns["OU"], 1_048_576.0);
        assert_eq!(columns["YGC"], 182.0);
        assert_eq!(columns["FGC"], 2.0);
        assert_eq!(columns["GCT"], 3.164);
    }

    #[test]
    fn rejects_jstat_output_that_does_not_line_up() {
        assert!(parse_jstat("S0C S1C\n1.0").is_none());
        assert!(parse_jstat("").is_none());
        assert!(parse_jstat("only a header").is_none());
    }

    // Real output, captured from a JDK 21 JVM run with -XX:+PerfDisableSharedMem.
    const G1_HEAP_INFO: &str = "\
1933618:
 garbage-first heap   total 262144K, used 123648K [0x00000000f0000000, 0x0000000100000000)
  region size 1024K, 42 young (43008K), 15 survivors (15360K)
 Metaspace       used 9812K, committed 10048K, reserved 1114112K
  class space    used 1154K, committed 1280K, reserved 1048576K";

    const SERIAL_HEAP_INFO: &str = "\
1934047:
 def new generation   total 78656K, used 26732K [0x00000000f0000000, 0x00000000f5550000, 0x00000000f5550000)
  eden space 69952K,  29% used [0x00000000f0000000, 0x00000000f141b040, 0x00000000f4450000)
  from space 8704K,  70% used [0x00000000f4450000, 0x00000000f4a50290, 0x00000000f4cd0000)
  to   space 8704K,   0% used [0x00000000f4cd0000, 0x00000000f4cd0000, 0x00000000f5550000)
 tenured generation   total 174784K, used 160701K [0x00000000f5550000, 0x0000000100000000, 0x0000000100000000)
   the space 174784K,  91% used [0x00000000f5550000, 0x00000000ff23f6e8, 0x00000000ff23f800, 0x0000000100000000)
 Metaspace       used 9810K, committed 10048K, reserved 1114112K
  class space    used 1153K, committed 1280K, reserved 1048576K";

    const ZGC_HEAP_INFO: &str = "\
1934140:
 ZHeap           used 168M, capacity 256M, max capacity 256M
 Metaspace       used 9807K, committed 10048K, reserved 1114112K
  class space    used 1153K, committed 1280K, reserved 1048576K";

    #[test]
    fn reads_g1_heap_info() {
        let heap = parse_heap_info(G1_HEAP_INFO).unwrap();
        assert_eq!(heap.used, Some(123_648 * 1024));
        assert_eq!(heap.committed, Some(262_144 * 1024));
        // The metaspace line is non-heap, and class space is inside it.
        assert_eq!(heap.non_heap, Some(9_812 * 1024));
        assert_eq!(heap.max, None, "G1 states no ceiling here");
    }

    #[test]
    fn adds_up_the_generations_a_serial_heap_reports() {
        let heap = parse_heap_info(SERIAL_HEAP_INFO).unwrap();
        assert_eq!(heap.used, Some((26_732 + 160_701) * 1024));
        assert_eq!(heap.committed, Some((78_656 + 174_784) * 1024));
        assert_eq!(heap.non_heap, Some(9_810 * 1024));
    }

    #[test]
    fn reads_zgc_which_words_it_differently() {
        let heap = parse_heap_info(ZGC_HEAP_INFO).unwrap();
        assert_eq!(heap.used, Some(168 * 1024 * 1024));
        // `max capacity` must not also be counted as `capacity`.
        assert_eq!(heap.committed, Some(256 * 1024 * 1024));
        assert_eq!(heap.max, Some(256 * 1024 * 1024));
    }

    #[test]
    fn heap_info_that_says_nothing_useful_is_rejected() {
        assert!(parse_heap_info("").is_none());
        assert!(parse_heap_info("12345:\njava.io.IOException: no such pid").is_none());
    }

    #[test]
    fn reads_the_heap_ceiling_from_vm_flags() {
        let output = "-XX:CICompilerCount=4 -XX:MaxHeapSize=8589934592 -XX:+UseG1GC";
        assert_eq!(parse_max_heap(output), Some(8_589_934_592));
        assert_eq!(parse_max_heap("-XX:+UseG1GC"), None);
        assert_eq!(parse_max_heap("-XX:MaxHeapSize=0"), None);
    }

    #[test]
    fn spots_the_flag_that_disables_the_perf_file() {
        // Aikar's flags carry this, which is why jstat fails on most servers.
        let flags = "-XX:+AlwaysPreTouch -XX:+PerfDisableSharedMem -XX:+UseG1GC";
        assert!(flags.contains("+PerfDisableSharedMem"));
        assert!(!"-XX:+UseG1GC".contains("+PerfDisableSharedMem"));
    }

    #[tokio::test]
    async fn a_missing_tool_is_reported_rather_than_panicking() {
        let config = JvmConfig {
            jstat: "mctop-no-such-tool".into(),
            jcmd: "mctop-no-such-tool".into(),
            ..JvmConfig::default()
        };
        let mut watcher = HeapWatcher::new(&config);
        assert!(watcher.sample(std::process::id()).await.is_none());
        assert!(watcher.last_error.unwrap().contains("not found on PATH"));
    }
}
