use std::collections::BTreeSet;

use crate::state::DesiredState;

/// A deterministic set of package changes to reconcile current with desired
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Plan {
    pub native_install: Vec<String>,
    pub native_remove: Vec<String>,
    pub foreign_install: Vec<String>,
    pub foreign_remove: Vec<String>,
}

impl Plan {
    /// Compute the changes needed to move from `current` to `desired`
    ///
    /// For each origin, installs are packages in `desired` but not `current`,
    /// and removals are packages in `current` but not `desired`. Results are
    /// sorted and deduplicated so the plan is deterministic.
    #[must_use]
    pub fn compute(current: &DesiredState, desired: &DesiredState) -> Self {
        Self {
            native_install: difference(&desired.native, &current.native),
            native_remove: difference(&current.native, &desired.native),
            foreign_install: difference(&desired.foreign, &current.foreign),
            foreign_remove: difference(&current.foreign, &desired.foreign),
        }
    }

    /// Return true when there are no changes to apply
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.native_install.is_empty()
            && self.native_remove.is_empty()
            && self.foreign_install.is_empty()
            && self.foreign_remove.is_empty()
    }
}

/// Packages present in `from` but absent from `exclude`, sorted and deduplicated
fn difference(from: &[String], exclude: &[String]) -> Vec<String> {
    let exclude: BTreeSet<&String> = exclude.iter().collect();
    from.iter()
        .filter(|pkg| !exclude.contains(pkg))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DesiredState;

    #[test]
    fn computes_installs_and_removes_per_origin() {
        let current = DesiredState {
            native: vec!["git".to_owned(), "vim".to_owned()],
            foreign: vec![],
            files: vec![],
        };
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![],
        };

        let plan = Plan::compute(&current, &desired);

        assert_eq!(plan.native_install, vec!["ripgrep".to_owned()]);
        assert_eq!(plan.native_remove, vec!["vim".to_owned()]);
        assert_eq!(plan.foreign_install, vec!["yay".to_owned()]);
        assert!(plan.foreign_remove.is_empty());
    }

    #[test]
    fn identical_states_produce_empty_plan() {
        let state = DesiredState {
            native: vec!["git".to_owned(), "vim".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![],
        };

        let plan = Plan::compute(&state, &state);

        assert!(plan.is_empty());
    }

    #[test]
    fn results_are_sorted_and_deduplicated() {
        let current = DesiredState {
            native: vec!["vim".to_owned(), "git".to_owned()],
            foreign: vec![],
            files: vec![],
        };
        let desired = DesiredState {
            native: vec!["zsh".to_owned(), "bash".to_owned(), "bash".to_owned()],
            foreign: vec![],
            files: vec![],
        };

        let plan = Plan::compute(&current, &desired);

        assert_eq!(
            plan.native_install,
            vec!["bash".to_owned(), "zsh".to_owned()]
        );
        assert_eq!(plan.native_remove, vec!["git".to_owned(), "vim".to_owned()]);
    }
}
