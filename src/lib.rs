//! Core of `mctop`, a terminal monitor for Folia and Paper Minecraft servers.
//!
//! Readings come from two places. TPS, tick times, the player list, and — on
//! Folia — the per-region breakdown are asked for over RCON. CPU, memory, heap
//! occupancy, and world sizes cannot be: the game does not know them. Those are
//! sampled from the machine mctop runs on, which is why the dashboard is at its
//! most useful on the server's own host.

pub mod app;
pub mod cli;
pub mod config;
pub mod format;
pub mod metrics;
pub mod server;
pub mod source;
pub mod tui;
pub mod ui;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;

use cli::{Cli, Command, ConfigAction};
use config::Config;
use server::Address;

/// Run the CLI as parsed from the command line.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    // The config subcommands never touch the network, so they do not need a
    // runtime and stay usable when the server is unreachable.
    if let Some(Command::Config { action }) = &cli.command {
        return config_command(action, &cli);
    }

    let (config, path) = load(&cli)?;
    let config = Arc::new(config);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // The work here is a handful of timers and a socket; the default pool
        // of one thread per core is far more than this needs.
        .worker_threads(2)
        .build()
        .context("starting the async runtime")?;

    match cli.command {
        None | Some(Command::Watch) => runtime.block_on(tui::run(config, path)),
        Some(Command::Status { address }) => runtime.block_on(status(config, address, cli.verbose)),
        Some(Command::Probe) => runtime.block_on(probe(config)),
        Some(Command::Config { .. }) => unreachable!("handled above"),
    }
}

/// Load the config and fold the command-line overrides into it.
fn load(cli: &Cli) -> anyhow::Result<(Config, Option<PathBuf>)> {
    let (mut config, path) = Config::load(cli.config.as_deref())?;

    if let Some(address) = &cli.address {
        // Validate here rather than at connect time, so a typo is reported
        // before the screen is taken over.
        Address::parse_with_default_port(address, source::rcon::DEFAULT_RCON_PORT)
            .context("--address")?;
        config.rcon.address = address.clone();
    }

    if let Some(interval) = cli.interval {
        anyhow::ensure!(
            interval.is_finite() && interval > 0.0,
            "--interval must be a positive number of seconds"
        );
        let milliseconds = (interval * 1000.0).round() as u64;
        config.refresh.tick_ms = milliseconds;
        // The region report usually comes from the same command as the tick
        // rate, so leaving it on its own clock would only put two figures on
        // screen that disagree about how old they are.
        config.refresh.region_ms = milliseconds;
    }

    if let Some(directory) = &cli.directory {
        config.server.directory = Some(directory.clone());
    }

    Ok((config, path))
}

/// One reading, printed as plain text.
async fn status(config: Arc<Config>, address: Option<String>, verbose: bool) -> anyhow::Result<()> {
    let mut config = (*config).clone();
    if let Some(address) = address {
        // `mctop status host` names the game address; RCON is a separate port,
        // so only the host carries over.
        let parsed = Address::parse(&address)?;
        if verbose {
            eprintln!("querying {parsed}");
        }
        // Display re-brackets an IPv6 literal, which a plain format! would not.
        config.rcon.address = Address {
            host: parsed.host,
            port: config_port(&config),
        }
        .to_string();
    }

    let mut client = source::rcon::RconClient::new(&config.rcon)?;
    let commands = &config.commands;

    let tps = source::parse::parse_tps(&client.command(&commands.tps).await?);
    let mspt = source::parse::parse_mspt(&client.command(&commands.mspt).await.unwrap_or_default());
    let players =
        source::parse::parse_players(&client.command(&commands.players).await.unwrap_or_default());
    let identity =
        source::parse::parse_version(&client.command(&commands.version).await.unwrap_or_default());
    let regions =
        source::parse::parse_regions(&client.command(&commands.regions).await.unwrap_or_default());

    let mut out = std::io::stdout().lock();
    writeln!(out, "server    {}", identity.summary())?;
    writeln!(
        out,
        "tps       {}",
        if tps.windows.is_empty() {
            "unavailable".to_owned()
        } else {
            tps.windows
                .iter()
                .map(|(label, value)| format!("{label} {value:.2}"))
                .collect::<Vec<_>>()
                .join("  ")
        }
    )?;
    if let Some(window) = mspt.current() {
        writeln!(
            out,
            "mspt      avg {:.2}  min {:.2}  max {:.2}",
            window.average, window.minimum, window.maximum
        )?;
    }
    writeln!(
        out,
        "players   {}{}",
        players.online,
        players.max.map_or(String::new(), |max| format!(" / {max}"))
    )?;
    if let Some(total) = regions.total {
        writeln!(out, "regions   {total}")?;
        if let Some(worst) = regions.worst() {
            writeln!(
                out,
                "busiest   {} at {}",
                worst.label(),
                format::percent(worst.pressure())
            )?;
        }
    }

    for world in source::disk::scan(&config.resolved_worlds()).worlds {
        writeln!(
            out,
            "world     {:<16} {}",
            world.name,
            format::bytes(world.bytes)
        )?;
    }
    out.flush()?;

    // A health check that prints "unavailable" and exits zero is a trap, so an
    // unreadable tick rate is a failure even though the server did answer.
    anyhow::ensure!(
        !tps.windows.is_empty(),
        "could not read the tick rate from `{}`; run `mctop probe` to see what \
         this server replied, then set [commands] to something it understands",
        commands.tps
    );

    Ok(())
}

