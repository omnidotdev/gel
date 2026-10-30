//! Pure builder for authoring a desired system configuration
//!
//! [`System`] accumulates the native (official-repo) and foreign (AUR) packages
//! a machine should have, plus any declaratively managed files, and lowers them
//! into a [`DesiredState`]. It is a pure data structure: no filesystem, process,
//! or clock access lives here, so a user config crate can depend on it and be
//! evaluated deterministically.

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{DesiredState, ManagedFile, ServiceIntent, SettingsIntent};

/// Accumulates the native and foreign packages a machine should have
///
/// Construct with [`System::new`], add packages with [`System::native`] and
/// [`System::foreign`] and managed files with [`System::file`] (all chainable
/// and order-independent), then lower into a [`DesiredState`] with
/// [`System::build`].
#[derive(Debug, Default, Clone)]
pub struct System {
    native: Vec<String>,
    foreign: Vec<String>,
    files: Vec<ManagedFile>,
    enable: Vec<String>,
    disable: Vec<String>,
    settings: SettingsIntent,
}

impl System {
    /// Start from an empty configuration
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add native (official-repo) packages, keeping earlier entries
    #[must_use]
    pub fn native<I, S>(mut self, pkgs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.native.extend(pkgs.into_iter().map(Into::into));
        self
    }

    /// Add foreign (AUR) packages, keeping earlier entries
    #[must_use]
    pub fn foreign<I, S>(mut self, pkgs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.foreign.extend(pkgs.into_iter().map(Into::into));
        self
    }

    /// Declare a managed file at `path` whose content should be `content`
    ///
    /// Chainable and order-independent. Declaring the same path more than once is
    /// last-wins: the final declaration's content is the one lowered into the
    /// [`DesiredState`] (see [`System::build`]).
    #[must_use]
    pub fn file(mut self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.files.push(ManagedFile {
            path: path.into(),
            content: content.into(),
            ..Default::default()
        });
        self
    }

    /// Declare a managed file at `path` with `content` and permission bits `mode`
    ///
    /// Like [`System::file`] but also manages the file's mode (for example
    /// `0o600` on a secret). Chainable and order-independent; declaring the same
    /// path more than once is last-wins for both content and mode (see
    /// [`System::build`]).
    #[must_use]
    pub fn file_mode(
        mut self,
        path: impl Into<String>,
        content: impl Into<String>,
        mode: u32,
    ) -> Self {
        self.files.push(ManagedFile {
            path: path.into(),
            content: content.into(),
            mode: Some(mode),
            ..Default::default()
        });
        self
    }

    /// Set the owning user of the most recently declared file
    ///
    /// Applies to the file added by the preceding [`System::file`] or
    /// [`System::file_mode`], so it reads naturally as
    /// `.file(path, content).owner("root")`. A no-op if no file has been declared
    /// yet, so a stray call can never panic.
    #[must_use]
    pub fn owner(mut self, owner: impl Into<String>) -> Self {
        if let Some(file) = self.files.last_mut() {
            file.owner = Some(owner.into());
        }
        self
    }

    /// Set the owning group of the most recently declared file
    ///
    /// Applies to the file added by the preceding [`System::file`] or
    /// [`System::file_mode`], mirroring [`System::owner`]. A no-op if no file has
    /// been declared yet.
    #[must_use]
    pub fn group(mut self, group: impl Into<String>) -> Self {
        if let Some(file) = self.files.last_mut() {
            file.group = Some(group.into());
        }
        self
    }

    /// Declare that `unit` should be enabled
    ///
    /// Chainable and order-independent. Repeats are collapsed by [`System::build`].
    /// A unit named in both [`System::enable`] and [`System::disable`] resolves to
    /// disable-only in the built state (see [`System::build`]).
    #[must_use]
    pub fn enable(mut self, unit: impl Into<String>) -> Self {
        self.enable.push(unit.into());
        self
    }

    /// Declare that `unit` should be disabled
    ///
    /// Chainable and order-independent. Repeats are collapsed by [`System::build`].
    /// Disable wins over a conflicting [`System::enable`] for the same unit.
    #[must_use]
    pub fn disable(mut self, unit: impl Into<String>) -> Self {
        self.disable.push(unit.into());
        self
    }

    /// Declare the desired hostname
    ///
    /// Chainable and last-call-wins: the final call sets the managed hostname.
    /// A hostname left unset is one gel never touches (see [`SettingsIntent`]).
    #[must_use]
    pub fn hostname(mut self, hostname: impl Into<String>) -> Self {
        self.settings.hostname = Some(hostname.into());
        self
    }

