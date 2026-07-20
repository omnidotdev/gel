use std::collections::HashMap;

use crate::{
    backend::{PackageBackend, file::FileBackend},
    error::GelError,
    state::SystemState,
};

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
    /// `write_file` was called for this path
    WriteFile(String),
    /// `remove_file` was called for this path
    RemoveFile(String),
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
    files: HashMap<String, String>,
    calls: Vec<Call>,
    fail_on: Option<Call>,
}

impl FakeBackend {
    /// Construct a backend seeded with the given explicit native and foreign sets
    #[must_use]
    pub fn with_explicit(native: &[&str], foreign: &[&str]) -> Self {
        Self {
            native: native.iter().map(|s| (*s).to_owned()).collect(),
            foreign: foreign.iter().map(|s| (*s).to_owned()).collect(),
            ..Self::default()
        }
    }

    /// Construct a backend seeded with the given files as `(path, content)` pairs
    #[must_use]
    pub fn with_files(files: &[(&str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
                .collect(),
            ..Self::default()
        }
    }

    /// Seed or overwrite a single file's content without recording a call
    pub fn set_file(&mut self, path: &str, content: &str) {
        self.files.insert(path.to_owned(), content.to_owned());
    }

    /// Return the ordered log of mutating calls made against this backend
    #[must_use]
    pub fn calls(&self) -> &[Call] {
        &self.calls
    }

    /// Inject a one-shot failure on the next mutator matching `call`'s variant
    ///
    /// Matching is by [`Call`] variant, not by package contents, so the exact
    /// packages passed to the failing call do not need to be known in advance.
    /// The next matching mutator records its attempted call, then returns an
    /// error without mutating state; subsequent calls of that variant succeed.
    pub fn set_fail_on(&mut self, call: Call) {
        self.fail_on = Some(call);
    }

    /// Consume a queued failure when its variant matches `call`
    ///
    /// Returns true when the pending `fail_on` matches, clearing it so only the
    /// next matching mutator fails
    fn take_failure(&mut self, call: &Call) -> bool {
        if self
            .fail_on
            .as_ref()
            .is_some_and(|target| std::mem::discriminant(target) == std::mem::discriminant(call))
        {
            self.fail_on = None;
            return true;
        }
        false
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
            files: Vec::new(),
            services: crate::state::ServiceIntent::default(),
        })
    }

    fn install_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        let call = Call::InstallNative(pkgs.to_vec());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend(
                "injected install_native failure".to_owned(),
            ));
        }
        add_missing(&mut self.native, pkgs);
        self.calls.push(call);
        Ok(())
    }

    fn remove_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        let call = Call::RemoveNative(pkgs.to_vec());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend(
                "injected remove_native failure".to_owned(),
            ));
        }
        remove_present(&mut self.native, pkgs);
        self.calls.push(call);
        Ok(())
    }

    fn install_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        let call = Call::InstallForeign(pkgs.to_vec());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend(
                "injected install_foreign failure".to_owned(),
            ));
        }
        add_missing(&mut self.foreign, pkgs);
        self.calls.push(call);
        Ok(())
    }

    fn remove_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        let call = Call::RemoveForeign(pkgs.to_vec());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend(
                "injected remove_foreign failure".to_owned(),
            ));
        }
        remove_present(&mut self.foreign, pkgs);
        self.calls.push(call);
        Ok(())
    }
}

impl FileBackend for FakeBackend {
    fn read_file(&self, path: &str) -> Result<Option<String>, GelError> {
        Ok(self.files.get(path).cloned())
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<(), GelError> {
        let call = Call::WriteFile(path.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected write_file failure".to_owned()));
        }
        self.files.insert(path.to_owned(), content.to_owned());
        self.calls.push(call);
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<(), GelError> {
        let call = Call::RemoveFile(path.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected remove_file failure".to_owned()));
        }
        self.files.remove(path);
        self.calls.push(call);
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

    #[test]
    fn read_file_is_none_until_written_then_some() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::default();

        assert_eq!(backend.read_file("/etc/hostname").expect("read"), None);

        backend
            .write_file("/etc/hostname", "gelbox\n")
            .expect("write");

        assert_eq!(
            backend.read_file("/etc/hostname").expect("read"),
            Some("gelbox\n".to_owned())
        );
    }

    #[test]
    fn remove_file_deletes_content() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/hostname", "gelbox\n")]);

        backend.remove_file("/etc/hostname").expect("remove");

        assert_eq!(backend.read_file("/etc/hostname").expect("read"), None);
    }

    #[test]
    fn file_operations_are_recorded_in_order() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::default();

        backend.write_file("/a", "one").expect("write");
        backend.remove_file("/a").expect("remove");

        assert_eq!(
            backend.calls(),
            &[
                Call::WriteFile("/a".to_owned()),
                Call::RemoveFile("/a".to_owned()),
            ]
        );
    }

    #[test]
    fn seeded_file_reads_back() {
        use crate::backend::file::FileBackend;

        let backend = FakeBackend::with_files(&[("/etc/motd", "hello\n")]);

        assert_eq!(
            backend.read_file("/etc/motd").expect("read"),
            Some("hello\n".to_owned())
        );
    }

    #[test]
    fn injected_write_failure_is_one_shot() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::default();
        backend.set_fail_on(Call::WriteFile(String::new()));

        // first matching write fails and does not mutate state
        let first = backend.write_file("/etc/hostname", "gelbox\n");
        assert!(first.is_err());
        assert_eq!(backend.read_file("/etc/hostname").expect("read"), None);

        // the next matching write succeeds now that the failure is consumed
        backend
            .write_file("/etc/hostname", "gelbox\n")
            .expect("write");
        assert_eq!(
            backend.read_file("/etc/hostname").expect("read"),
            Some("gelbox\n".to_owned())
        );
    }

    #[test]
    fn injected_failure_is_one_shot() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        backend.set_fail_on(Call::InstallNative(Vec::new()));

        // first matching call fails and does not mutate state
        let first = backend.install_native(&["git".to_owned()]);
        assert!(first.is_err());
        assert!(
            !backend
                .query_explicit()
                .expect("query")
                .native
                .contains(&"git".to_owned())
        );

        // the next matching call succeeds now that the failure is consumed
        backend
            .install_native(&["git".to_owned()])
            .expect("install");
        assert!(
            backend
                .query_explicit()
                .expect("query")
                .native
                .contains(&"git".to_owned())
        );
    }
}
