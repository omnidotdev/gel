use std::collections::HashMap;

use crate::{
    backend::{
        PackageBackend, file::FileBackend, service::ServiceBackend, settings::SettingsBackend,
    },
    error::GelError,
    state::{SettingKey, SystemState},
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
    /// `set_mode` was called for this path
    SetFileMode(String),
    /// `set_owner` was called for this path
    SetFileOwner(String),
    /// `set_group` was called for this path
    SetFileGroup(String),
    /// `enable` was called for this unit
    EnableService(String),
    /// `disable` was called for this unit
    DisableService(String),
    /// `set` was called for this setting with this value
    SetSetting(SettingKey, String),
}

/// An in-memory backend for tests, implementing all four backend traits
///
/// It implements [`PackageBackend`], [`FileBackend`](crate::backend::file::FileBackend),
/// [`ServiceBackend`], and [`SettingsBackend`], holding native and foreign
/// package sets, a map of managed files, a map of unit enabled-states, and a map
/// of setting values. It mutates them on install/remove, write/remove,
/// enable/disable, and set, recording an ordered log of every mutating call so
/// tests can assert both resulting state and the exact operations performed.
#[derive(Debug, Default, Clone)]
pub struct FakeBackend {
    native: Vec<String>,
    foreign: Vec<String>,
    files: HashMap<String, String>,
    file_modes: HashMap<String, u32>,
    file_owners: HashMap<String, String>,
    file_groups: HashMap<String, String>,
    services: HashMap<String, bool>,
    settings: HashMap<SettingKey, String>,
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

    /// Construct a backend seeded with the given units marked enabled
    #[must_use]
    pub fn with_enabled(units: &[&str]) -> Self {
        Self {
            services: units.iter().map(|u| ((*u).to_owned(), true)).collect(),
            ..Self::default()
        }
    }

    /// Construct a backend seeded with the given settings as `(key, value)` pairs
    #[must_use]
    pub fn with_settings(settings: &[(SettingKey, &str)]) -> Self {
        Self {
            settings: settings
                .iter()
                .map(|(key, value)| (*key, (*value).to_owned()))
                .collect(),
            ..Self::default()
        }
    }

    /// Seed or overwrite a single file's content without recording a call
    pub fn set_file(&mut self, path: &str, content: &str) {
        self.files.insert(path.to_owned(), content.to_owned());
    }

    /// Seed or overwrite a single file's mode without recording a call
    pub fn set_file_mode(&mut self, path: &str, mode: u32) {
        self.file_modes.insert(path.to_owned(), mode);
    }

    /// Seed or overwrite a single file's owner without recording a call
    pub fn set_file_owner(&mut self, path: &str, owner: &str) {
        self.file_owners.insert(path.to_owned(), owner.to_owned());
    }

    /// Seed or overwrite a single file's group without recording a call
    pub fn set_file_group(&mut self, path: &str, group: &str) {
        self.file_groups.insert(path.to_owned(), group.to_owned());
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
            settings: crate::state::SettingsIntent::default(),
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
        // a removed file carries no mode or ownership, so drop any tracked ones
        self.file_modes.remove(path);
        self.file_owners.remove(path);
        self.file_groups.remove(path);
        self.calls.push(call);
        Ok(())
    }

    fn mode(&self, path: &str) -> Result<Option<u32>, GelError> {
        // an absent file has no mode; a present file reports a stored mode only
        // when one was set, so a file whose mode gel never managed reads back as
        // None and a declared mode will plan a change
        if !self.files.contains_key(path) {
            return Ok(None);
        }
        Ok(self.file_modes.get(path).copied())
    }

    fn set_mode(&mut self, path: &str, mode: u32) -> Result<(), GelError> {
        let call = Call::SetFileMode(path.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected set_mode failure".to_owned()));
        }
        self.file_modes.insert(path.to_owned(), mode);
        self.calls.push(call);
        Ok(())
    }

    fn owner(&self, path: &str) -> Result<Option<String>, GelError> {
        // an absent file has no owner; a present file reports a stored owner only
        // when one was set, mirroring mode's unmanaged-is-None semantics
        if !self.files.contains_key(path) {
            return Ok(None);
        }
        Ok(self.file_owners.get(path).cloned())
    }

    fn group(&self, path: &str) -> Result<Option<String>, GelError> {
        if !self.files.contains_key(path) {
            return Ok(None);
        }
        Ok(self.file_groups.get(path).cloned())
    }

    fn set_owner(&mut self, path: &str, owner: &str) -> Result<(), GelError> {
        let call = Call::SetFileOwner(path.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected set_owner failure".to_owned()));
        }
        self.file_owners.insert(path.to_owned(), owner.to_owned());
        self.calls.push(call);
        Ok(())
    }

