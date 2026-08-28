//! Parsers for console output.
//!
//! Server forks word their reports differently and change the wording between
//! releases, so nothing here insists on an exact layout. Each parser strips
//! formatting, finds the labels it recognises, and harvests the numbers near
//! them. Anything it cannot make sense of is left as `None` rather than
//! guessed at, and the raw text stays available in the Log tab so a mismatch
//! is visible rather than silent.

use std::sync::LazyLock;

use regex::Regex;

use crate::metrics::{
    MsptReading, MsptWindow, Players, Region, RegionReport, ServerIdentity, TpsReading,
};

/// Remove Minecraft section-sign colour codes and ANSI escapes.
pub fn strip_formatting(input: &str) -> String {
    static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());

    let without_ansi = ANSI.replace_all(input, "");

    let mut out = String::with_capacity(without_ansi.len());
    let mut chars = without_ansi.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{a7}' {
            // A section sign escapes exactly one following character.
            chars.next();
        } else {
            out.push(ch);
        }
    }
    out
}

/// Every finite number in `text`, in order.
fn numbers(text: &str) -> Vec<f64> {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?\d+(?:\.\d+)?").unwrap());

    NUMBER
        .find_iter(text)
        .filter_map(|found| found.as_str().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .collect()
}

/// The window labels in a phrase such as `from last 5s, 1m, 5m:`.
fn window_labels(text: &str) -> Option<Vec<String>> {
    static FROM_LAST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)from\s+last\s+([^:]+):").unwrap());

    let captures = FROM_LAST.captures(text)?;
    let labels: Vec<String> = captures
        .get(1)?
        .as_str()
        .split(',')
        .map(|label| label.trim().to_owned())
        .filter(|label| !label.is_empty())
        .collect();

    (!labels.is_empty()).then_some(labels)
}

/// Pair values with labels, inventing positional labels when there are more
/// values than the header announced.
fn label_values(labels: Option<Vec<String>>, values: Vec<f64>) -> Vec<(String, f64)> {
    let labels = labels.unwrap_or_default();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let label = labels
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("#{}", index + 1));
            (label, value)
        })
        .collect()
}

/// Parse a `/tps` report.
///
/// Handles Paper's single line (`TPS from last 1m, 5m, 15m: 20.0, 20.0, 20.0`)
/// and Folia's longer report, where the headline rate sits among per-region
/// detail. Values above 20 are kept as reported; some forks overshoot slightly.
pub fn parse_tps(raw: &str) -> TpsReading {
    let text = strip_formatting(raw);

    for line in text.lines() {
        if !line.to_ascii_lowercase().contains("tps") {
            continue;
        }
        // Skip per-region lines; the headline is what this parser wants.
        if line.to_ascii_lowercase().contains("region") {
            continue;
        }

        let labels = window_labels(line);
        // Only look at the part after the colon so that window labels such as
        // `15m` are not mistaken for rates.
        let tail = line.split_once(':').map_or(line, |(_, tail)| tail);
        let values: Vec<f64> = numbers(tail)
            .into_iter()
            .filter(|value| (0.0..=100.0).contains(value))
            .collect();

        if !values.is_empty() {
            return TpsReading {
                windows: label_values(labels, values),
            };
        }
    }

    TpsReading::default()
}

/// Parse an `/mspt` report: tick durations as average/minimum/maximum triples,
/// one per window.
pub fn parse_mspt(raw: &str) -> MsptReading {
    static TRIPLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d+(?:\.\d+)?)\s*/\s*(\d+(?:\.\d+)?)\s*/\s*(\d+(?:\.\d+)?)").unwrap()
    });

    let text = strip_formatting(raw);
    let labels = window_labels(&text);

    let mut windows = Vec::new();
    for captures in TRIPLE.captures_iter(&text) {
        let value = |index: usize| {
            captures
                .get(index)
                .and_then(|found| found.as_str().parse::<f64>().ok())
        };
        if let (Some(average), Some(minimum), Some(maximum)) = (value(1), value(2), value(3)) {
            windows.push(MsptWindow {
                average,
                minimum,
                maximum,
            });
        }
    }

    if windows.is_empty() {
        // Some forks report a single average with no spread.
        if let Some(average) = text
            .lines()
            .find(|line| line.to_ascii_lowercase().contains("mspt"))
            .and_then(|line| numbers(line).into_iter().next())
        {
            windows.push(MsptWindow {
                average,
                minimum: average,
                maximum: average,
            });
        }
    }

    let labels = labels.unwrap_or_default();
    MsptReading {
        windows: windows
            .into_iter()
            .enumerate()
            .map(|(index, window)| {
                let label = labels
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", index + 1));
                (label, window)
            })
            .collect(),
    }
}

