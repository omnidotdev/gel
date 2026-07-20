use crate::error::GelError;

/// An abstraction over a service manager (systemd) for unit enable state
///
/// Implementations perform the real side effects (querying whether a unit is
/// enabled, enabling it, disabling it). The core engine drives a backend through
/// this trait so that planning stays pure and testable against an in-memory fake.
///
/// Only units named in a [`ServiceIntent`](crate::state::ServiceIntent) are ever
/// passed here: gel operates on explicit intent, never a full-set convergence, so
/// it never disables a unit it was not told about.
pub trait ServiceBackend {
    /// Return whether `unit` is currently enabled
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to query the unit.
    fn is_enabled(&self, unit: &str) -> Result<bool, GelError>;

    /// Enable `unit`
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to enable the unit.
    fn enable(&mut self, unit: &str) -> Result<(), GelError>;

    /// Disable `unit`
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to disable the unit.
    fn disable(&mut self, unit: &str) -> Result<(), GelError>;
}
