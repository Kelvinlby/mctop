//! Configuration: file on disk, environment, and command-line overrides.
//!
//! Resolution order, lowest priority first: built-in defaults, the config file,
//! `MCTOP_*` environment variables, then command-line flags.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// Environment variable holding the RCON password.
pub const PASSWORD_ENV: &str = "MCTOP_RCON_PASSWORD";

/// The whole of `mctop`'s configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub rcon: RconConfig,
    pub refresh: RefreshConfig,
    pub commands: CommandConfig,
    pub process: ProcessConfig,
    pub jvm: JvmConfig,
    pub ui: UiConfig,
    /// World directories to measure on disk. Empty means "discover them".
    #[serde(rename = "world")]
    pub worlds: Vec<WorldConfig>,
}

/// Cosmetic details about the server being watched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Label shown in the header. Defaults to the RCON host.
    pub name: Option<String>,
    /// Directory the server runs from. Used to discover worlds.
    pub directory: Option<PathBuf>,
}

/// How to reach the server's RCON port.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RconConfig {
    /// `host` or `host:port`; the port defaults to 25575.
    pub address: String,
    /// Password in plain text. Prefer `password_file` or [`PASSWORD_ENV`].
    pub password: Option<String>,
    /// File whose first line is the password.
    pub password_file: Option<PathBuf>,
    /// How long to wait for a connection before giving up.
    pub connect_timeout_ms: u64,
    /// How long to wait for a single command's response.
    pub command_timeout_ms: u64,
    /// How long to wait before retrying a dropped connection.
    pub reconnect_delay_ms: u64,
}

impl Default for RconConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:25575".into(),
            password: None,
            password_file: None,
            connect_timeout_ms: 5_000,
            command_timeout_ms: 5_000,
            reconnect_delay_ms: 5_000,
        }
    }
}

impl RconConfig {
    /// Resolve the password from, in order: [`PASSWORD_ENV`], `password_file`,
    /// then the inline `password`.
    pub fn resolve_password(&self) -> anyhow::Result<String> {
        if let Ok(password) = std::env::var(PASSWORD_ENV)
            && !password.is_empty()
        {
            return Ok(password);
        }

        if let Some(path) = &self.password_file {
            let text = fs::read_to_string(path)
                .with_context(|| format!("reading password file {}", path.display()))?;
            return Ok(text.lines().next().unwrap_or_default().trim().to_owned());
        }

        self.password.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "no RCON password: set {PASSWORD_ENV}, `rcon.password_file`, or `rcon.password`"
            )
        })
    }
}

/// Polling intervals. Each collector runs on its own clock so that a cheap
/// metric is not held back by an expensive one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RefreshConfig {
    /// TPS and MSPT polling.
    pub tick_ms: u64,
    /// Per-region detail polling.
    pub region_ms: u64,
    /// Player list and version polling.
    pub roster_ms: u64,
    /// CPU, memory, and JVM heap sampling.
    pub process_ms: u64,
    /// World size scanning, which walks the disk and is deliberately slow.
    pub disk_secs: u64,
    /// Screen redraws.
    pub draw_ms: u64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            tick_ms: 1_000,
            region_ms: 2_000,
            roster_ms: 5_000,
            process_ms: 2_000,
            disk_secs: 120,
            draw_ms: 250,
        }
    }
}

impl RefreshConfig {
    pub fn tick(&self) -> Duration {
        floor_ms(self.tick_ms, 200)
    }

    pub fn region(&self) -> Duration {
        floor_ms(self.region_ms, 200)
    }

    pub fn roster(&self) -> Duration {
        floor_ms(self.roster_ms, 500)
    }

    pub fn process(&self) -> Duration {
        floor_ms(self.process_ms, 200)
    }

    pub fn disk(&self) -> Duration {
        Duration::from_secs(self.disk_secs.max(5))
    }

    pub fn draw(&self) -> Duration {
        floor_ms(self.draw_ms, 50)
    }
}

fn floor_ms(value: u64, min: u64) -> Duration {
    Duration::from_millis(value.max(min))
}

/// The console commands each metric is read from. Overridable because server
/// forks word their output differently, and plugins such as Spark can stand in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandConfig {
    pub tps: String,
    pub mspt: String,
    /// Source of per-region detail. On Folia this is the same `tps` report.
    pub regions: String,
    pub players: String,
    pub version: String,
}