    fn set_group(&mut self, path: &str, group: &str) -> Result<(), GelError> {
        let call = Call::SetFileGroup(path.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected set_group failure".to_owned()));
        }
        self.file_groups.insert(path.to_owned(), group.to_owned());
        self.calls.push(call);
        Ok(())
    }
}

impl ServiceBackend for FakeBackend {
    fn is_enabled(&self, unit: &str) -> Result<bool, GelError> {
        // An unknown unit is treated as not enabled
        Ok(self.services.get(unit).copied().unwrap_or(false))
    }

    fn enable(&mut self, unit: &str) -> Result<(), GelError> {
        let call = Call::EnableService(unit.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected enable failure".to_owned()));
        }
        self.services.insert(unit.to_owned(), true);
        self.calls.push(call);
        Ok(())
    }

    fn disable(&mut self, unit: &str) -> Result<(), GelError> {
        let call = Call::DisableService(unit.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected disable failure".to_owned()));
        }
        self.services.insert(unit.to_owned(), false);
        self.calls.push(call);
        Ok(())
    }
}

impl SettingsBackend for FakeBackend {
    fn get(&self, key: SettingKey) -> Result<Option<String>, GelError> {
        Ok(self.settings.get(&key).cloned())
    }

    fn set(&mut self, key: SettingKey, value: &str) -> Result<(), GelError> {
        let call = Call::SetSetting(key, value.to_owned());
        if self.take_failure(&call) {
            self.calls.push(call);
            return Err(GelError::Backend("injected set failure".to_owned()));
        }
        self.settings.insert(key, value.to_owned());
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
    fn set_mode_records_call_and_mode_reads_back() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);

        backend.set_mode("/etc/exports", 0o644).expect("set_mode");

