use crate::{backend::PackageBackend, error::GelError, state::SystemState};

/// A recorded backend operation, in the order it was invoked
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `install_native` was called with these packages
    InstallNative(Vec<String>),
    /// `remove_native` was called with these packages
    RemoveNative(Vec<String>),
    /// `install_foreign` was called with these packages
    InstallForeign(Vec<String>),
    /// `remove_foreign` was called with these packages
    RemoveForeign(Vec<String>),
}

/// An in-memory [`PackageBackend`] for tests
///
/// It holds native and foreign package sets, mutates them on install/remove,
/// and records an ordered log of every mutating call so tests can assert both
/// resulting state and the exact operations performed.
#[derive(Debug, Default, Clone)]
pub struct FakeBackend {
    native: Vec<String>,
    foreign: Vec<String>,
    calls: Vec<Call>,
}

impl FakeBackend {
    /// Construct a backend seeded with the given explicit native and foreign sets
    #[must_use]
    pub fn with_explicit(native: &[&str], foreign: &[&str]) -> Self {
        Self {
            native: native.iter().map(|s| (*s).to_owned()).collect(),
            foreign: foreign.iter().map(|s| (*s).to_owned()).collect(),
            calls: Vec::new(),
        }
    }

    /// Return the ordered log of mutating calls made against this backend
    #[must_use]
    pub fn calls(&self) -> &[Call] {
        &self.calls
    }
}

/// Add packages absent from `set`, preserving insertion order
fn add_missing(set: &mut Vec<String>, pkgs: &[String]) {
    for pkg in pkgs {
        if !set.contains(pkg) {
            set.push(pkg.clone());
        }
    }
}

/// Remove any packages in `pkgs` from `set`
fn remove_present(set: &mut Vec<String>, pkgs: &[String]) {
    set.retain(|pkg| !pkgs.contains(pkg));
}

impl PackageBackend for FakeBackend {
    fn query_explicit(&self) -> Result<SystemState, GelError> {
        Ok(SystemState {
            native: self.native.clone(),
            foreign: self.foreign.clone(),
        })
    }

    fn install_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        add_missing(&mut self.native, pkgs);
        self.calls.push(Call::InstallNative(pkgs.to_vec()));
        Ok(())
    }

    fn remove_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        remove_present(&mut self.native, pkgs);
        self.calls.push(Call::RemoveNative(pkgs.to_vec()));
        Ok(())
    }

    fn install_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        add_missing(&mut self.foreign, pkgs);
        self.calls.push(Call::InstallForeign(pkgs.to_vec()));
        Ok(())
    }

    fn remove_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        remove_present(&mut self.foreign, pkgs);
        self.calls.push(Call::RemoveForeign(pkgs.to_vec()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::PackageBackend;

    #[test]
    fn query_returns_explicit_sets() {
        let backend = FakeBackend::with_explicit(&["git", "vim"], &["yay"]);

        let state = backend.query_explicit().expect("query");

        assert_eq!(state.native, vec!["git".to_owned(), "vim".to_owned()]);
        assert_eq!(state.foreign, vec!["yay".to_owned()]);
    }

    #[test]
    fn install_native_updates_state_and_records_call() {
        let mut backend = FakeBackend::with_explicit(&["git"], &[]);

        backend
            .install_native(&["ripgrep".to_owned()])
            .expect("install");

        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"ripgrep".to_owned()));
        assert_eq!(
            backend.calls(),
            &[Call::InstallNative(vec!["ripgrep".to_owned()])]
        );
    }
}
