//! gel: declarative system management for Arch Linux
//!
//! The CLI implements an eval/apply split. `gel eval` compiles and runs a Rust
//! config crate into a desired-state artifact (pure, works in any build). The
//! system-touching commands are wired in a following change.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod eval;
mod paths;

/// Declarative system management for Arch Linux
#[derive(Parser)]
#[command(name = "gel")]
#[command(version, about)]
#[command(after_help = STATE_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Extra `--help` detail on the eval/apply split and on-disk locations
const STATE_HELP: &str = "\
Eval/apply split:
  `gel eval <dir>` compiles and runs a Rust config crate and writes the desired
  state to an artifact for the system-touching commands to consume.

Locations (override the artifact with --out):
  artifact  ${XDG_STATE_HOME:-~/.local/state}/gel/desired.json";

#[derive(Subcommand)]
enum Command {
    /// Evaluate a Rust config crate into a desired-state artifact
    Eval {
        /// Directory of the config crate (contains its Cargo.toml)
        dir: PathBuf,
        /// Write the artifact here instead of the default state path
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        // eval is pure (runs cargo, writes a file); available in every build
        Command::Eval { dir, out } => eval::run(&dir, out),
    }
}
