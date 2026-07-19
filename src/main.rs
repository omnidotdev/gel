use clap::Parser;

/// Declarative system management for Arch Linux
#[derive(Parser)]
#[command(name = "gel")]
#[command(version, about)]
struct Cli {}

// the binary is fallible by convention (anyhow); later tasks add commands that
// return errors, so the `Result` return is not yet exercised
#[allow(clippy::unnecessary_wraps)]
fn main() -> anyhow::Result<()> {
    // clap handles `--version` and `--help`, exiting before this returns
    Cli::parse();

    Ok(())
}