fn config_port(config: &Config) -> u16 {
    Address::parse_with_default_port(&config.rcon.address, source::rcon::DEFAULT_RCON_PORT)
        .map_or(source::rcon::DEFAULT_RCON_PORT, |address| address.port)
}

/// Print the unparsed response to every configured command.
async fn probe(config: Arc<Config>) -> anyhow::Result<()> {
    let mut client = source::rcon::RconClient::new(&config.rcon)?;
    let commands = &config.commands;

    // On Folia the region detail rides along with the TPS report, so the same
    // command often appears twice. Running it twice would only pad the output.
    let mut wanted: Vec<(&str, &str)> = Vec::new();
    for (metric, command) in [
        ("tps", commands.tps.as_str()),
        ("mspt", commands.mspt.as_str()),
        ("regions", commands.regions.as_str()),
        ("players", commands.players.as_str()),
        ("version", commands.version.as_str()),
    ] {
        if !command.trim().is_empty() && !wanted.iter().any(|&(_, seen)| seen == command) {
            wanted.push((metric, command));
        }
    }

    let mut out = std::io::stdout().lock();
    writeln!(out, "probing {}\n", client.address())?;

    for (metric, command) in wanted {
        writeln!(out, "── {metric}: `{command}` {}", "─".repeat(40))?;
        match client.command(command).await {
            Ok(response) => {
                let text = source::parse::strip_formatting(&response);
                if text.trim().is_empty() {
                    writeln!(out, "  (empty response)")?;
                } else {
                    for line in text.lines() {
                        writeln!(out, "  {line}")?;
                    }
                }
            }
            Err(error) => writeln!(out, "  failed: {error:#}")?,
        }
        writeln!(out)?;
    }

    Ok(())
}

fn config_command(action: &ConfigAction, cli: &Cli) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => {
            let path = cli
                .config
                .clone()
                .or_else(config::default_path)
                .context("no config directory on this platform")?;
            println!(
                "{}{}",
                path.display(),
                if path.exists() {
                    ""
                } else {
                    "  (does not exist yet)"
                }
            );
            Ok(())
        }
        ConfigAction::Init { force } => {
            let path = cli
                .config
                .clone()
                .or_else(config::default_path)
                .context("no config directory on this platform")?;

            if path.exists() && !force {
                anyhow::bail!(
                    "{} already exists; pass --force to overwrite it",
                    path.display()
                );
            }

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, config::TEMPLATE)
                .with_context(|| format!("writing {}", path.display()))?;

            println!("wrote {}", path.display());
            println!("Set [rcon].address and the password, then run `mctop`.");
            Ok(())
        }
        ConfigAction::Show => {
            let (config, path) = load(cli)?;
            match path {
                Some(path) => println!("# from {}", path.display()),
                None => println!("# no config file found; these are the defaults"),
            }
            print!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
    }
}
