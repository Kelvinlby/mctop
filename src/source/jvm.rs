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
//! Both tools must run as the user that owns the server process.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time;

use crate::config::{JvmConfig, Tool};
use crate::metrics::{HeapStats, History};

/// Longest a JDK tool may take before it is abandoned for this tick.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

/// Samples the heap of one JVM over time.
pub struct HeapWatcher {
    jstat: Tool,
    jcmd: Tool,
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
    /// Why the last attempt failed, surfaced in the log rather than swallowed.
    pub last_error: Option<String>,
}

impl HeapWatcher {
    pub fn new(config: &JvmConfig) -> Self {
        Self {
            jstat: config.jstat.clone(),
            jcmd: config.jcmd.clone(),
            window: Duration::from_secs(config.heap_after_gc_window_secs.max(10)),
            used: History::new(2048),
            last_collection: None,
            previous_counts: None,
            previous_gc_time: None,
            max_heap: None,
            max_heap_checked: false,
            last_error: None,
        }
    }

    /// Sample the heap of `pid`. Returns `None` when the JDK tools are missing
    /// or refuse to attach; the reason is left in [`Self::last_error`].
    pub async fn sample(&mut self, pid: u32) -> Option<HeapStats> {
        if !self.max_heap_checked {
            self.max_heap_checked = true;
            self.max_heap = self.read_max_heap(pid).await;
        }

        let output = match run(&self.jstat, &["-gc", &pid.to_string()]).await {
            Ok(output) => output,
            Err(error) => {
                self.last_error = Some(format!("{}: {error}", self.jstat.label()));
                return None;
            }
        };

        let Some(columns) = parse_jstat(&output) else {
            self.last_error = Some(format!(
                "could not read `{} -gc {pid}` output",
                self.jstat.label()
            ));
            return None;
        };
        self.last_error = None;

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

        Some(HeapStats {
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

    /// Read `-Xmx` from the running JVM's flags.
    async fn read_max_heap(&mut self, pid: u32) -> Option<u64> {
        match run(&self.jcmd, &[&pid.to_string(), "VM.flags"]).await {
            Ok(output) => parse_max_heap(&output),
            Err(error) => {
                // Only the -Xmx ceiling is lost when jcmd is unavailable, and
                // the committed size stands in for it, so this is not fatal.
                self.last_error = Some(format!("{}: {error}", self.jcmd.label()));
                None
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

    #[test]
    fn reads_the_heap_ceiling_from_vm_flags() {
        let output = "-XX:CICompilerCount=4 -XX:MaxHeapSize=8589934592 -XX:+UseG1GC";
        assert_eq!(parse_max_heap(output), Some(8_589_934_592));
        assert_eq!(parse_max_heap("-XX:+UseG1GC"), None);
        assert_eq!(parse_max_heap("-XX:MaxHeapSize=0"), None);
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
