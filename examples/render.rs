//! Renders each tab into a text buffer so the layout can be eyeballed without
//! a real server. Run with: cargo run --example render -- [width] [height]

use std::sync::Arc;
use std::time::Duration;

use mctop::app::{App, Tab};
use mctop::config::Config;
use mctop::metrics::*;
use mctop::source::{Kind, Update};
use mctop::ui::{self, theme::Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn main() {
    let mut args = std::env::args().skip(1);
    let width: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(120);
    let height: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(38);

    let mut config = Config::default();
    config.server.name = Some("survival".into());
    let config = Arc::new(config);
    let theme = Theme::new(&config.ui);
    let mut app = App::new(Arc::clone(&config));

    seed(&mut app);
    respace_history(&mut app);

    for tab in Tab::ALL {
        app.tab = tab;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &theme))
            .unwrap();
        println!("\n===== {} ({width}x{height}) =====", tab.title());
        print(terminal.backend());
    }

    // Draw the region table twice, moving the selection down the list, so the
    // scroll offset and the mid-list highlight can be eyeballed.
    app.tab = Tab::Regions;
    for selected in [0usize, 17] {
        app.region_selected = selected;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &theme))
            .unwrap();
        println!(
            "\n===== Regions, row {selected} selected (offset {}) =====",
            app.region_offset
        );
        print(terminal.backend());
    }

    // The case an operator running Aikar's flags sees: heap read by jcmd, but
    // no collector counters, because the perf file is switched off.
    app.tab = Tab::System;
    app.heap.counters_available = false;
    app.heap.perf_disabled = true;
    app.heap.young_collections = None;
    app.heap.full_collections = None;
    app.heap.gc_seconds = None;
    app.heap.gc_load = None;
    {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &theme))
            .unwrap();
        println!("\n===== System with -XX:+PerfDisableSharedMem =====");
        print(terminal.backend());
    }

    // The case where the process is found but the JDK tools are refused, which
    // is what an operator sees when mctop runs as a different user.
    app.heap = Default::default();
    app.heap_error =
        Some("jstat: sun.jvmstat.monitor.MonitorException: Could not attach to 4242".into());
    for tab in [Tab::Overview, Tab::System] {
        app.tab = tab;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &theme))
            .unwrap();
        println!("\n===== {} with the heap refused =====", tab.title());
        print(terminal.backend());
    }

    app.tab = Tab::Overview;
    app.show_help = true;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, &mut app, &theme))
        .unwrap();
    println!("\n===== Help overlay =====");
    print(terminal.backend());
}

/// `App::apply` stamps every sample with the moment it arrived, so seeding 300
/// of them in a tight loop leaves the charts with no time axis to draw against.
/// Lay the same values back out at one-second spacing.
fn respace_history(app: &mut App) {
    let now = std::time::Instant::now();
    for (history, capacity) in [
        (&mut app.tps_history, 300usize),
        (&mut app.mspt_history, 300),
        (&mut app.cpu_history, 300),
        (&mut app.heap_history, 300),
    ] {
        let values = history.tail(capacity);
        let mut respaced = History::new(capacity + 1);
        let count = values.len();
        for (index, value) in values.into_iter().enumerate() {
            let age = Duration::from_secs((count - index) as u64);
            respaced.push_at(now - age, value);
        }
        *history = respaced;
    }
}

fn print(backend: &TestBackend) {
    let buffer = backend.buffer();
    for y in 0..buffer.area.height {
        let mut row = String::new();
        for x in 0..buffer.area.width {
            row.push_str(buffer[(x, y)].symbol());
        }
        println!("{}", row.trim_end());
    }
}