/// Parse the per-region detail out of a Folia report.
///
/// Regions are reported one block at a time, and the wording differs between
/// builds, so a block is anything introduced by the word `region` and every
/// indented or comma-separated field that follows it. Recognised fields are
/// TPS, MSPT, utilisation, and the player, entity, and chunk counts.
pub fn parse_regions(raw: &str) -> RegionReport {
    static HEADER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^\s*(?:-\s*)?region\b").unwrap());
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(?:total\s+)?regions?\b\s*[:=]\s*(\d+)").unwrap());
    static THREADS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:region\s+|total\s+)?threads?\b\s*[:=]\s*(\d+)").unwrap()
    });

    let text = strip_formatting(raw);

    let mut report = RegionReport::default();
    for line in text.lines() {
        if report.total.is_none()
            && let Some(captures) = TOTAL.captures(line)
            && let Some(total) = captures.get(1).and_then(|m| m.as_str().parse().ok())
        {
            report.total = Some(total);
        }
        if report.threads.is_none()
            && let Some(captures) = THREADS.captures(line)
            && let Some(threads) = captures.get(1).and_then(|m| m.as_str().parse().ok())
        {
            report.threads = Some(threads);
        }
    }

    // Group the lines into blocks, one per region.
    let mut blocks: Vec<String> = Vec::new();
    for line in text.lines() {
        if HEADER.is_match(line) {
            blocks.push(line.to_owned());
        } else if let Some(block) = blocks.last_mut()
            && (line.starts_with(char::is_whitespace) || line.trim_start().starts_with('-'))
            && !line.trim().is_empty()
        {
            block.push('\n');
            block.push_str(line);
        }
    }

    report.regions = blocks
        .iter()
        .filter_map(|block| parse_region_block(block))
        .collect();
    if report.total.is_none() && !report.regions.is_empty() {
        report.total = Some(report.regions.len() as u32);
    }

    report
}

fn parse_region_block(block: &str) -> Option<Region> {
    static COORDS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\(\s*(-?\d+)\s*,\s*(-?\d+)\s*\)").unwrap());
    static WORLD_KEYED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)\bworld\s*[:=]\s*"?([^",\s\]\)]+)"#).unwrap());
    static WORLD_BARE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)^\s*(?:-\s*)?region\b[\s#:]*\d*[\s:]*[\[("]?\s*([A-Za-z0-9_.-]+)"#)
            .unwrap()
    });

    let mut region = Region {
        world: WORLD_KEYED
            .captures(block)
            .or_else(|| WORLD_BARE.captures(block))
            .and_then(|captures| captures.get(1))
            .map(|found| found.as_str().to_owned())
            .filter(|world| !is_metric_word(world)),
        chunk: COORDS.captures(block).and_then(|captures| {
            let x = captures.get(1)?.as_str().parse().ok()?;
            let z = captures.get(2)?.as_str().parse().ok()?;
            Some((x, z))
        }),
        ..Region::default()
    };

    region.tps = field(block, r"(?i)\btps\b");
    region.mspt = field(block, r"(?i)\bmspt\b");
    region.utilisation = field(block, r"(?i)\butil(?:isation|ization)?\b").map(|value| {
        // Utilisation is reported either as a percentage or as a fraction.
        if block.contains('%') || value > 1.5 {
            value / 100.0
        } else {
            value
        }
    });
    region.players = field(block, r"(?i)\bplayers?\b").map(|value| value.max(0.0) as u32);
    region.entities = field(block, r"(?i)\bentit(?:y|ies)\b").map(|value| value.max(0.0) as u32);
    region.chunks = field(block, r"(?i)\bchunks?\b").map(|value| value.max(0.0) as u32);

    let has_data = region.tps.is_some()
        || region.mspt.is_some()
        || region.utilisation.is_some()
        || region.players.is_some()
        || region.entities.is_some()
        || region.chunks.is_some();

    (has_data || region.chunk.is_some()).then_some(region)
}

