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
        });
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

    /// Lower the accumulated configuration into a [`DesiredState`]
    ///
    /// Each package origin is sorted and deduplicated so that authoring order and
    /// accidental repeats do not affect the result. This mirrors how the planner
    /// already normalizes a plan, so an imported state and an authored state
    /// compare equal when they name the same packages.
    ///
    /// Managed files are likewise sorted by path and deduplicated by path so the
    /// result is deterministic. Deduplication is last-wins: when a path is
    /// declared more than once, the content of the final declaration is kept.
    ///
    /// Service intent is sorted and deduplicated per list. When a unit is named in
    /// both enable and disable, **disable wins**: it is dropped from the enable
    /// list so the built state never carries a unit in both lists. This mirrors
    /// the planner's disable-wins conflict rule, so an ambiguous declaration can
    /// never leave a unit enabled.
    #[must_use]
    pub fn build(self) -> DesiredState {
        DesiredState {
            native: sorted_unique(self.native),
            foreign: sorted_unique(self.foreign),
            files: sorted_unique_files(self.files),
            services: build_services(self.enable, self.disable),
            settings: SettingsIntent::default(),
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

/// Sort managed files by path and deduplicate by path, keeping the last content
///
/// A [`BTreeMap`] keyed by path yields both properties at once: inserting an
/// already-present path overwrites the earlier content (last-wins), and
/// iteration is ordered by path (deterministic).
fn sorted_unique_files(files: Vec<ManagedFile>) -> Vec<ManagedFile> {
    let mut by_path: BTreeMap<String, String> = BTreeMap::new();
    for file in files {
        by_path.insert(file.path, file.content);
    }
    by_path
        .into_iter()
        .map(|(path, content)| ManagedFile { path, content })
        .collect()
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
                },
                ManagedFile {
                    path: "/etc/motd".to_owned(),
                    content: "hello\n".to_owned(),
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
}
