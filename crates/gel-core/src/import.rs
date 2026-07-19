use crate::{backend::PackageBackend, error::GelError, state::DesiredState};

/// Import the current machine's explicit packages as a desired state
///
/// This is the inverse of applying: it snapshots what the backend reports as
/// explicitly installed so it can be captured into a config.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to query package state.
pub fn import(b: &impl PackageBackend) -> Result<DesiredState, GelError> {
    b.query_explicit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;

    #[test]
    fn import_captures_explicit_native_and_foreign() {
        let backend = FakeBackend::with_explicit(&["git", "vim"], &["yay"]);

        let desired = import(&backend).expect("import");

        assert_eq!(desired.native, vec!["git".to_owned(), "vim".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
    }
}
