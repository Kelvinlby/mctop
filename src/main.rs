use clap::Parser;

use mctop::cli::Cli;

fn main() -> anyhow::Result<()> {
    mctop::run(Cli::parse())
}
