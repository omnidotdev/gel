use crate::{error::GelError, state::SystemState};

pub mod file;
pub mod service;
pub mod settings;

#[cfg(any(test, feature = "test-util"))]
pub mod fake;

// Real Arch Linux backend, compiled only with the `arch` feature
#[cfg(feature = "arch")]
pub mod arch;

/// An abstraction over a system package manager
///
/// Implementations perform the real side effects (querying, installing, and
/// removing packages). The core engine drives a backend through this trait so
/// that planning and orchestration stay pure and testable.
pub trait PackageBackend {
    /// Return the set of explicitly installed packages, split by origin
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to query package state.
    fn query_explicit(&self) -> Result<SystemState, GelError>;

    /// Install the given native (official-repo) packages
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to install a package.
    fn install_native(&mut self, pkgs: &[String]) -> Result<(), GelError>;

    /// Remove the given native (official-repo) packages
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to remove a package.
    fn remove_native(&mut self, pkgs: &[String]) -> Result<(), GelError>;

    /// Install the given foreign (AUR) packages
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to install a package.
    fn install_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError>;

    /// Remove the given foreign (AUR) packages
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to remove a package.
    fn remove_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError>;
}