/// A word that names a metric is never a world name.
fn is_metric_word(word: &str) -> bool {
    const METRICS: [&str; 8] = [
        "tps",
        "mspt",
        "util",
        "utilisation",
        "utilization",
        "players",
        "entities",
        "chunks",
    ];
    METRICS
        .iter()
        .any(|metric| metric.eq_ignore_ascii_case(word))
}

/// The first number following `label` in `block`, wherever it appears.
fn field(block: &str, label: &str) -> Option<f64> {
    let pattern = format!(r"{label}\s*[:=]?\s*(-?\d+(?:\.\d+)?)");
    let regex = Regex::new(&pattern).ok()?;
    regex
        .captures(block)?
        .get(1)?
        .as_str()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Parse `/list`: `There are 3 of a max of 20 players online: alice, bob`.
pub fn parse_players(raw: &str) -> Players {
    static COUNTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)there\s+are\s+(\d+)(?:\s*(?:/|\s+of\s+a\s+max\s+of\s+)\s*(\d+))?").unwrap()
    });

    let text = strip_formatting(raw);
    let mut players = Players::default();

    if let Some(captures) = COUNTS.captures(&text) {
        players.online = captures
            .get(1)
            .and_then(|found| found.as_str().parse().ok())
            .unwrap_or(0);
        players.max = captures
            .get(2)
            .and_then(|found| found.as_str().parse().ok());
    }

    if let Some((_, names)) = text.split_once(':') {
        players.names = names
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
    }

    // A trailing count with no names still tells us how many are online.
    if players.online == 0 && !players.names.is_empty() {
        players.online = players.names.len() as u32;
    }

    players
}