fn seed(app: &mut App) {
    app.apply(Update::Link(Link::Up));
    app.apply(Update::Identity(ServerIdentity {
        flavour: Some("Folia".into()),
        minecraft_version: Some("1.21.4".into()),
        raw: None,
    }));

    for i in 0..300 {
        let wobble = ((i as f64) / 9.0).sin();
        app.apply(Update::Tps(TpsReading {
            windows: vec![
                ("5s".into(), 19.4 + wobble * 0.6),
                ("1m".into(), 19.7),
                ("5m".into(), 19.9),
                ("15m".into(), 20.0),
            ],
        }));
        app.apply(Update::Mspt(MsptReading {
            windows: vec![
                (
                    "5s".into(),
                    MsptWindow {
                        average: 18.0 + wobble * 12.0,
                        minimum: 4.2,
                        maximum: 46.8,
                    },
                ),
                (
                    "1m".into(),
                    MsptWindow {
                        average: 16.4,
                        minimum: 3.9,
                        maximum: 61.2,
                    },
                ),
            ],
        }));
        app.apply(Update::Process(Some(ProcessStats {
            pid: 4242,
            cpu_percent: 380.0 + wobble * 60.0,
            rss: 9_800_000_000,
            virtual_size: 21_000_000_000,
            threads: Some(148),
            uptime: Duration::from_secs(3 * 86_400 + 4 * 3600 + 900),
            cores: 8,
            load_average: Some([4.12, 3.88, 3.51]),
            system_memory: (12_800_000_000, 33_500_000_000),
        })));
        app.apply(Update::Heap {
            error: None,
            stats: Some(HeapStats {
                used: Some(6_400_000_000 + (wobble * 900_000_000.0) as u64),
                committed: Some(10_700_000_000),
                max: Some(12_884_901_888),
                after_gc: Some(3_300_000_000),
                after_gc_measured: true,
                young_collections: Some(18_204),
                full_collections: Some(3),
                gc_seconds: Some(412.6),
                gc_load: Some(0.021),
                non_heap: Some(412_000_000),
                counters_available: true,
                perf_disabled: false,
            }),
        });
    }

    app.apply(Update::Players(Players {
        online: 47,
        max: Some(120),
        names: vec!["alice".into(), "bob".into()],
    }));

    let worlds = ["world", "world_nether", "world_the_end"];
    let regions: Vec<Region> = (0..24)
        .map(|i| Region {
            world: Some(worlds[i % 3].into()),
            chunk: Some(((i as i64 - 12) * 37, (i as i64 * 19) - 90)),
            block: None,
            tps: Some(20.0 - (i as f64) * 0.31),
            mspt: Some(2.0 + (i as f64) * 2.4),
            utilisation: Some(0.03 + (i as f64) * 0.042),
            players: Some((24 - i as u32) / 3),
            entities: Some(120 + (i as u32) * 137),
            chunks: Some(441 + (i as u32) * 23),
        })
        .collect();
    app.apply(Update::Regions(RegionReport {
        regions,
        total: Some(37),
        threads: Some(6),
    }));

    app.apply(Update::Disk(DiskUsage {
        worlds: vec![
            WorldUsage {
                name: "world".into(),
                path: "/srv/mc/world".into(),
                bytes: 38_400_000_000,
                files: 284_302,
                region_bytes: 31_000_000_000,
                entity_bytes: 4_100_000_000,
                poi_bytes: 900_000_000,
                partial: false,
            },
            WorldUsage {
                name: "world_nether".into(),
                path: "/srv/mc/world_nether".into(),
                bytes: 7_200_000_000,
                files: 61_204,
                region_bytes: 6_400_000_000,
                entity_bytes: 500_000_000,
                poi_bytes: 120_000_000,
                partial: false,
            },
            WorldUsage {
                name: "world_the_end".into(),
                path: "/srv/mc/world_the_end".into(),
                bytes: 1_900_000_000,
                files: 14_880,
                region_bytes: 1_700_000_000,
                entity_bytes: 90_000_000,
                poi_bytes: 30_000_000,
                partial: true,
            },
        ],
        free: Some((210_000_000_000, 940_000_000_000)),
        scanned_at: Some(std::time::SystemTime::now() - Duration::from_secs(74)),
        scanning: false,
    }));

    app.note(Kind::Info, "config: /home/op/.config/mctop/config.toml");
    app.note(Kind::Info, "connected to 127.0.0.1:25575");
    app.note(Kind::Warn, "heap unavailable: jstat: not found on PATH");
    app.note(Kind::Sent, "list");
    app.note(
        Kind::Received,
        "There are 3 of a max of 60 players online: alice, bob, carol",
    );
    app.note(Kind::Sent, "save-all");
    app.note(Kind::Received, "Saving the game (this may take a moment!)");
    app.note(Kind::Received, "Saved the game");
    app.note(Kind::Sent, "whatever");
    app.note(Kind::Error, "`whatever` failed: Unknown command");
    app.input.set("say the server is going down in 5 minutes");
    app.apply(Update::Raw {
        command: "tps".into(),
        response: "\u{a7}6TPS from last 5s, 1m, 5m, 15m: \u{a7}a19.87, 19.70, 19.90, 20.00".into(),
    });
}
