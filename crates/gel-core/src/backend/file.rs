use crate::error::GelError;

/// An abstraction over a filesystem for managed files
///
/// Implementations perform the real side effects (reading, writing, and removing
/// file content). The core engine drives a backend through this trait so that
/// planning stays pure and testable against an in-memory fake.
pub trait FileBackend {
    /// Return the current content of `path`, or `None` if it does not exist
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to read the file.
    fn read_file(&self, path: &str) -> Result<Option<String>, GelError>;

    /// Write `content` to `path`, creating or replacing the file
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to write the file.
    fn write_file(&mut self, path: &str, content: &str) -> Result<(), GelError>;

    /// Remove the file at `path`
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the backend fails to remove the file.
    fn remove_file(&mut self, path: &str) -> Result<(), GelError>;
}
