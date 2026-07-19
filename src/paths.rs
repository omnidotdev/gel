//! Resolution of the on-disk locations gel reads and writes
//!
//! gel keeps its desired-state artifact and transaction journal under a per-user
//! state directory: `${XDG_STATE_HOME:-~/.local/state}/gel`.

use std::path::PathBuf;

use anyhow::Context;

/// The gel state directory: `${XDG_STATE_HOME:-~/.local/state}/gel`
///
/// # Errors
///
/// Returns an error when neither `XDG_STATE_HOME` nor `HOME` can be resolved.
pub fn state_dir() -> anyhow::Result<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
        if !base.is_empty() {
            return Ok(PathBuf::from(base).join("gel"));
        }
    }
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .context(
            "cannot resolve the gel state directory: neither XDG_STATE_HOME nor HOME is set",
        )?;
    Ok(PathBuf::from(home).join(".local/state/gel"))
}

/// The default desired-state artifact path, `<state-dir>/desired.json`
///
/// # Errors
///
/// Returns an error when the state directory cannot be resolved.
pub fn default_artifact() -> anyhow::Result<PathBuf> {
    Ok(state_dir()?.join("desired.json"))
}

/// The transaction journal directory, `<state-dir>/journal`
///
/// # Errors
///
/// Returns an error when the state directory cannot be resolved.
#[cfg(feature = "arch")]
pub fn journal_dir() -> anyhow::Result<PathBuf> {
    Ok(state_dir()?.join("journal"))
}