/// Parse `/version`: `This server is running Folia version 1.21.4-… (MC: 1.21.4)`.
pub fn parse_version(raw: &str) -> ServerIdentity {
    static RUNNING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)running\s+([A-Za-z][A-Za-z0-9_-]*)\s+version").unwrap());
    static MC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\(\s*MC\s*:\s*([^)]+?)\s*\)").unwrap());
    static BARE_VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(1\.\d+(?:\.\d+)?)\b").unwrap());

    let text = strip_formatting(raw);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();

    ServerIdentity {
        flavour: RUNNING
            .captures(line)
            .and_then(|captures| captures.get(1))
            .map(|found| found.as_str().to_owned()),
        minecraft_version: MC
            .captures(line)
            .or_else(|| BARE_VERSION.captures(line))
            .and_then(|captures| captures.get(1))
            .map(|found| found.as_str().to_owned()),
        raw: (!line.is_empty()).then(|| line.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_section_codes_and_ansi() {
        assert_eq!(strip_formatting("\u{a7}6TPS: \u{a7}a20.0"), "TPS: 20.0");
        assert_eq!(strip_formatting("\x1b[32mgreen\x1b[0m"), "green");
    }

    #[test]
    fn reads_paper_tps() {
        let raw = "\u{a7}6TPS from last 1m, 5m, 15m: \u{a7}a*20.0\u{a7}6, \u{a7}a19.87\u{a7}6, \u{a7}a19.94";
        let tps = parse_tps(raw);
        assert_eq!(
            tps.windows,
            vec![
                ("1m".to_string(), 20.0),
                ("5m".to_string(), 19.87),
                ("15m".to_string(), 19.94),
            ]
        );
        assert_eq!(tps.current(), Some(20.0));
        assert_eq!(tps.window("15m"), Some(19.94));
    }

    #[test]
    fn reads_folia_style_tps_with_extra_windows() {
        let raw = "TPS from last 5s, 15s, 1m, 5m, 15m: 20.0, 19.9, 19.8, 19.7, 19.6";
        let tps = parse_tps(raw);
        assert_eq!(tps.windows.len(), 5);
        assert_eq!(tps.windows[0], ("5s".to_string(), 20.0));
        assert_eq!(tps.current(), Some(20.0));
    }

    #[test]
    fn tps_ignores_per_region_lines() {
        let raw = "\
Server TPS from last 1m, 5m, 15m: 19.5, 19.6, 19.7
Region world (0, 0): TPS 12.0";
        assert_eq!(parse_tps(raw).current(), Some(19.5));
    }

    #[test]
    fn tps_labels_positionally_without_a_header() {
        let tps = parse_tps("TPS: 20.0 20.0");
        assert_eq!(
            tps.windows,
            vec![("#1".to_string(), 20.0), ("#2".to_string(), 20.0)]
        );
    }

    #[test]
    fn unparseable_tps_is_empty_rather_than_guessed() {
        assert!(parse_tps("Unknown command").windows.is_empty());
        assert_eq!(parse_tps("").current(), None);
    }

    #[test]
    fn reads_paper_mspt() {
        let raw = "\u{a7}6Server tick times \u{a7}e(\u{a7}aavg\u{a7}e/\u{a7}amin\u{a7}e/\u{a7}amax\u{a7}e)\u{a7}6 from last 5s, 10s, 1m:\n\
\u{a7}6\u{25f4} \u{a7}a1.05\u{a7}e/\u{a7}a0.40\u{a7}e/\u{a7}a12.30\u{a7}6, \u{a7}a1.10/0.42/18.00\u{a7}6, \u{a7}a1.20/0.39/44.10";
        let mspt = parse_mspt(raw);
        assert_eq!(mspt.windows.len(), 3);
        assert_eq!(mspt.windows[0].0, "5s");
        assert_eq!(
            mspt.windows[0].1,
            MsptWindow {
                average: 1.05,
                minimum: 0.40,
                maximum: 12.30
            }
        );
        assert_eq!(mspt.current().unwrap().maximum, 12.30);
        assert_eq!(mspt.windows[2].1.maximum, 44.10);
    }

    #[test]
    fn reads_a_bare_mspt_average() {
        let mspt = parse_mspt("Current MSPT: 4.2");
        assert_eq!(
            mspt.current(),
            Some(MsptWindow {
                average: 4.2,
                minimum: 4.2,
                maximum: 4.2
            })
        );
    }

    #[test]
    fn unparseable_mspt_is_empty() {
        assert!(parse_mspt("Unknown command").windows.is_empty());
    }

    #[test]
    fn reads_regions_written_as_key_value_pairs() {
        let raw = "\
Total Regions: 3
Region Threads: 4
Region 1: world=world, chunk=(12, -4), tps=19.90, mspt=3.20, util=6.4%, players=3, entities=120, chunks=441
Region 2: world=world_nether, chunk=(-30, 8), tps=20.00, mspt=1.10, util=2.2%, players=0, entities=14, chunks=90";

        let report = parse_regions(raw);
        assert_eq!(report.total, Some(3));
        assert_eq!(report.threads, Some(4));
        assert_eq!(report.regions.len(), 2);

        let first = &report.regions[0];
        assert_eq!(first.world.as_deref(), Some("world"));
        assert_eq!(first.chunk, Some((12, -4)));
        assert_eq!(first.tps, Some(19.90));
        assert_eq!(first.mspt, Some(3.20));
        assert_eq!(first.players, Some(3));
        assert_eq!(first.entities, Some(120));
        assert_eq!(first.chunks, Some(441));
        assert!((first.utilisation.unwrap() - 0.064).abs() < 1e-9);
        assert_eq!(report.worst().unwrap().world.as_deref(), Some("world"));
    }

    #[test]
    fn reads_regions_written_as_prose() {
        let raw = "\
Region world (12, -4): TPS 19.98, MSPT 3.21, Util 6.4%
Region world_the_end (-30, 8): TPS 20.00, MSPT 1.05, Util 2.1%";

        let report = parse_regions(raw);
        assert_eq!(report.regions.len(), 2);
        assert_eq!(report.regions[1].world.as_deref(), Some("world_the_end"));
        assert_eq!(report.regions[1].chunk, Some((-30, 8)));
        assert_eq!(report.regions[1].tps, Some(20.00));
        // Absent totals fall back to the number of regions actually detailed.
        assert_eq!(report.total, Some(2));
    }

    #[test]
    fn reads_regions_written_as_indented_blocks() {
        let raw = "\
Region [world, (0, 0)]:
  TPS: 20.0
  MSPT: 2.5
  Utilisation: 5.0%
Region [world_nether, (4, 4)]:
  TPS: 18.2
  MSPT: 27.4
  Utilisation: 54.8%";

        let report = parse_regions(raw);
        assert_eq!(report.regions.len(), 2);
        assert_eq!(report.regions[0].world.as_deref(), Some("world"));
        assert_eq!(report.regions[1].mspt, Some(27.4));
        assert!((report.regions[1].utilisation.unwrap() - 0.548).abs() < 1e-9);
        assert_eq!(
            report.worst().unwrap().world.as_deref(),
            Some("world_nether")
        );
    }

    #[test]
    fn utilisation_given_as_a_fraction_is_left_alone() {
        let report = parse_regions("Region world (0, 0): util=0.42");
        assert_eq!(report.regions[0].utilisation, Some(0.42));
    }

    #[test]
    fn a_report_without_regions_yields_none() {
        let report = parse_regions("TPS from last 1m, 5m, 15m: 20.0, 20.0, 20.0");
        assert!(report.regions.is_empty());
        assert_eq!(report.total, None);
    }

    #[test]
    fn reads_the_player_list() {
        let raw = "There are 3 of a max of 20 players online: alice, bob, carol";
        let players = parse_players(raw);
        assert_eq!(players.online, 3);
        assert_eq!(players.max, Some(20));
        assert_eq!(players.names, vec!["alice", "bob", "carol"]);
    }

    #[test]
    fn reads_a_slash_separated_player_list() {
        let players = parse_players("There are 1/60 players online: dave");
        assert_eq!(players.online, 1);
        assert_eq!(players.max, Some(60));
        assert_eq!(players.names, vec!["dave"]);
    }

    #[test]
    fn reads_an_empty_player_list() {
        let players = parse_players("There are 0 of a max of 20 players online:");
        assert_eq!(players.online, 0);
        assert_eq!(players.max, Some(20));
        assert!(players.names.is_empty());
    }

    #[test]
    fn reads_the_folia_version() {
        let raw = "This server is running Folia version 1.21.4-DEV-a1b2c3d (MC: 1.21.4) (Implementing API version 1.21.4-R0.1-SNAPSHOT)";
        let identity = parse_version(raw);
        assert_eq!(identity.flavour.as_deref(), Some("Folia"));
        assert_eq!(identity.minecraft_version.as_deref(), Some("1.21.4"));
        assert!(identity.is_folia());
        assert_eq!(identity.summary(), "Folia 1.21.4");
    }

    #[test]
    fn reads_the_paper_version() {
        let identity =
            parse_version("This server is running Paper version git-Paper-196 (MC: 1.20.4)");
        assert_eq!(identity.flavour.as_deref(), Some("Paper"));
        assert!(!identity.is_folia());
    }

    #[test]
    fn an_unrecognised_version_still_keeps_the_raw_line() {
        let identity = parse_version("some custom build");
        assert_eq!(identity.flavour, None);
        assert_eq!(identity.raw.as_deref(), Some("some custom build"));
        assert_eq!(identity.summary(), "unknown");
    }
}
