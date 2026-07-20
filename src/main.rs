//! gel: declarative system management for Arch Linux
//!
//! The CLI implements an eval/apply split. `gel eval` compiles and runs a Rust
//! config crate into a desired-state artifact (pure, works in any build). The
//! system-touching commands (`import`, `diff`, `apply`, `rollback`) drive the
//! real Arch backend and are available only when built with `--features arch`;
//! without it they fail fast with a clear rebuild message.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod eval;
mod paths;

// Rendering and the system-touching commands only exist in an Arch build; the
// default build fast-fails those subcommands instead.
#[cfg(feature = "arch")]
mod render;
#[cfg(feature = "arch")]
mod system;

/// Shown when a system-touching command runs in a build without `arch` support
#[cfg(not(feature = "arch"))]
const NO_ARCH_MESSAGE: &str = "gel was built without Arch support; rebuild with --features arch";

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
  state to an artifact. `gel diff`/`gel apply` then read that artifact.

Locations (override the artifact with --out/--artifact):
  artifact  ${XDG_STATE_HOME:-~/.local/state}/gel/desired.json
  journal   ${XDG_STATE_HOME:-~/.local/state}/gel/journal

Arch support:
  import/diff/apply/rollback touch the system and require a build with
  `--features arch`. `rollback` reverses packages, managed files, and service
  enable/disable state; snapshot-based filesystem restore is planned for a
  later phase.";

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
    /// Show the plan to converge the system toward the artifact (read-only)
    Diff {
        /// Read the desired state from here instead of the default state path
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    /// Converge the system toward the desired artifact
    Apply {
        /// Also remove explicit packages absent from the desired state
        #[arg(long)]
        prune: bool,
        /// Read the desired state from here instead of the default state path
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    /// Capture the current explicit packages as a desired state
    Import {
        /// Write the imported state here instead of printing it to stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Roll back the most recent apply at the package level
    Rollback,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        // eval is pure (runs cargo, writes a file); available in every build
        Command::Eval { dir, out } => eval::run(&dir, out),

        // the system-touching commands need the real Arch backend
        #[cfg(feature = "arch")]
        Command::Diff { artifact } => system::diff(artifact),
        #[cfg(feature = "arch")]
        Command::Apply { prune, artifact } => system::apply_cmd(prune, artifact),
        #[cfg(feature = "arch")]
        Command::Import { out } => system::import(out),
        #[cfg(feature = "arch")]
        Command::Rollback => system::rollback(),

        // without the `arch` feature they fast-fail before touching anything
        #[cfg(not(feature = "arch"))]
        Command::Diff { .. }
        | Command::Apply { .. }
        | Command::Import { .. }
        | Command::Rollback => anyhow::bail!(NO_ARCH_MESSAGE),
    }
}
