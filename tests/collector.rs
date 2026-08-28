//! End-to-end tests of the collector against a stand-in server that speaks
//! RCON. These exercise the path a reading actually takes: socket, parser,
//! channel, application state.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mctop::app::App;
use mctop::config::{Config, RefreshConfig};
use mctop::metrics::Link;
use mctop::source::{self, Control, Kind, Update};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[path = "../examples/mock_server.rs"]
mod mock;

/// Start a stand-in server on a port the operating system picks, and return a
/// config already pointed at it.
async fn start(flavour: mock::Flavour) -> Config {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(mock::serve(listener, "secret".into(), flavour));

    let mut config = Config::default();
    config.rcon.address = address;
    config.rcon.password = Some("secret".into());
    config.refresh = RefreshConfig {
        tick_ms: 200,
        region_ms: 200,
        roster_ms: 200,
        process_ms: 200,
        disk_secs: 3_600,
        draw_ms: 200,
    };
    // The local process is a separate concern, tested on its own.
    config.process.enabled = false;
    config.jvm.enabled = false;
    config
}

/// Drive the collectors until `ready` is satisfied, or give up.
async fn collect(config: Config, ready: impl Fn(&App) -> bool) -> App {
    let config = Arc::new(config);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (_controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
        if ready(&app) {
            break;
        }
    }

    app
}

#[tokio::test]
async fn reads_a_folia_server_end_to_end() {
    let config = start(mock::Flavour::Folia).await;
    let app = collect(config, |app| {
        app.tps.current().is_some()
            && app.mspt.current().is_some()
            && !app.regions.regions.is_empty()
            && app.players.online > 0
            && app.identity.flavour.is_some()
    })
    .await;

    assert_eq!(app.link, Link::Up);
    assert!(app.identity.is_folia(), "identity: {:?}", app.identity);
    assert_eq!(app.identity.minecraft_version.as_deref(), Some("1.21.4"));

    // Five windows, shortest first.
    assert_eq!(app.tps.current(), Some(19.87));
    assert_eq!(app.tps.window("15m"), Some(20.00));

    let mspt = app.mspt.current().unwrap();
    assert_eq!(mspt.average, 4.21);
    assert_eq!(mspt.maximum, 18.70);

    assert_eq!(app.players.online, 3);
    assert_eq!(app.players.max, Some(60));
    assert_eq!(app.players.names.len(), 3);

    assert_eq!(app.regions.total, Some(4));
    assert_eq!(app.regions.threads, Some(6));
    assert_eq!(app.regions.regions.len(), 4);
    assert!(app.has_regions());

    // The overloaded region is the one the dashboard should surface.
    let worst = app.regions.worst().unwrap();
    assert_eq!(worst.world.as_deref(), Some("world"));
    assert_eq!(worst.chunk, Some((96, -8)));
    assert_eq!(worst.mspt, Some(48.60));
    assert_eq!(worst.entities, Some(4102));
    assert!((worst.pressure() - 0.972).abs() < 1e-9);

    // The histories the charts draw from are being filled.
    assert!(!app.tps_history.is_empty());
    assert!(!app.mspt_history.is_empty());

    // The raw responses are kept for the Log tab.
    assert!(app.raw.iter().any(|(command, _)| command == "tps"));
}

#[tokio::test]
async fn a_paper_server_reports_no_regions_and_says_so_once() {
    let config = start(mock::Flavour::Paper).await;
    let app = collect(config, |app| {
        app.tps.current().is_some() && app.identity.flavour.is_some()
    })
    .await;

    assert_eq!(app.tps.current(), Some(20.0));
    assert_eq!(app.identity.flavour.as_deref(), Some("Paper"));
    assert!(!app.identity.is_folia());
    assert!(app.regions.regions.is_empty());
    assert!(!app.has_regions(), "the Regions tab should stay empty");
}

