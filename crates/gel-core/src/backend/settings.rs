use crate::{error::GelError, state::SettingKey};

/// An abstraction over global system settings (hostname, timezone, locale)
///
/// Implementations perform the real side effects (reading a setting's current
/// value and writing a new one). The core engine drives a backend through this
/// trait so that planning stays pure and testable against an in-memory fake.
///
/// Only settings named in a [`SettingsIntent`](crate::state::SettingsIntent) are
/// ever passed here: gel operates on explicit intent, never a full-set
/// convergence, so it never changes a setting it was not told about.
pub trait SettingsBackend {
    /// Return the current value of `key`, or `None` when it is unset or unreadable
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to read the setting.
    fn get(&self, key: SettingKey) -> Result<Option<String>, GelError>;

    /// Set `key` to `value`
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to set the setting.
    fn set(&mut self, key: SettingKey, value: &str) -> Result<(), GelError>;
}
