//! System command execution used by the real Arch backend and btrfs provider
//!
//! This module is only compiled with the `arch` feature. It defines a thin
//! [`CommandRunner`] seam so the real implementations can be unit tested by
//! asserting the exact argv they build, without executing anything on the host.

use std::process::Command;

use crate::error::GelError;

/// Captured result of running an external command
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    /// Whether the process exited with a success status
    pub success: bool,
    /// Captured standard output
    pub stdout: String,
    /// Captured standard error
    pub stderr: String,
}

/// Runs external commands and resolves program availability
///
/// Abstracted behind a trait so tests can assert the constructed argv without
/// executing a real process.
pub trait CommandRunner {
    /// Run `program` with `args`, capturing its output
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the process cannot be spawned.
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, GelError>;

    /// Return true when `program` is resolvable on the current PATH
    fn is_available(&self, program: &str) -> bool;
}

/// A [`CommandRunner`] backed by the real operating system
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, GelError> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn is_available(&self, program: &str) -> bool {
        which::which(program).is_ok()
    }
}
