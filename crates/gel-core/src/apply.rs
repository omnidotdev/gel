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

/// Reconcile a backend toward `desired`, returning the plan that was computed
///
/// Installs are always applied. Removals are only executed when `opts.prune`
/// is set; otherwise the returned plan still reports them so callers can see
/// what a prune would do. The returned [`Plan`] always reflects the full diff,
/// independent of whether removals were executed.
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
    let plan = Plan::compute(&current, desired);
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
        };

        let plan = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // ripgrep installed, vim still present because prune is off
        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"ripgrep".to_owned()));
        assert!(state.native.contains(&"vim".to_owned()));

        // the plan still reports vim as a removal even though it was not executed
        assert_eq!(plan.native_remove, vec!["vim".to_owned()]);

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
    fn foreign_path_installs_and_prunes_symmetrically() {
        let mut backend = FakeBackend::with_explicit(&[], &["old-aur", "keep-aur"]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec!["keep-aur".to_owned(), "new-aur".to_owned()],
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
