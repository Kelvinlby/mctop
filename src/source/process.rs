//! CPU and memory for the server's JVM, read from the local machine.
//!
//! These numbers cannot come over RCON: the game has no idea how much CPU it is
//! using. So mctop finds the JVM process itself and samples it. When mctop runs
//! on a different machine from the server, `process.enabled = false` turns this
//! off and the System tab says so instead of showing nothing.

use regex::Regex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::config::ProcessConfig;
use crate::metrics::ProcessStats;

/// Samples one process repeatedly. Kept alive across ticks because CPU use is a
/// delta between two samples, and the first one is always zero.
pub struct ProcessWatcher {
    system: System,
    pattern: Option<Regex>,
    pinned: Option<Pid>,
    current: Option<Pid>,
    cores: usize,
}

impl ProcessWatcher {
    pub fn new(config: &ProcessConfig) -> anyhow::Result<Self> {
        let pattern = if config.pid.is_some() {
            None
        } else {
            Some(
                Regex::new(&config.match_pattern)
                    .map_err(|error| anyhow::anyhow!("process.match_pattern: {error}"))?,
            )
        };

        Ok(Self {
            system: System::new(),
            pattern,
            pinned: config.pid.map(Pid::from_u32),
            current: None,
            cores: System::physical_core_count().unwrap_or_else(num_cpus_fallback),
        })
    }

    /// Take a sample. Returns `None` while no matching process is running.
    pub fn sample(&mut self) -> Option<ProcessStats> {
        let refresh = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_tasks()
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet);

        // Re-scan every process only while looking for the server; once found,
        // refreshing the single PID is far cheaper.
        match self.current {
            Some(pid) => {
                self.system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    refresh,
                );
                if self.system.process(pid).is_none() {
                    self.current = None;
                }
            }
            None => {
                self.system
                    .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
                self.current = self.find();
            }
        }

        let pid = self.current?;
        self.system.refresh_memory();
        let process = self.system.process(pid)?;

        Some(ProcessStats {
            pid: pid.as_u32(),
            cpu_percent: f64::from(process.cpu_usage()),
            rss: process.memory(),
            virtual_size: process.virtual_memory(),
            threads: process.tasks().map(|tasks| tasks.len() as u32),
            uptime: std::time::Duration::from_secs(process.run_time()),
            cores: self.cores.max(1),
            load_average: load_average(),
            system_memory: (self.system.used_memory(), self.system.total_memory()),
        })
    }

    /// The PID being watched, if any.
    pub fn pid(&self) -> Option<u32> {
        self.current.map(|pid| pid.as_u32())
    }

    /// Locate the server process: the pinned PID when given, otherwise the
    /// JVM whose command line matches the pattern. Where several match, the
    /// largest resident set wins, which is reliably the game server rather
    /// than a build tool or a launcher sharing the directory.
    fn find(&self) -> Option<Pid> {
        if let Some(pinned) = self.pinned {
            // A pinned PID is taken as given, thread or not: the operator may
            // know something we do not.
            return self.system.process(pinned).map(|_| pinned);
        }

        let pattern = self.pattern.as_ref()?;

        self.system
            .processes()
            .iter()
            // On Linux each JVM thread shows up alongside the process that owns
            // it, sharing its name and memory. Picking one of those would give
            // a plausible-looking PID that the JDK tools cannot attach to.
            .filter(|(_, process)| process.thread_kind().is_none())
            .filter(|(_, process)| {
                let name = process.name().to_string_lossy();
                name.contains("java") || name.contains("jre") || name.contains("jdk")
            })
            .filter(|(_, process)| {
                let command_line = process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ");
                pattern.is_match(&command_line)
            })
            .max_by_key(|(_, process)| process.memory())
            .map(|(&pid, _)| pid)
    }
}

fn load_average() -> Option<[f64; 3]> {
    let average = System::load_average();
    let values = [average.one, average.five, average.fifteen];
    values.iter().any(|&value| value > 0.0).then_some(values)
}

fn num_cpus_fallback() -> usize {
    std::thread::available_parallelism().map_or(1, |count| count.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_pid_is_sampled_directly() {
        let mut watcher = ProcessWatcher::new(&ProcessConfig {
            pid: Some(std::process::id()),
            enabled: true,
            ..ProcessConfig::default()
        })
        .unwrap();

        let stats = watcher.sample().expect("our own process is running");
        assert_eq!(stats.pid, std::process::id());
        assert_eq!(watcher.pid(), Some(std::process::id()));
        assert!(stats.rss > 0);
        assert!(stats.cores >= 1);
        assert!(stats.system_memory.1 > 0);
        // CPU is a delta, so the first reading is zero rather than absent.
        assert!(stats.cpu_fraction() >= 0.0);
    }

    #[test]
    fn a_pattern_that_matches_nothing_yields_nothing() {
        let mut watcher = ProcessWatcher::new(&ProcessConfig {
            pid: None,
            match_pattern: "mctop-no-such-server-anywhere".into(),
            enabled: true,
        })
        .unwrap();

        assert!(watcher.sample().is_none());
        assert_eq!(watcher.pid(), None);
    }

    #[test]
    fn an_invalid_pattern_is_rejected_at_startup() {
        let result = ProcessWatcher::new(&ProcessConfig {
            pid: None,
            match_pattern: "(unclosed".into(),
            enabled: true,
        });
        let error = result.err().expect("an invalid regex should be refused");
        assert!(error.to_string().contains("process.match_pattern"));
    }
}
