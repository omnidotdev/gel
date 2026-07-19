//! Pure builder for authoring a desired system configuration
//!
//! [`System`] accumulates the native (official-repo) and foreign (AUR) packages
//! a machine should have and lowers them into a [`DesiredState`]. It is a pure
//! data structure: no filesystem, process, or clock access lives here, so a user
//! config crate can depend on it and be evaluated deterministically.

use crate::state::DesiredState;

/// Accumulates the native and foreign packages a machine should have
///
/// Construct with [`System::new`], add packages with [`System::native`] and
/// [`System::foreign`] (both chainable and order-independent), then lower into a
/// [`DesiredState`] with [`System::build`].
#[derive(Debug, Default, Clone)]
pub struct System {
    native: Vec<String>,
    foreign: Vec<String>,
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

    /// Lower the accumulated configuration into a [`DesiredState`]
    ///
    /// Each origin is sorted and deduplicated so that authoring order and
    /// accidental repeats do not affect the result. This mirrors how the planner
    /// already normalizes a plan, so an imported state and an authored state
    /// compare equal when they name the same packages.
    #[must_use]
    pub fn build(self) -> DesiredState {
        DesiredState {
            native: sorted_unique(self.native),
            foreign: sorted_unique(self.foreign),
        }
    }
}

/// Sort a package list and drop duplicates
fn sorted_unique(mut pkgs: Vec<String>) -> Vec<String> {
    pkgs.sort();
    pkgs.dedup();
    pkgs
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
}