        assert_eq!(backend.mode("/etc/exports").expect("mode"), Some(0o644));
        assert_eq!(
            backend.calls(),
            &[Call::SetFileMode("/etc/exports".to_owned())]
        );
    }

    #[test]
    fn mode_is_none_for_absent_file() {
        use crate::backend::file::FileBackend;

        let backend = FakeBackend::default();

        assert_eq!(backend.mode("/etc/exports").expect("mode"), None);
    }

    #[test]
    fn mode_is_none_for_present_file_without_a_set_mode() {
        use crate::backend::file::FileBackend;

        // a file whose content is known but whose mode was never set reads back as
        // an unmanaged (None) mode, so a declared mode plans a change
        let backend = FakeBackend::with_files(&[("/etc/motd", "hi\n")]);

        assert_eq!(backend.mode("/etc/motd").expect("mode"), None);
    }

    #[test]
    fn remove_file_clears_mode() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_mode("/etc/exports", 0o600).expect("set_mode");

        backend.remove_file("/etc/exports").expect("remove");

        assert_eq!(backend.mode("/etc/exports").expect("mode"), None);
    }

    #[test]
    fn injected_set_mode_failure_is_one_shot() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_fail_on(Call::SetFileMode(String::new()));

        // first matching set_mode fails and does not mutate state
        let first = backend.set_mode("/etc/exports", 0o644);
        assert!(first.is_err());
        assert_eq!(backend.mode("/etc/exports").expect("mode"), None);

        // the next matching set_mode succeeds now that the failure is consumed
        backend.set_mode("/etc/exports", 0o644).expect("set_mode");
        assert_eq!(backend.mode("/etc/exports").expect("mode"), Some(0o644));
    }

    #[test]
    fn set_owner_and_group_record_calls_and_read_back() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);

        backend
            .set_owner("/etc/exports", "root")
            .expect("set_owner");
        backend.set_group("/etc/exports", "nfs").expect("set_group");

        assert_eq!(
            backend.owner("/etc/exports").expect("owner"),
            Some("root".to_owned())
        );
        assert_eq!(
            backend.group("/etc/exports").expect("group"),
            Some("nfs".to_owned())
        );
        assert_eq!(
            backend.calls(),
            &[
                Call::SetFileOwner("/etc/exports".to_owned()),
                Call::SetFileGroup("/etc/exports".to_owned()),
            ]
        );
    }

    #[test]
    fn owner_and_group_are_none_for_absent_file() {
        use crate::backend::file::FileBackend;

        let backend = FakeBackend::default();

        assert_eq!(backend.owner("/etc/exports").expect("owner"), None);
        assert_eq!(backend.group("/etc/exports").expect("group"), None);
    }

    #[test]
    fn remove_file_clears_owner_and_group() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_owner("/etc/exports", "root");
        backend.set_file_group("/etc/exports", "nfs");

        backend.remove_file("/etc/exports").expect("remove");

        assert_eq!(backend.owner("/etc/exports").expect("owner"), None);
        assert_eq!(backend.group("/etc/exports").expect("group"), None);
    }

    #[test]
    fn injected_set_owner_failure_is_one_shot() {
        use crate::backend::file::FileBackend;

        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_fail_on(Call::SetFileOwner(String::new()));

        // first matching set_owner fails and does not mutate state
        let first = backend.set_owner("/etc/exports", "root");
        assert!(first.is_err());
        assert_eq!(backend.owner("/etc/exports").expect("owner"), None);

        // the next matching set_owner succeeds now that the failure is consumed
        backend
            .set_owner("/etc/exports", "root")
            .expect("set_owner");
        assert_eq!(
            backend.owner("/etc/exports").expect("owner"),
            Some("root".to_owned())
        );
    }

    #[test]
    fn enable_flips_state_and_records_call() {
        use crate::backend::service::ServiceBackend;

        let mut backend = FakeBackend::default();

        backend.enable("sshd.service").expect("enable");

        assert!(backend.is_enabled("sshd.service").expect("query"));
        assert_eq!(
            backend.calls(),
            &[Call::EnableService("sshd.service".to_owned())]
        );
    }

    #[test]
    fn disable_flips_state_and_records_call() {
        use crate::backend::service::ServiceBackend;

        let mut backend = FakeBackend::with_enabled(&["bluetooth.service"]);

        backend.disable("bluetooth.service").expect("disable");

        assert!(!backend.is_enabled("bluetooth.service").expect("query"));
        assert_eq!(
            backend.calls(),
            &[Call::DisableService("bluetooth.service".to_owned())]
        );
    }

    #[test]
    fn unknown_unit_is_not_enabled() {
        use crate::backend::service::ServiceBackend;

        let backend = FakeBackend::default();

        assert!(!backend.is_enabled("unknown.service").expect("query"));
    }

    #[test]
    fn seeded_units_read_back_as_enabled() {
        use crate::backend::service::ServiceBackend;

        let backend = FakeBackend::with_enabled(&["sshd.service", "docker.service"]);

        assert!(backend.is_enabled("sshd.service").expect("query"));
        assert!(backend.is_enabled("docker.service").expect("query"));
    }

    #[test]
    fn injected_enable_failure_is_one_shot() {
        use crate::backend::service::ServiceBackend;

        let mut backend = FakeBackend::default();
        backend.set_fail_on(Call::EnableService(String::new()));

        // first matching enable fails and does not mutate state
        let first = backend.enable("sshd.service");
        assert!(first.is_err());
        assert!(!backend.is_enabled("sshd.service").expect("query"));

        // the next matching enable succeeds now that the failure is consumed
        backend.enable("sshd.service").expect("enable");
        assert!(backend.is_enabled("sshd.service").expect("query"));
    }

    #[test]
    fn set_setting_stores_value_and_records_call() {
        use crate::{backend::settings::SettingsBackend, state::SettingKey};

        let mut backend = FakeBackend::default();

        backend.set(SettingKey::Hostname, "gelbox").expect("set");

        assert_eq!(
            backend.get(SettingKey::Hostname).expect("get"),
            Some("gelbox".to_owned())
        );
        assert_eq!(
            backend.calls(),
            &[Call::SetSetting(SettingKey::Hostname, "gelbox".to_owned())]
        );
    }

    #[test]
    fn get_setting_is_none_until_set_then_some() {
        use crate::{backend::settings::SettingsBackend, state::SettingKey};

        let mut backend = FakeBackend::default();

        assert_eq!(backend.get(SettingKey::Timezone).expect("get"), None);

        backend.set(SettingKey::Timezone, "UTC").expect("set");

        assert_eq!(
            backend.get(SettingKey::Timezone).expect("get"),
            Some("UTC".to_owned())
        );
    }

    #[test]
    fn seeded_setting_reads_back() {
        use crate::{backend::settings::SettingsBackend, state::SettingKey};

        let backend = FakeBackend::with_settings(&[(SettingKey::Locale, "en_US.UTF-8")]);

        assert_eq!(
            backend.get(SettingKey::Locale).expect("get"),
            Some("en_US.UTF-8".to_owned())
        );
    }

    #[test]
    fn injected_set_setting_failure_is_one_shot() {
        use crate::{backend::settings::SettingsBackend, state::SettingKey};

        let mut backend = FakeBackend::default();
        backend.set_fail_on(Call::SetSetting(SettingKey::Hostname, String::new()));

        // first matching set fails and does not mutate state
        let first = backend.set(SettingKey::Hostname, "gelbox");
        assert!(first.is_err());
        assert_eq!(backend.get(SettingKey::Hostname).expect("get"), None);

        // the next matching set succeeds now that the failure is consumed
        backend.set(SettingKey::Hostname, "gelbox").expect("set");
        assert_eq!(
            backend.get(SettingKey::Hostname).expect("get"),
            Some("gelbox".to_owned())
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
