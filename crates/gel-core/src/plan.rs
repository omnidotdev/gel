use std::collections::BTreeSet;

use crate::{
    backend::file::FileBackend,
    error::GelError,
    state::{DesiredState, ManagedFile},
};

/// A deterministic set of package and file changes to reconcile current with desired
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Plan {
    pub native_install: Vec<String>,
    pub native_remove: Vec<String>,
    pub foreign_install: Vec<String>,
    pub foreign_remove: Vec<String>,
    pub file_writes: Vec<ManagedFile>,
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
            // File writes require reading current content, which is impure, so
            // they are planned separately via `plan_files`
            file_writes: Vec::new(),
        }
    }

    /// Return true when there are no changes to apply
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.native_install.is_empty()
            && self.native_remove.is_empty()
            && self.foreign_install.is_empty()
            && self.foreign_remove.is_empty()
            && self.file_writes.is_empty()
    }
}

/// Compute the managed files that need a write to reach `desired`
///
/// A desired file is included when it is absent on the backend or its current
/// content differs from the desired content. Unlike [`Plan::compute`], this
/// reads current state through `backend`, so it is kept out of the pure planner.
/// This phase does not remove files, only writes. Results are ordered by path so
/// the plan is deterministic regardless of authoring order.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to read a file.
pub fn plan_files(
    backend: &impl FileBackend,
    desired: &DesiredState,
) -> Result<Vec<ManagedFile>, GelError> {
    let mut writes = Vec::new();
    for file in &desired.files {
        let current = backend.read_file(&file.path)?;
        if current.as_deref() != Some(file.content.as_str()) {
            writes.push(file.clone());
        }
    }
    writes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(writes)
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
    use crate::{
        backend::fake::FakeBackend,
        state::{DesiredState, ManagedFile},
    };

    fn desired_with_files(files: Vec<ManagedFile>) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files,
            services: crate::state::ServiceIntent::default(),
        }
    }

    #[test]
    fn absent_file_is_planned_for_write() {
        let backend = FakeBackend::default();
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn identical_content_is_not_planned() {
        let backend = FakeBackend::with_files(&[("/etc/hostname", "gelbox\n")]);
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert!(writes.is_empty());
    }

    #[test]
    fn changed_content_is_planned() {
        let backend = FakeBackend::with_files(&[("/etc/hostname", "old\n")]);
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "new\n".to_owned(),
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn writes_are_ordered_by_path() {
        let backend = FakeBackend::default();
        let desired = desired_with_files(vec![
            ManagedFile {
                path: "/z".to_owned(),
                content: "z".to_owned(),
            },
            ManagedFile {
                path: "/a".to_owned(),
                content: "a".to_owned(),
            },
            ManagedFile {
                path: "/m".to_owned(),
                content: "m".to_owned(),
            },
        ]);

        let writes = plan_files(&backend, &desired).expect("plan");

        let paths: Vec<&str> = writes.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["/a", "/m", "/z"]);
    }

    #[test]
    fn file_writes_count_toward_is_empty() {
        let plan = Plan {
            file_writes: vec![ManagedFile {
                path: "/a".to_owned(),
                content: "a".to_owned(),
            }],
            ..Plan::default()
        };

        assert!(!plan.is_empty());
    }

    #[test]
    fn computes_installs_and_removes_per_origin() {
        let current = DesiredState {
            native: vec!["git".to_owned(), "vim".to_owned()],
            foreign: vec![],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
        };
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
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
            services: crate::state::ServiceIntent::default(),
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
            services: crate::state::ServiceIntent::default(),
        };
        let desired = DesiredState {
            native: vec!["zsh".to_owned(), "bash".to_owned(), "bash".to_owned()],
            foreign: vec![],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
        };

        let plan = Plan::compute(&current, &desired);

        assert_eq!(
            plan.native_install,
            vec!["bash".to_owned(), "zsh".to_owned()]
        );
        assert_eq!(plan.native_remove, vec!["git".to_owned(), "vim".to_owned()]);
    }
}
