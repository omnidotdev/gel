use crate::{
    backend::{PackageBackend, file::FileBackend, service::ServiceBackend},
    error::GelError,
    journal::{FileBackup, ServiceBackup},
    plan::{Plan, plan_files, plan_services},
    state::DesiredState,
};

/// Options controlling how a desired state is applied
#[derive(Debug, Clone, Copy)]
pub struct ApplyOpts {
    /// When true, remove explicit packages not present in the desired state
    ///
    /// The default (false) is additive: only installs are performed, and
    /// packages absent from the desired state are left in place.
    pub prune: bool,
}

/// The result of an [`apply`]: the effective plan plus per-file rollback data
///
/// `apply` is the only place that reads a managed file's prior content in the
/// same pass it overwrites it, so it is the natural owner of the backups needed
/// to undo those writes. Returning them in a small struct (rather than exposing
/// a second helper the caller would run against the already-mutated backend)
/// keeps the read-before-write atomic and lets the journal-writing caller record
/// everything from one value.
#[derive(Debug)]
pub struct Applied {
    /// The effective plan that was executed, including the files written
    pub plan: Plan,
    /// Prior content of each file written, so the transaction can be rolled back
    pub file_backups: Vec<FileBackup>,
    /// Prior enabled-state of each unit toggled, so it can be rolled back
    pub service_backups: Vec<ServiceBackup>,
}