impl Default for CommandConfig {
    fn default() -> Self {
        Self {
            tps: "tps".into(),
            mspt: "mspt".into(),
            regions: "tps".into(),
            players: "list".into(),
            version: "version".into(),
        }
    }
}

/// How to find the server's JVM process for CPU and memory readings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessConfig {
    /// Watch this PID exactly. `None` means search.
    pub pid: Option<u32>,
    /// Regex matched against the full command line when searching.
    pub match_pattern: String,
    /// Sample the local process at all. Turn off when watching a remote server.
    pub enabled: bool,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            pid: None,
            match_pattern: r"(?i)folia|paper|purpur|spigot|craftbukkit|server\.jar|minecraft"
                .into(),
            enabled: true,
        }
    }
}

/// A JDK tool to invoke: either a bare program name or a program with leading
/// arguments.
///
/// The list form exists so the tools can be run as a different user than mctop
/// itself. `jstat` reads a file the JVM creates mode 0600, and `jcmd` attaches
/// over a socket that checks the caller's user and group, so neither can be
/// reached from an account that merely shares the server's group. Rather than
/// force the whole dashboard to run as the server's user, point the tool at a
/// wrapper:
///
/// ```toml
/// jstat = ["sudo", "-n", "-u", "minecraft", "jstat"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool(Vec<String>);

impl Tool {
    /// Build from words, e.g. `["sudo", "-n", "-u", "minecraft", "jstat"]`.
    pub fn from_words<I, S>(words: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let words: Vec<String> = words.into_iter().map(Into::into).collect();
        anyhow::ensure!(
            !words.is_empty() && words.iter().any(|word| !word.trim().is_empty()),
            "a JDK tool needs at least a program name"
        );
        Ok(Self(words))
    }

    /// The program to run and the arguments that come before the tool's own.
    /// `None` when the tool has been configured empty.
    pub fn parts(&self) -> Option<(&str, &[String])> {
        let (program, leading) = self.0.split_first()?;
        Some((program.as_str(), leading))
    }

    /// How to name this tool in a message.
    pub fn label(&self) -> String {
        self.0.join(" ")
    }
}

impl From<&str> for Tool {
    fn from(value: &str) -> Self {
        Self(vec![value.to_owned()])
    }
}

impl Serialize for Tool {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Keep the common case readable in `mctop config show`.
        match self.0.as_slice() {
            [only] => serializer.serialize_str(only),
            words => words.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            One(String),
            Many(Vec<String>),
        }

        let words = match Either::deserialize(deserializer)? {
            Either::One(one) => vec![one],
            Either::Many(many) => many,
        };

        Self::from_words(words).map_err(serde::de::Error::custom)
    }
}

/// Heap readings, which come from the JDK's own tools rather than RCON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JvmConfig {
    pub enabled: bool,
    /// How to run `jcmd`. A bare name is looked up on `PATH`; a list may put a
    /// wrapper such as `sudo -n -u minecraft` in front of it.
    pub jcmd: Tool,
    /// How to run `jstat`, in the same two forms.
    pub jstat: Tool,
    /// Window over which the heap low-water mark is treated as the
    /// after-collection occupancy when no collection has been observed.
    pub heap_after_gc_window_secs: u64,
}

impl Default for JvmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            jcmd: "jcmd".into(),
            jstat: "jstat".into(),
            heap_after_gc_window_secs: 300,
        }
    }
}

/// Presentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Samples retained for the history charts.
    pub history: usize,
    /// Rows of scrollback in the Console tab.
    pub log_lines: usize,
    /// TPS at or above this is healthy.
    pub tps_good: f64,
    /// TPS at or above this is a warning; below it is critical.
    pub tps_warn: f64,
    /// MSPT at or below this is healthy.
    pub mspt_good: f64,
    /// MSPT at or below this is a warning; above it is critical.
    pub mspt_warn: f64,
    /// Draw the borders with rounded corners.
    pub rounded_borders: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            history: 480,
            log_lines: 500,
            tps_good: 19.5,
            tps_warn: 18.0,
            mspt_good: 25.0,
            mspt_warn: 40.0,
            rounded_borders: true,
        }
    }
}

/// A world directory to measure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldConfig {
    /// Display name. Defaults to the directory's file name.
    #[serde(default)]
    pub name: Option<String>,
    pub path: PathBuf,
}