#[tokio::test]
async fn unreadable_output_is_reported_rather_than_invented() {
    let config = start(mock::Flavour::Unrecognisable).await;
    let app = collect(config, |app| {
        app.log
            .iter()
            .any(|entry| entry.message.contains("could not read"))
    })
    .await;

    // Nothing was guessed at.
    assert_eq!(app.tps.current(), None);
    assert_eq!(app.mspt.current(), None);
    assert!(app.regions.regions.is_empty());

    // But the operator is told, and pointed at the setting that fixes it.
    let complaint = app
        .log
        .iter()
        .find(|entry| entry.message.contains("could not read"))
        .expect("should log what it could not read");
    assert!(complaint.message.contains("[commands]"), "{complaint:?}");

    // And the raw response is available to look at.
    assert!(
        app.raw
            .iter()
            .any(|(_, response)| response.contains("Unknown"))
    );
}

#[tokio::test]
async fn a_bad_password_is_reported_and_not_retried_in_a_loop() {
    let mut config = start(mock::Flavour::Folia).await;
    config.rcon.password = Some("wrong".into());
    config.rcon.reconnect_delay_ms = 60_000;

    let app = collect(config, |app| matches!(app.link, Link::Down(_))).await;

    match &app.link {
        Link::Down(reason) => assert!(reason.contains("authentication"), "{reason}"),
        other => panic!("expected a downed link, got {other:?}"),
    }
    assert!(app.tps.current().is_none());
}

#[tokio::test]
async fn a_server_that_is_not_there_does_not_spin() {
    let mut config = start(mock::Flavour::Folia).await;
    // Port 1 has nothing listening.
    config.rcon.address = "127.0.0.1:1".into();
    config.rcon.reconnect_delay_ms = 30_000;

    let app = collect(config, |app| matches!(app.link, Link::Down(_))).await;
    assert!(matches!(app.link, Link::Down(_)));

    // One complaint, not one per poll: the log collapses repeats.
    let failures = app
        .log
        .iter()
        .filter(|entry| entry.message.contains("127.0.0.1:1"))
        .count();
    assert!(failures <= 2, "logged {failures} times");
}

#[tokio::test]
async fn pausing_stops_collection_and_resuming_restarts_it() {
    let config = Arc::new(start(mock::Flavour::Folia).await);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    // Wait for the first reading.
    while app.tps.current().is_none() {
        match tokio::time::timeout(Duration::from_secs(10), updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => panic!("no reading arrived"),
        }
    }

    controls_tx.send(Control::SetPaused(true)).await.unwrap();
    // Drain whatever was already in flight, plus the pause acknowledgement.
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while updates_rx.recv().await.is_some() {}
    })
    .await;

    // Nothing further should arrive while paused.
    let quiet = tokio::time::timeout(Duration::from_millis(1_500), updates_rx.recv()).await;
    assert!(
        quiet.is_err(),
        "collection continued while paused: {quiet:?}"
    );

    controls_tx.send(Control::SetPaused(false)).await.unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match updates_rx.recv().await {
                Some(Update::Tps(tps)) => return tps,
                Some(_) => continue,
                None => panic!("collector stopped"),
            }
        }
    })
    .await;
    assert!(resumed.is_ok(), "collection did not resume");
}

#[tokio::test]
async fn refreshing_on_demand_produces_a_reading_immediately() {
    let mut config = start(mock::Flavour::Folia).await;
    // Long intervals, so anything that arrives promptly came from the refresh.
    config.refresh.tick_ms = 600_000;
    config.refresh.region_ms = 600_000;
    config.refresh.roster_ms = 600_000;
    let config = Arc::new(config);

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    controls_tx.send(Control::RefreshNow).await.unwrap();

    let mut app = App::new(config);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while app.tps.current().is_none() {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
    }

    assert_eq!(app.tps.current(), Some(19.87));
}