/// Reconcile a backend toward `desired`, returning what was applied
///
/// Installs are always applied. Removals are only executed when `opts.prune`
/// is set. The returned [`Applied::plan`] is the EFFECTIVE plan: it equals what
/// was actually executed, so in additive mode (`prune` off) its `native_remove`
/// and `foreign_remove` are empty. This makes the returned plan safe to journal
/// for rollback, since inverting it can only undo operations that really
/// happened. To preview what a prune would remove without executing it, use
/// [`Plan::compute`] directly.
///
/// After packages converge, managed files are written: for each file that is
/// absent or whose content differs, the prior content is read and recorded as a
/// [`FileBackup`] before the new content is written, so the transaction can be
/// rolled back to the exact prior bytes (or the file deleted if it was created).
/// The written files populate `plan.file_writes` so the effective plan reflects
/// them.
///
/// After files, declared services converge: only units named in
/// `desired.services` are considered (never a full-set prune), and for each unit
/// that is not already in its declared state its prior enabled-state is read and
/// recorded as a [`ServiceBackup`] before it is enabled or disabled, so the
/// transaction can restore it. The toggled units populate `plan.service_enable`
/// and `plan.service_disable` so the effective plan reflects them.
///
/// # Errors
///
/// Returns [`GelError`] if any backend query or mutation fails.
pub fn apply<B: PackageBackend + FileBackend + ServiceBackend>(
    b: &mut B,
    desired: &DesiredState,
    opts: ApplyOpts,
) -> Result<Applied, GelError> {
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

    // converge managed files after packages, capturing prior content per write
    let writes = plan_files(b, desired)?;
    let mut file_backups = Vec::with_capacity(writes.len());
    for file in &writes {
        // read the prior content BEFORE overwriting so rollback can restore it
        let prior = b.read_file(&file.path)?;
        b.write_file(&file.path, &file.content)?;
        file_backups.push(FileBackup {
            path: file.path.clone(),
            prior,
        });
    }
    plan.file_writes = writes;

    // converge declared services after files, capturing prior state per toggle.
    // plan_services only returns units that are not already in their declared
    // state, so every unit here genuinely changes and is worth backing up
    let (enable, disable) = plan_services(b, desired)?;
    let mut service_backups = Vec::with_capacity(enable.len() + disable.len());
    for unit in &enable {
        // read the prior enabled-state BEFORE toggling so rollback can restore it
        let prior_enabled = b.is_enabled(unit)?;
        b.enable(unit)?;
        service_backups.push(ServiceBackup {
            unit: unit.clone(),
            prior_enabled,
        });
    }
    for unit in &disable {
        let prior_enabled = b.is_enabled(unit)?;
        b.disable(unit)?;
        service_backups.push(ServiceBackup {
            unit: unit.clone(),
            prior_enabled,
        });
    }
    plan.service_enable = enable;
    plan.service_disable = disable;

    Ok(Applied {
        plan,
        file_backups,
        service_backups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{
            PackageBackend, fake::Call, fake::FakeBackend, file::FileBackend,
            service::ServiceBackend,
        },
        journal::{FileBackup, ServiceBackup},
        state::{DesiredState, ManagedFile, ServiceIntent},
    };

    fn desired_files(files: Vec<ManagedFile>) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files,
            services: ServiceIntent::default(),
        }
    }

    #[test]
    fn additive_apply_writes_new_file_and_backs_up_absent_prior() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the file now exists on the backend with the desired content
        assert_eq!(
            backend.read_file("/etc/hostname").expect("read"),
            Some("gelbox\n".to_owned())
        );
        // a new file records a None prior, so rollback knows to delete it
        assert_eq!(
            applied.file_backups,
            vec![FileBackup {
                path: "/etc/hostname".to_owned(),
                prior: None,
            }]
        );
        // the effective plan reflects what was written
        assert_eq!(applied.plan.file_writes, desired.files);
        // the write is observable in the call log
        assert!(
            backend
                .calls()
                .contains(&Call::WriteFile("/etc/hostname".to_owned()))
        );
    }

    #[test]
    fn apply_changing_existing_file_captures_prior_content() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        backend.set_file("/etc/hostname", "old\n");
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "new\n".to_owned(),
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the backend now holds the new content
        assert_eq!(
            backend.read_file("/etc/hostname").expect("read"),
            Some("new\n".to_owned())
        );
        // the backup captured the PRIOR content so rollback can restore it
        assert_eq!(
            applied.file_backups,
            vec![FileBackup {
                path: "/etc/hostname".to_owned(),
                prior: Some("old\n".to_owned()),
            }]
        );
    }

    #[test]
    fn apply_skips_file_whose_content_already_matches() {
        let mut backend = FakeBackend::with_files(&[("/etc/hostname", "gelbox\n")]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // an already-matching file is neither written nor backed up
        assert!(applied.plan.file_writes.is_empty());
        assert!(applied.file_backups.is_empty());
        assert!(
            !backend
                .calls()
                .iter()
                .any(|call| matches!(call, Call::WriteFile(_)))
        );
    }

    #[test]
    fn apply_enables_declared_disabled_unit_and_backs_up_prior() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                enable: vec!["sshd.service".to_owned()],
                disable: vec![],
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the unit is now enabled on the backend
        assert!(backend.is_enabled("sshd.service").expect("query"));
        // its prior disabled state is recorded so rollback can restore it
        assert_eq!(
            applied.service_backups,
            vec![ServiceBackup {
                unit: "sshd.service".to_owned(),
                prior_enabled: false,
            }]
        );
        // the effective plan reflects the enable
        assert_eq!(applied.plan.service_enable, vec!["sshd.service".to_owned()]);
        assert!(applied.plan.service_disable.is_empty());
        // the enable is observable in the call log
        assert!(
            backend
                .calls()
                .contains(&Call::EnableService("sshd.service".to_owned()))
        );
    }

    #[test]
    fn apply_disables_declared_enabled_unit_and_backs_up_prior() {
        // seed the unit enabled so the apply must disable it
        let mut backend = FakeBackend::with_enabled(&["bluetooth.service"]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                enable: vec![],
                disable: vec!["bluetooth.service".to_owned()],
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the unit is now disabled on the backend
        assert!(!backend.is_enabled("bluetooth.service").expect("query"));
        // its prior enabled state is recorded so rollback can restore it
        assert_eq!(
            applied.service_backups,
            vec![ServiceBackup {
                unit: "bluetooth.service".to_owned(),
                prior_enabled: true,
            }]
        );
        // the effective plan reflects the disable
        assert_eq!(
            applied.plan.service_disable,
            vec!["bluetooth.service".to_owned()]
        );
        assert!(applied.plan.service_enable.is_empty());
        assert!(
            backend
                .calls()
                .contains(&Call::DisableService("bluetooth.service".to_owned()))
        );
    }

    #[test]
    fn apply_skips_unit_already_in_desired_state() {
        // sshd already enabled (declared enable), bluetooth already disabled
        // (declared disable): neither should be touched
        let mut backend = FakeBackend::with_enabled(&["sshd.service"]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                enable: vec!["sshd.service".to_owned()],
                disable: vec!["bluetooth.service".to_owned()],
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // no unit changed, so no backups and an empty service plan
        assert!(applied.service_backups.is_empty());
        assert!(applied.plan.service_enable.is_empty());
        assert!(applied.plan.service_disable.is_empty());
        // and no enable/disable call was ever made
        assert!(
            !backend
                .calls()
                .iter()
                .any(|call| matches!(call, Call::EnableService(_) | Call::DisableService(_)))
        );
    }

    #[test]
    fn full_apply_then_rollback_restores_files_services_and_inverts_packages() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        // start: git + vim installed, one pre-existing managed file with old
        // content, bluetooth enabled (to be disabled), sshd disabled (to be
        // enabled)
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        backend.set_file("/etc/changed", "old\n");
        backend.enable("bluetooth.service").expect("seed enable");
        // the seeding enable must not leak into the transaction call log below
        let seeded_calls = backend.calls().len();
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![
                ManagedFile {
                    path: "/etc/new".to_owned(),
                    content: "created\n".to_owned(),
                },
                ManagedFile {
                    path: "/etc/changed".to_owned(),
                    content: "new\n".to_owned(),
                },
            ],
            services: ServiceIntent {
                enable: vec!["sshd.service".to_owned()],
                disable: vec!["bluetooth.service".to_owned()],
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the apply enabled sshd and disabled bluetooth, and recorded each prior
        assert!(backend.is_enabled("sshd.service").expect("query"));
        assert!(!backend.is_enabled("bluetooth.service").expect("query"));
        assert_eq!(
            applied.service_backups,
            vec![
                ServiceBackup {
                    unit: "sshd.service".to_owned(),
                    prior_enabled: false,
                },
                ServiceBackup {
                    unit: "bluetooth.service".to_owned(),
                    prior_enabled: true,
                },
            ]
        );
        // the transaction toggled exactly the two declared units
        let toggles = backend.calls()[seeded_calls..]
            .iter()
            .filter(|c| matches!(c, Call::EnableService(_) | Call::DisableService(_)))
            .count();
        assert_eq!(toggles, 2);

        // journal the effective plan together with the file and service backups
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
        };
        write_entry(dir.path(), &entry).expect("write");

        // roll back against the post-apply backend
        rollback_last(dir.path(), &mut backend).expect("rollback");

        // the file created by the transaction is deleted
        assert_eq!(backend.read_file("/etc/new").expect("read"), None);
        // the changed file is restored to its prior content
        assert_eq!(
            backend.read_file("/etc/changed").expect("read"),
            Some("old\n".to_owned())
        );
        // packages are inverted: the installed ripgrep is removed, git is kept
        let state = backend.query_explicit().expect("query");
        assert!(!state.native.contains(&"ripgrep".to_owned()));
        assert!(state.native.contains(&"git".to_owned()));
        assert!(state.native.contains(&"vim".to_owned()));
        // services are restored to their prior state: the unit the transaction
        // enabled is disabled again, the one it disabled is enabled again
        assert!(!backend.is_enabled("sshd.service").expect("query"));
        assert!(backend.is_enabled("bluetooth.service").expect("query"));
    }

    #[test]
    fn additive_default_installs_but_does_not_prune() {
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // ripgrep installed, vim still present because prune is off
        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"ripgrep".to_owned()));
        assert!(state.native.contains(&"vim".to_owned()));

        // the returned plan is the EFFECTIVE plan: removes are cleared because
        // prune is off, so what apply returns equals what it executed
        assert!(applied.plan.native_remove.is_empty());
        assert!(applied.plan.foreign_remove.is_empty());

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
            services: crate::state::ServiceIntent::default(),
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
            services: crate::state::ServiceIntent::default(),
        };
        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // journal the EFFECTIVE plan that apply returned
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
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
            services: crate::state::ServiceIntent::default(),
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
            services: crate::state::ServiceIntent::default(),
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