impl Config {
    /// Load from `path` when given, else from the default location when it
    /// exists, else fall back to defaults.
    pub fn load(path: Option<&Path>) -> anyhow::Result<(Self, Option<PathBuf>)> {
        let path = match path {
            Some(path) => Some(path.to_path_buf()),
            None => default_path().filter(|path| path.exists()),
        };

        let Some(path) = path else {
            return Ok((Self::default(), None));
        };

        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;

        Ok((config, Some(path)))
    }

    /// Worlds to measure, discovering them under `server.directory` when the
    /// config does not list any.
    pub fn resolved_worlds(&self) -> Vec<WorldConfig> {
        if !self.worlds.is_empty() {
            return self.worlds.clone();
        }

        let Some(directory) = &self.server.directory else {
            return Vec::new();
        };

        discover_worlds(directory)
    }

    /// The label for the header.
    pub fn display_name(&self) -> String {
        self.server
            .name
            .clone()
            .unwrap_or_else(|| self.rcon.address.clone())
    }
}

/// A directory is a world when it holds a `level.dat`. Folia keeps the nether
/// and the end beside the overworld as `world_nether` and `world_the_end`.
pub fn discover_worlds(directory: &Path) -> Vec<WorldConfig> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut worlds: Vec<WorldConfig> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("level.dat").is_file())
        .map(|path| WorldConfig {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            path,
        })
        .collect();

    worlds.sort_by(|a, b| a.path.cmp(&b.path));
    worlds
}

/// `~/.config/mctop/config.toml` and its equivalents.
pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "mctop")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

/// A commented starter config, written by `mctop config init`.
pub const TEMPLATE: &str = include_str!("../assets/config.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let text = toml::to_string(&Config::default()).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.rcon.address, "127.0.0.1:25575");
        assert_eq!(parsed.commands.tps, "tps");
    }

    #[test]
    fn template_parses_and_matches_the_defaults() {
        let parsed: Config = toml::from_str(TEMPLATE).unwrap();
        let defaults = Config::default();
        assert_eq!(parsed.refresh.tick_ms, defaults.refresh.tick_ms);
        assert_eq!(parsed.commands.regions, defaults.commands.regions);
        assert_eq!(parsed.ui.tps_warn, defaults.ui.tps_warn);
    }

    #[test]
    fn a_jdk_tool_may_be_a_name_or_a_command() {
        let config: Config = toml::from_str(
            "[jvm]\njstat = \"jstat\"\njcmd = [\"sudo\", \"-n\", \"-u\", \"minecraft\", \"jcmd\"]\n",
        )
        .unwrap();

        assert_eq!(config.jvm.jstat.parts(), Some(("jstat", &[][..])));

        let (program, leading) = config.jvm.jcmd.parts().unwrap();
        assert_eq!(program, "sudo");
        assert_eq!(leading, ["-n", "-u", "minecraft", "jcmd"]);
        assert_eq!(config.jvm.jcmd.label(), "sudo -n -u minecraft jcmd");
    }

    #[test]
    fn a_single_word_tool_round_trips_as_a_string() {
        let text = toml::to_string(&Config::default()).unwrap();
        assert!(text.contains("jstat = \"jstat\""), "{text}");

        let with_wrapper = Config {
            jvm: JvmConfig {
                jstat: Tool::from_words(["sudo", "jstat"]).unwrap(),
                ..JvmConfig::default()
            },
            ..Config::default()
        };
        let text = toml::to_string(&with_wrapper).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.jvm.jstat.label(), "sudo jstat");
    }

    #[test]
    fn an_empty_jdk_tool_is_rejected() {
        assert!(toml::from_str::<Config>("[jvm]\njstat = []\n").is_err());
        assert!(toml::from_str::<Config>("[jvm]\njstat = \"\"\n").is_err());
        assert!(Tool::from_words(Vec::<String>::new()).is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = toml::from_str::<Config>("[rcon]\naddres = \"x\"\n").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn intervals_have_a_floor() {
        let refresh = RefreshConfig {
            tick_ms: 0,
            ..RefreshConfig::default()
        };
        assert_eq!(refresh.tick(), Duration::from_millis(200));
    }

    #[test]
    fn password_file_wins_over_inline() {
        let dir = std::env::temp_dir().join("mctop-test-password");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("password");
        fs::write(&path, "from-file\n").unwrap();

        let config = RconConfig {
            password: Some("inline".into()),
            password_file: Some(path),
            ..RconConfig::default()
        };

        // Only meaningful when the environment does not override both.
        if std::env::var(PASSWORD_ENV).is_err() {
            assert_eq!(config.resolve_password().unwrap(), "from-file");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
