use crate::error::GelError;

/// An abstraction over kernel parameters (sysctl)
///
/// Implementations perform the real side effects (reading a parameter's current
/// value and writing a new one). The core engine drives a backend through this
/// trait so that planning stays pure and testable against an in-memory fake.
///
/// Only parameters named in a [`DesiredState::sysctl`](crate::state::DesiredState)
/// are ever passed here: gel operates on explicit intent, never a full-set
/// convergence, so it never changes a parameter it was not told about.
pub trait SysctlBackend {
    /// Return the current value of `name`, or `None` when unset or unreadable
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to read the parameter.
    fn get(&self, name: &str) -> Result<Option<String>, GelError>;

    /// Set `name` to `value`
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to set the parameter.
    fn set(&mut self, name: &str, value: &str) -> Result<(), GelError>;
}