    /// Declare the desired system timezone
    ///
    /// Chainable and last-call-wins: the final call sets the managed timezone.
    /// A timezone left unset is one gel never touches (see [`SettingsIntent`]).
    #[must_use]
    pub fn timezone(mut self, timezone: impl Into<String>) -> Self {
        self.settings.timezone = Some(timezone.into());
        self
    }

    /// Declare the desired system locale
    ///
    /// Chainable and last-call-wins: the final call sets the managed locale.
    /// A locale left unset is one gel never touches (see [`SettingsIntent`]).
    #[must_use]
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.settings.locale = Some(locale.into());
        self
    }

    /// Lower the accumulated configuration into a [`DesiredState`]
    ///
    /// Each package origin is sorted and deduplicated so that authoring order and
    /// accidental repeats do not affect the result. This mirrors how the planner
    /// already normalizes a plan, so an imported state and an authored state
    /// compare equal when they name the same packages.
    ///
    /// Managed files are likewise sorted by path and deduplicated by path so the
    /// result is deterministic. Deduplication is last-wins: when a path is
    /// declared more than once, the content and mode of the final declaration are
    /// kept.
    ///
    /// Service intent is sorted and deduplicated per list. When a unit is named in
    /// both enable and disable, **disable wins**: it is dropped from the enable
    /// list so the built state never carries a unit in both lists. This mirrors
    /// the planner's disable-wins conflict rule, so an ambiguous declaration can
    /// never leave a unit enabled.
    ///
    /// System settings (hostname, timezone, locale) are carried through as
    /// declared. Each is last-call-wins per field, so only the final value for a
    /// field survives; a field never set stays `None`, meaning gel leaves it
    /// unmanaged.
    #[must_use]
    pub fn build(self) -> DesiredState {
        DesiredState {
            native: sorted_unique(self.native),
            foreign: sorted_unique(self.foreign),
            files: sorted_unique_files(self.files),
            services: build_services(self.enable, self.disable),
            settings: self.settings,
        }
    }
}

/// Lower accumulated enable/disable declarations into a [`ServiceIntent`]
///
/// Each list is sorted and deduplicated, and disable wins over a conflicting
/// enable: a unit present in both lists is removed from enable so the result
/// never names a unit in both.
fn build_services(enable: Vec<String>, disable: Vec<String>) -> ServiceIntent {
    let disable = sorted_unique(disable);
    let disable_set: BTreeSet<&String> = disable.iter().collect();
    let enable = sorted_unique(enable)
        .into_iter()
        .filter(|unit| !disable_set.contains(unit))
        .collect();
    ServiceIntent { enable, disable }
}

/// Sort a package list and drop duplicates
fn sorted_unique(mut pkgs: Vec<String>) -> Vec<String> {
    pkgs.sort();
    pkgs.dedup();
    pkgs
}

