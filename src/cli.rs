use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Terminal dashboard for Folia and Paper Minecraft servers.
///
/// Run with no subcommand to open the dashboard.
#[derive(Debug, Parser)]
#[command(name = "mctop", version, about, long_about = None)]
pub struct Cli {
    /// Print more detail while running
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Config file to use instead of the default location
    #[arg(short, long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// RCON address, e.g. `127.0.0.1:25575`; overrides the config
    #[arg(short, long, global = true, value_name = "HOST[:PORT]")]
    pub address: Option<String>,

    /// Seconds between TPS readings; overrides the config
    #[arg(short, long, global = true, value_name = "SECONDS")]
    pub interval: Option<f64>,

    /// Server directory, used to find world folders; overrides the config
    #[arg(short = 'd', long, global = true, value_name = "PATH")]
    pub directory: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the dashboard. This is what running `mctop` with no arguments does.
    Watch,

    /// Print a single reading and exit, for scripts and health checks
    Status {
        /// Server address, e.g. `play.example.net` or `10.0.0.5:25565`
        address: Option<String>,
    },

    /// Run each configured command and print its unparsed response.
    ///
    /// Use this when a metric shows as unavailable: it shows exactly what the
    /// server replied, so `[commands]` can be pointed at something it knows.
    Probe,

    /// Show or create the config file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the path mctop reads its config from
    Path,
    /// Write a commented starter config, if none exists
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Print the configuration in force, after all overrides
    Show,
}