#[tokio::test]
async fn folia_region_detail_costs_no_extra_round_trips() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let executed = Arc::new(AtomicUsize::new(0));
    tokio::spawn(mock::serve_counting(
        listener,
        "secret".into(),
        mock::Flavour::Folia,
        Arc::clone(&executed),
    ));

    let mut config = Config::default();
    config.rcon.address = address;
    config.rcon.password = Some("secret".into());
    config.process.enabled = false;
    config.jvm.enabled = false;
    // The defaults point the region report and the tick rate at the same
    // command, so one poll should serve both.
    assert_eq!(config.commands.regions, config.commands.tps);
    config.refresh = RefreshConfig {
        tick_ms: 200,
        region_ms: 200,
        roster_ms: 600_000,
        process_ms: 600_000,
        disk_secs: 3_600,
        draw_ms: 200,
    };

    let config = Arc::new(config);
    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (_controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    let mut app = App::new(config);
    let mut tick_polls = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2_000);
    while let Ok(Some(update)) = tokio::time::timeout_at(deadline, updates_rx.recv()).await {
        if matches!(update, Update::Tps(_)) {
            tick_polls += 1;
        }
        app.apply(update);
    }

    assert_eq!(app.regions.regions.len(), 4, "region detail should arrive");
    assert!(tick_polls > 2, "only {tick_polls} tick polls ran");

    // Each tick poll runs `tps` and `mspt`; the single roster poll at startup
    // adds the player list and the version. Asking `tps` a second time for the
    // region report would make it three per tick instead of two. Counting
    // relative to the polls that actually ran keeps this stable under load.
    let executed = executed.load(Ordering::SeqCst);
    let ceiling = tick_polls * 2 + 4;
    assert!(
        executed <= ceiling,
        "ran {executed} commands for {tick_polls} tick polls (at most {ceiling} expected); \
         the region report should ride along with the tick poll"
    );
}

#[tokio::test]
async fn a_separate_region_command_is_still_polled_on_its_own_clock() {
    let mut config = start(mock::Flavour::Folia).await;
    // Point the region report at a command of its own, and stop the tick clock.
    config.commands.regions = "tps --regions".into();
    config.refresh.tick_ms = 600_000;
    config.refresh.region_ms = 200;

    let app = collect(config, |app| !app.regions.regions.is_empty()).await;
    assert_eq!(app.regions.regions.len(), 4);
    assert!(
        app.raw
            .iter()
            .any(|(command, _)| command == "tps --regions"),
        "the configured command should be the one that was run"
    );
}

/// `mctop status` is meant for scripts, so its exit code has to mean something.
#[tokio::test]
async fn status_fails_when_the_tick_rate_cannot_be_read() {
    use std::process::Command;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(mock::serve(
        listener,
        "secret".into(),
        mock::Flavour::Unrecognisable,
    ));

    let broken = tokio::task::spawn_blocking({
        let address = address.clone();
        move || {
            Command::new(env!("CARGO_BIN_EXE_mctop"))
                .env("MCTOP_RCON_PASSWORD", "secret")
                .args(["--address", &address, "status"])
                .output()
                .unwrap()
        }
    })
    .await
    .unwrap();

    assert!(
        !broken.status.success(),
        "a server it cannot read is a failure"
    );
    let complaint = String::from_utf8_lossy(&broken.stderr);
    assert!(complaint.contains("mctop probe"), "{complaint}");
}

