use crate::{backend::PackageBackend, error::GelError, plan::Plan, state::DesiredState};

/// Options controlling how a desired state is applied
#[derive(Debug, Clone, Copy)]
pub struct ApplyOpts {
    /// When true, remove explicit packages not present in the desired state
    ///
    /// The default (false) is additive: only installs are performed, and
    /// packages absent from the desired state are left in place.
    pub prune: bool,
}

/// Reconcile a backend toward `desired`, returning the effective plan applied
///
/// Installs are always applied. Removals are only executed when `opts.prune`
/// is set. The returned [`Plan`] is the EFFECTIVE plan: it equals what was
/// actually executed, so in additive mode (`prune` off) its `native_remove`
/// and `foreign_remove` are empty. This makes the returned plan safe to
/// journal for rollback, since inverting it can only undo operations that
/// really happened. To preview what a prune would remove without executing it,
/// use [`Plan::compute`] directly.
///
/// # Errors
///
/// Returns [`GelError`] if any backend query or mutation fails.
pub fn apply(
    b: &mut impl PackageBackend,
    desired: &DesiredState,
    opts: ApplyOpts,
) -> Result<Plan, GelError> {
    let current = b.query_explicit()?;
    let mut plan = Plan::compute(&current, desired);
    if !plan.native_install.is_empty() {
        b.install_native(&plan.native_install)?;
    }
    if !plan.foreign_install.is_empty() {
        b.install_foreign(&plan.foreign_install)?;
    }
    if opts.prune {
        if !plan.native_remove.is_empty() {
            b.remove_native(&plan.native_remove)?;
        }
        if !plan.foreign_remove.is_empty() {
            b.remove_foreign(&plan.foreign_remove)?;
        }
    } else {
        // additive mode executes no removals, so the effective plan carries none
        plan.native_remove.clear();
        plan.foreign_remove.clear();
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{PackageBackend, fake::Call, fake::FakeBackend},
        state::DesiredState,
    };

    #[test]
    fn additive_default_installs_but_does_not_prune() {
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![],
        };

        let plan = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // ripgrep installed, vim still present because prune is off
        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"ripgrep".to_owned()));
        assert!(state.native.contains(&"vim".to_owned()));

        // the returned plan is the EFFECTIVE plan: removes are cleared because
        // prune is off, so what apply returns equals what it executed
        assert!(plan.native_remove.is_empty());
        assert!(plan.foreign_remove.is_empty());

        // no remove call was ever made
        assert_eq!(
            backend.calls(),
            &[Call::InstallNative(vec!["ripgrep".to_owned()])]
        );
    }

    #[test]
    fn prune_removes_extra_native_packages() {
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        let desired = DesiredState {
            native: vec!["git".to_owned()],
            foreign: vec![],
            files: vec![],
        };

        apply(&mut backend, &desired, ApplyOpts { prune: true }).expect("apply");

        let state = backend.query_explicit().expect("query");
        assert!(!state.native.contains(&"vim".to_owned()));
        assert_eq!(state.native, vec!["git".to_owned()]);

        assert_eq!(
            backend.calls(),
            &[Call::RemoveNative(vec!["vim".to_owned()])]
        );
    }

    #[test]
    fn rollback_of_effective_plan_does_not_resurrect_kept_packages() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        // additive apply: install ripgrep, keep vim (never removed)
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![],
        };
        let plan = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // journal the EFFECTIVE plan that apply returned
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan,
            snapshot: None,
        };
        write_entry(dir.path(), &entry).expect("write");

        // rollback only inverts installs that actually happened; vim is untouched
        let mut rollback_backend = FakeBackend::with_explicit(&["git", "vim", "ripgrep"], &[]);
        rollback_last(dir.path(), &mut rollback_backend).expect("rollback");

        // no install issued for the kept vim, only ripgrep removed
        assert_eq!(
            rollback_backend.calls(),
            &[Call::RemoveNative(vec!["ripgrep".to_owned()])]
        );
        let state = rollback_backend.query_explicit().expect("query");
        assert!(state.native.contains(&"vim".to_owned()));
        assert!(!state.native.contains(&"ripgrep".to_owned()));
    }

    #[test]
    fn apply_propagates_backend_failure_after_prior_calls() {
        // fail when foreign install is attempted, after native install succeeds
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        backend.set_fail_on(Call::InstallForeign(Vec::new()));
        let desired = DesiredState {
            native: vec!["git".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![],
        };

        let result = apply(&mut backend, &desired, ApplyOpts { prune: false });

        assert!(result.is_err());
        // the native install already ran and remains observable, and the
        // failed foreign attempt is recorded as intent before it errored
        assert_eq!(
            backend.calls(),
            &[
                Call::InstallNative(vec!["git".to_owned()]),
                Call::InstallForeign(vec!["yay".to_owned()]),
            ]
        );
        // the native package was actually installed, the foreign one was not
        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"git".to_owned()));
        assert!(!state.foreign.contains(&"yay".to_owned()));
    }

    #[test]
    fn foreign_path_installs_and_prunes_symmetrically() {
        let mut backend = FakeBackend::with_explicit(&[], &["old-aur", "keep-aur"]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec!["keep-aur".to_owned(), "new-aur".to_owned()],
            files: vec![],
        };

        apply(&mut backend, &desired, ApplyOpts { prune: true }).expect("apply");

        let state = backend.query_explicit().expect("query");
        assert!(state.foreign.contains(&"new-aur".to_owned()));
        assert!(state.foreign.contains(&"keep-aur".to_owned()));
        assert!(!state.foreign.contains(&"old-aur".to_owned()));

        assert_eq!(
            backend.calls(),
            &[
                Call::InstallForeign(vec!["new-aur".to_owned()]),
                Call::RemoveForeign(vec!["old-aur".to_owned()]),
            ]
        );
    }
}
