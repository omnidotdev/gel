//! The `gel eval` command: evaluate a Rust config crate into an artifact
//!
//! This is the eval half of gel's eval/apply split. A user config is an ordinary
//! Rust binary that prints a [`DesiredState`] as JSON on stdout; `eval` runs it
//! with cargo, captures that JSON, and writes it to a desired-state artifact that
//! `diff` and `apply` later consume. It performs no package operations, so it is
//! safe to run without the `arch` feature.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use gel_core::state::DesiredState;

use crate::paths;

/// Evaluate the config crate at `dir` and write its desired state to an artifact
///
/// `out` overrides the destination; the default is [`paths::default_artifact`].
///
/// # Errors
///
/// Returns an error when cargo cannot be run, the config fails to evaluate, its
/// output is not a valid desired state, or the artifact cannot be written.
pub fn run(dir: &Path, out: Option<PathBuf>) -> anyhow::Result<()> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.exists() {
        bail!("no Cargo.toml found at {}", manifest.display());
    }

    // run the config crate; its stdout is the desired state as JSON
    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .context("failed to run cargo to evaluate the config")?;
    if !output.status.success() {
        // surface cargo's own diagnostics to the developer, then fail concisely
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        bail!("config evaluation failed for {}", dir.display());
    }

    let stdout = String::from_utf8(output.stdout).context("config produced non-UTF-8 output")?;
    let desired = parse_desired(&stdout)?;

    let artifact = match out {
        Some(path) => path,
        None => paths::default_artifact()?,
    };
    if let Some(parent) = artifact.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&desired)?;
    fs::write(&artifact, json)
        .with_context(|| format!("failed to write artifact to {}", artifact.display()))?;

    let services = desired.services.enable.len() + desired.services.disable.len();
    let settings = desired.settings.declared().len();
    println!(
        "evaluated config: {} native, {} foreign, {} files, {} services, {} settings -> {}",
        desired.native.len(),
        desired.foreign.len(),
        desired.files.len(),
        services,
        settings,
        artifact.display()
    );
    Ok(())
}

/// Parse the JSON a config crate prints into a [`DesiredState`]
///
/// # Errors
///
/// Returns an error when `stdout` is not a valid desired-state document.
pub fn parse_desired(stdout: &str) -> anyhow::Result<DesiredState> {
    serde_json::from_str(stdout.trim()).context("config did not produce a valid desired state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_desired_state_json() {
        let json = r#"{"native":["git","ripgrep"],"foreign":["yay"]}"#;

        let desired = parse_desired(json).expect("parse");

        assert_eq!(desired.native, vec!["git".to_owned(), "ripgrep".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        // a config that prints with a trailing newline still parses
        let json = "\n  {\"native\":[],\"foreign\":[]}  \n";

        let desired = parse_desired(json).expect("parse");

        assert!(desired.native.is_empty());
        assert!(desired.foreign.is_empty());
    }

    #[test]
    fn rejects_non_desired_state_output() {
        let result = parse_desired("not json at all");

        assert!(result.is_err());
    }
}