/// Sort managed files by path and deduplicate by path, keeping the last file
///
/// A [`BTreeMap`] keyed by path yields both properties at once: inserting an
/// already-present path overwrites the earlier declaration (last-wins), and
/// iteration is ordered by path (deterministic). The whole [`ManagedFile`] is
/// kept as the value, so the final declaration's content AND mode survive, rather
/// than only its content.
fn sorted_unique_files(files: Vec<ManagedFile>) -> Vec<ManagedFile> {
    let mut by_path: BTreeMap<String, ManagedFile> = BTreeMap::new();
    for file in files {
        by_path.insert(file.path.clone(), file);
    }
    by_path.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_expected_desired_state() {
        let desired = System::new()
            .native(["git", "ripgrep"])
            .foreign(["yay"])
            .build();

        assert_eq!(
            desired,
            DesiredState {
                native: vec!["git".to_owned(), "ripgrep".to_owned()],
                foreign: vec!["yay".to_owned()],
                files: vec![],
                services: crate::state::ServiceIntent::default(),
                settings: crate::state::SettingsIntent::default(),
            }
        );
    }

    #[test]
    fn build_sorts_and_deduplicates_so_order_does_not_matter() {
        let desired = System::new()
            .native(["vim", "git", "git"])
            .native(["bash"])
            .foreign(["yay", "paru", "yay"])
            .build();

        // sorted and deduplicated regardless of authoring order or repeats
        assert_eq!(
            desired.native,
            vec!["bash".to_owned(), "git".to_owned(), "vim".to_owned()]
        );
        assert_eq!(desired.foreign, vec!["paru".to_owned(), "yay".to_owned()]);
    }

    #[test]
    fn empty_system_builds_empty_state() {
        let desired = System::new().build();

        assert!(desired.native.is_empty());
        assert!(desired.foreign.is_empty());
    }

    #[test]
    fn accepts_both_str_and_string_items() {
        // native/foreign are generic over Into<String>, so &str and String mix
        let desired = System::new()
            .native(vec!["git".to_owned()])
            .foreign(["yay"])
            .build();

        assert_eq!(desired.native, vec!["git".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
    }

    #[test]
    fn file_declarations_are_accumulated_into_state() {
        let desired = System::new()
            .file("/etc/hostname", "gelbox\n")
            .file("/etc/motd", "hello\n")
            .build();

        assert_eq!(
            desired.files,
            vec![
                ManagedFile {
                    path: "/etc/hostname".to_owned(),
                    content: "gelbox\n".to_owned(),
                    ..Default::default()
                },
                ManagedFile {
                    path: "/etc/motd".to_owned(),
                    content: "hello\n".to_owned(),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn build_sorts_files_by_path_regardless_of_authoring_order() {
        let desired = System::new()
            .file("/z", "z")
            .file("/a", "a")
            .file("/m", "m")
            .build();

        let paths: Vec<&str> = desired.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["/a", "/m", "/z"]);
    }

    #[test]
    fn duplicate_paths_are_last_wins() {
        // declaring the same path twice keeps the final declaration's content
        let desired = System::new()
            .file("/etc/hostname", "first\n")
            .file("/etc/hostname", "second\n")
            .build();

        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/hostname".to_owned(),
                content: "second\n".to_owned(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn files_mix_with_packages() {
        // files and packages accumulate independently and both land in the state
        let desired = System::new()
            .native(["git"])
            .foreign(["yay"])
            .file("/etc/hostname", "gelbox\n")
            .build();

        assert_eq!(desired.native, vec!["git".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/hostname".to_owned(),
                content: "gelbox\n".to_owned(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn file_accepts_both_str_and_string() {
        // file is generic over Into<String> for both path and content
        let desired = System::new()
            .file("/etc/hostname".to_owned(), "gelbox\n")
            .file("/etc/motd", "hi\n".to_owned())
            .build();

        assert_eq!(desired.files.len(), 2);
    }

    #[test]
    fn file_mode_sets_the_declared_mode() {
        let desired = System::new()
            .file_mode("/etc/exports", "x\n", 0o644)
            .build();

        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/exports".to_owned(),
                content: "x\n".to_owned(),
                mode: Some(0o644),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn plain_file_leaves_mode_unmanaged() {
        // the plain file() builder declares no mode, leaving it unmanaged
        let desired = System::new().file("/etc/motd", "hi\n").build();

        assert_eq!(desired.files[0].mode, None);
    }

    #[test]
    fn duplicate_path_last_wins_preserves_mode() {
        // re-declaring a path keeps the final declaration's content AND mode, so a
        // later file_mode is not silently dropped by deduplication
        let desired = System::new()
            .file("/etc/exports", "first\n")
            .file_mode("/etc/exports", "second\n", 0o600)
            .build();

        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/exports".to_owned(),
                content: "second\n".to_owned(),
                mode: Some(0o600),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn owner_and_group_set_attributes_on_the_last_file() {
        let desired = System::new()
            .file("/etc/exports", "x\n")
            .owner("root")
            .group("nfs")
            .build();

        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/exports".to_owned(),
                content: "x\n".to_owned(),
                owner: Some("root".to_owned()),
                group: Some("nfs".to_owned()),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn owner_and_group_compose_with_file_mode() {
        let desired = System::new()
            .file_mode("/etc/exports", "x\n", 0o600)
            .owner("root")
            .group("nfs")
            .build();

        assert_eq!(
            desired.files,
            vec![ManagedFile {
                path: "/etc/exports".to_owned(),
                content: "x\n".to_owned(),
                mode: Some(0o600),
                owner: Some("root".to_owned()),
                group: Some("nfs".to_owned()),
            }]
        );
    }

    #[test]
    fn owner_with_no_declared_file_is_a_noop() {
        // calling owner before any file is declared must not panic and adds nothing
        let desired = System::new().owner("root").build();

        assert!(desired.files.is_empty());
    }

    #[test]
    fn enable_and_disable_declarations_are_accumulated() {
        let desired = System::new()
            .enable("sshd.service")
            .disable("bluetooth.service")
            .build();

        assert_eq!(desired.services.enable, vec!["sshd.service".to_owned()]);
        assert_eq!(
            desired.services.disable,
            vec!["bluetooth.service".to_owned()]
        );
    }

    #[test]
    fn build_sorts_and_deduplicates_each_service_list() {
        // authoring order and accidental repeats must not affect the result
        let desired = System::new()
            .enable("c.service")
            .enable("a.service")
            .enable("a.service")
            .disable("d.service")
            .disable("b.service")
            .disable("b.service")
            .build();

        assert_eq!(
            desired.services.enable,
            vec!["a.service".to_owned(), "c.service".to_owned()]
        );
        assert_eq!(
            desired.services.disable,
            vec!["b.service".to_owned(), "d.service".to_owned()]
        );
    }

    #[test]
    fn build_resolves_enable_disable_conflict_disable_wins() {
        // a unit both enabled and disabled must end up only in disable, never
        // in both lists, matching the planner's disable-wins rule
        let desired = System::new()
            .enable("conflict.service")
            .disable("conflict.service")
            .build();

        assert!(desired.services.enable.is_empty());
        assert_eq!(
            desired.services.disable,
            vec!["conflict.service".to_owned()]
        );
    }

    #[test]
    fn services_mix_with_packages_and_files() {
        // services accumulate independently and land in the state alongside the
        // rest of the configuration
        let desired = System::new()
            .native(["git"])
            .foreign(["yay"])
            .file("/etc/hostname", "gelbox\n")
            .enable("sshd.service")
            .disable("bluetooth.service")
            .build();

        assert_eq!(desired.native, vec!["git".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
        assert_eq!(desired.files.len(), 1);
        assert_eq!(desired.services.enable, vec!["sshd.service".to_owned()]);
        assert_eq!(
            desired.services.disable,
            vec!["bluetooth.service".to_owned()]
        );
    }

    #[test]
    fn enable_and_disable_accept_both_str_and_string() {
        // enable/disable are generic over Into<String>, so &str and String mix
        let desired = System::new()
            .enable("sshd.service".to_owned())
            .disable("bluetooth.service")
            .build();

        assert_eq!(desired.services.enable, vec!["sshd.service".to_owned()]);
        assert_eq!(
            desired.services.disable,
            vec!["bluetooth.service".to_owned()]
        );
    }

    #[test]
    fn each_setting_setter_sets_its_field() {
        let desired = System::new()
            .hostname("gelbox")
            .timezone("Etc/UTC")
            .locale("en_US.UTF-8")
            .build();

        assert_eq!(
            desired.settings,
            SettingsIntent {
                hostname: Some("gelbox".to_owned()),
                timezone: Some("Etc/UTC".to_owned()),
                locale: Some("en_US.UTF-8".to_owned()),
            }
        );
    }

    #[test]
    fn unset_settings_stay_none() {
        // only hostname is declared; the others must remain unmanaged
        let desired = System::new().hostname("gelbox").build();

        assert_eq!(desired.settings.hostname, Some("gelbox".to_owned()));
        assert!(desired.settings.timezone.is_none());
        assert!(desired.settings.locale.is_none());
    }

    #[test]
    fn no_setting_setters_leaves_all_none() {
        let desired = System::new().native(["git"]).build();

        assert_eq!(desired.settings, SettingsIntent::default());
    }

    #[test]
    fn setting_setters_are_last_call_wins_per_field() {
        // the final call for each field is the one that survives, independently
        let desired = System::new()
            .hostname("first")
            .hostname("second")
            .timezone("Etc/UTC")
            .timezone("America/New_York")
            .build();

        assert_eq!(desired.settings.hostname, Some("second".to_owned()));
        assert_eq!(
            desired.settings.timezone,
            Some("America/New_York".to_owned())
        );
        // locale was never set, so it stays unmanaged
        assert!(desired.settings.locale.is_none());
    }

    #[test]
    fn setting_setters_accept_both_str_and_string() {
        // each setter is generic over Into<String>, so &str and String mix
        let desired = System::new()
            .hostname("gelbox".to_owned())
            .timezone("Etc/UTC")
            .locale("en_US.UTF-8".to_owned())
            .build();

        assert_eq!(desired.settings.hostname, Some("gelbox".to_owned()));
        assert_eq!(desired.settings.timezone, Some("Etc/UTC".to_owned()));
        assert_eq!(desired.settings.locale, Some("en_US.UTF-8".to_owned()));
    }

    #[test]
    fn settings_mix_with_packages_files_and_services() {
        // settings accumulate independently and land in the state alongside the
        // rest of the configuration
        let desired = System::new()
            .native(["git"])
            .foreign(["yay"])
            .file("/etc/motd", "hi\n")
            .enable("sshd.service")
            .disable("bluetooth.service")
            .hostname("gelbox")
            .timezone("Etc/UTC")
            .build();

        assert_eq!(desired.native, vec!["git".to_owned()]);
        assert_eq!(desired.foreign, vec!["yay".to_owned()]);
        assert_eq!(desired.files.len(), 1);
        assert_eq!(desired.services.enable, vec!["sshd.service".to_owned()]);
        assert_eq!(
            desired.services.disable,
            vec!["bluetooth.service".to_owned()]
        );
        assert_eq!(desired.settings.hostname, Some("gelbox".to_owned()));
        assert_eq!(desired.settings.timezone, Some("Etc/UTC".to_owned()));
        assert!(desired.settings.locale.is_none());
    }
}