#[tokio::test]
async fn status_succeeds_against_a_server_it_understands() {
    use std::process::Command;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(mock::serve(listener, "secret".into(), mock::Flavour::Folia));

    let good = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_mctop"))
            .env("MCTOP_RCON_PASSWORD", "secret")
            .args(["--address", &address, "status"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let report = String::from_utf8_lossy(&good.stdout);
    assert!(report.contains("Folia 1.21.4"), "{report}");
    assert!(report.contains("busiest   world (96, -8)"), "{report}");
}

/// A server that has stopped answering is the one whose CPU and memory matter
/// most, so the local collectors must not be waiting behind a console command.
#[tokio::test]
async fn local_sampling_continues_while_the_server_is_hung() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(mock::serve(
        listener,
        "secret".into(),
        mock::Flavour::Unresponsive,
    ));

    let mut config = Config::default();
    config.rcon.address = address;
    config.rcon.password = Some("secret".into());
    // Long enough that a shared loop would be stuck in it for this whole test.
    config.rcon.command_timeout_ms = 30_000;
    // Watch this test binary; the heap needs a JDK, which this does not.
    config.process.pid = Some(std::process::id());
    config.process.enabled = true;
    config.jvm.enabled = false;
    config.refresh = RefreshConfig {
        tick_ms: 200,
        region_ms: 200,
        roster_ms: 200,
        process_ms: 200,
        disk_secs: 3_600,
        draw_ms: 200,
    };

    let config = Arc::new(config);
    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (_controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    let mut app = App::new(config);
    let mut samples = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1_500);
    while let Ok(Some(update)) = tokio::time::timeout_at(deadline, updates_rx.recv()).await {
        if matches!(update, Update::Process(Some(_))) {
            samples += 1;
        }
        app.apply(update);
    }

    assert!(
        samples >= 3,
        "only {samples} process samples arrived while the server was hung"
    );
    assert_eq!(
        app.process.as_ref().map(|stats| stats.pid),
        Some(std::process::id())
    );
    // And nothing was invented about the server itself.
    assert!(app.tps.current().is_none());
}

#[tokio::test]
async fn a_typed_command_reaches_the_server_and_its_reply_comes_back() {
    let config = Arc::new(start(mock::Flavour::Folia).await);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    controls_tx
        .send(Control::Send("list".into()))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
        if app.log.iter().any(|entry| entry.kind == Kind::Received) {
            break;
        }
    }

    // The command is echoed, so the scrollback reads as a conversation.
    let sent = app
        .log
        .iter()
        .find(|entry| entry.kind == Kind::Sent)
        .expect("the command should be echoed");
    assert_eq!(sent.message, "list");

    let reply = app
        .log
        .iter()
        .find(|entry| entry.kind == Kind::Received)
        .expect("the reply should be shown");
    assert!(reply.message.contains("players online"), "{reply:?}");
    // Colour codes are stripped before the reply is shown.
    assert!(!reply.message.contains('\u{a7}'));

    // A typed command is not poll traffic, so it stays out of the raw ring.
    assert!(app.raw.iter().all(|(command, _)| command != "list"));
}

#[tokio::test]
async fn a_multi_line_reply_becomes_one_entry_per_line() {
    let config = Arc::new(start(mock::Flavour::Folia).await);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    // The mock's tps report is several lines long.
    controls_tx.send(Control::Send("tps".into())).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
        if app.log.iter().filter(|e| e.kind == Kind::Received).count() > 4 {
            break;
        }
    }

    let received = app.log.iter().filter(|e| e.kind == Kind::Received).count();
    assert!(
        received >= 5,
        "got {received} lines from a multi-line reply"
    );
    assert!(
        app.log.iter().all(|entry| !entry.message.contains('\n')),
        "each line should be its own entry so scrolling counts lines"
    );
}

#[tokio::test]
async fn a_command_that_fails_says_so_in_the_console() {
    let mut config = start(mock::Flavour::Folia).await;
    config.rcon.address = "127.0.0.1:1".into();
    let config = Arc::new(config);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    controls_tx
        .send(Control::Send("list".into()))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
        if app.log.iter().any(|entry| entry.kind == Kind::Error) {
            break;
        }
    }

    assert!(
        app.log.iter().any(|entry| entry.kind == Kind::Sent),
        "the attempt is still echoed"
    );
    assert!(
        app.log.iter().any(|entry| entry.kind == Kind::Error),
        "and the failure is reported rather than swallowed"
    );
}

#[tokio::test]
async fn a_command_runs_even_while_polling_is_paused() {
    let config = Arc::new(start(mock::Flavour::Folia).await);
    let mut app = App::new(Arc::clone(&config));

    let (updates_tx, mut updates_rx) = mpsc::channel(256);
    let (controls_tx, controls_rx) = mpsc::channel(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    controls_tx.send(Control::SetPaused(true)).await.unwrap();
    controls_tx
        .send(Control::Send("list".into()))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, updates_rx.recv()).await {
            Ok(Some(update)) => app.apply(update),
            _ => break,
        }
        if app.log.iter().any(|entry| entry.kind == Kind::Received) {
            break;
        }
    }

    // Pausing stops mctop asking questions; it does not gag the operator.
    assert!(app.log.iter().any(|entry| entry.kind == Kind::Received));
    assert!(app.tps.current().is_none(), "polling really was paused");
}
