use crate::{
    backend::{
        PackageBackend, file::FileBackend, service::ServiceBackend, settings::SettingsBackend,
    },
    error::GelError,
    journal::{FileBackup, RuntimeBackup, ServiceBackup, SettingBackup, SysctlBackup},
    plan::{Plan, plan_files, plan_runtime, plan_services, plan_settings, plan_sysctl},
    state::{DesiredState, ManagedFile},
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
    /// Prior value of each setting changed, so it can be rolled back
    pub setting_backups: Vec<SettingBackup>,
    /// Prior active-state of each unit started, so it can be rolled back
    pub runtime_backups: Vec<RuntimeBackup>,
    /// Prior value of each sysctl parameter changed, so it can be rolled back
    pub sysctl_backups: Vec<SysctlBackup>,
}

/// Reconcile managed files (content, mode, owner, group) toward `desired`
///
/// Returns the files written (the effective file plan) and a [`FileBackup`] per
/// file, capturing prior content and metadata before any change so the whole file
/// reconciliation can be rolled back. Content is written only when it differs; each
/// of mode/owner/group is set only when declared and differing from the current.
///
/// # Errors
///
/// Returns [`GelError`] if any backend read or write fails.
fn converge_files<B: FileBackend>(
    b: &mut B,
    desired: &DesiredState,
) -> Result<(Vec<ManagedFile>, Vec<FileBackup>), GelError> {
    let writes = plan_files(b, desired)?;
    let mut file_backups = Vec::with_capacity(writes.len());
    for file in &writes {
        // read the prior content, mode, owner, and group BEFORE changing them so
        // rollback can restore each to exactly what it was
        let prior = b.read_file(&file.path)?;
        let prior_mode = b.mode(&file.path)?;
        let prior_owner = b.owner(&file.path)?;
        let prior_group = b.group(&file.path)?;
        // write content only when it actually differs, so a metadata-only change
        // does not rewrite identical bytes or emit a spurious write
        if prior.as_deref() != Some(file.content.as_str()) {
            b.write_file(&file.path, &file.content)?;
        }
        // set each managed facet only when it is declared (Some) and differs from
        // the current value; a facet left None is unmanaged and never touched
        if let Some(mode) = file.mode {
            if prior_mode != Some(mode) {
                b.set_mode(&file.path, mode)?;
            }
        }
        if let Some(owner) = &file.owner {
            if prior_owner.as_deref() != Some(owner.as_str()) {
                b.set_owner(&file.path, owner)?;
            }
        }
        if let Some(group) = &file.group {
            if prior_group.as_deref() != Some(group.as_str()) {
                b.set_group(&file.path, group)?;
            }
        }
        file_backups.push(FileBackup {
            path: file.path.clone(),
            prior,
            prior_mode,
            prior_owner,
            prior_group,
        });
    }
    Ok((writes, file_backups))
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
/// After packages converge, managed files are reconciled: for each file whose
/// content, mode, owner, or group differs, the prior content and metadata are read
/// and recorded as a [`FileBackup`] before the change, so the transaction can be
/// rolled back to the exact prior bytes and permission/ownership (or the file
/// deleted if it was created). Content is rewritten only when it differs, and each
/// of mode/owner/group is set only when declared and differing. The affected files
/// populate `plan.file_writes` so the effective plan reflects them.
///
/// After files, declared services converge: only units named in
/// `desired.services` are considered (never a full-set prune), and for each unit
/// that is not already in its declared state its prior enabled-state is read and
/// recorded as a [`ServiceBackup`] before it is enabled or disabled, so the
/// transaction can restore it. The toggled units populate `plan.service_enable`
/// and `plan.service_disable` so the effective plan reflects them.
///
/// After enablement, runtime state converges: each declared-start unit not already
/// active is started and each declared-stop unit that is active is stopped, with
/// each toggled unit's prior active-state recorded as a [`RuntimeBackup`] so
/// rollback can undo the toggle. The toggles populate
/// `plan.service_start`/`service_stop`.
///
/// After services, declared settings converge: only settings named in
/// `desired.settings` are considered, and for each setting whose current value
/// differs from the desired value its prior value is read and recorded as a
/// [`SettingBackup`] before it is set, so the transaction can restore it. The
/// changed settings populate `plan.setting_changes` so the effective plan
/// reflects them.
///
/// Finally, declared sysctl parameters converge the same way: each param whose
/// current value differs is set, its prior value recorded as a [`SysctlBackup`],
/// and the changes populate `plan.sysctl_changes`.
///
/// # Errors
///
/// Returns [`GelError`] if any backend query or mutation fails.
pub fn apply<
    B: PackageBackend
        + FileBackend
        + ServiceBackend
        + SettingsBackend
        + crate::backend::sysctl::SysctlBackend,
>(
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

    // converge managed files after packages (content, mode, owner, group), with a
    // FileBackup per change so the whole file reconciliation can be rolled back
    let (writes, file_backups) = converge_files(b, desired)?;
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

    // converge runtime (active) state after enablement: start each declared-start
    // unit that is not already active and stop each declared-stop unit that is,
    // capturing each unit's prior active-state so rollback can undo the toggle
    let (start, stop) = plan_runtime(b, desired)?;
    let mut runtime_backups = Vec::with_capacity(start.len() + stop.len());
    for unit in &start {
        let prior_active = b.is_active(unit)?;
        b.start(unit)?;
        runtime_backups.push(RuntimeBackup {
            unit: unit.clone(),
            prior_active,
        });
    }
    for unit in &stop {
        let prior_active = b.is_active(unit)?;
        b.stop(unit)?;
        runtime_backups.push(RuntimeBackup {
            unit: unit.clone(),
            prior_active,
        });
    }
    plan.service_start = start;
    plan.service_stop = stop;

    // converge declared settings after services, capturing prior value per change.
    // plan_settings only returns settings whose current value differs, so every
    // entry here genuinely changes and is worth backing up
    let setting_changes = plan_settings(b, desired)?;
    let mut setting_backups = Vec::with_capacity(setting_changes.len());
    for (key, value) in &setting_changes {
        // read the prior value BEFORE setting so rollback can restore it.
        // fully-qualified because SysctlBackend also has get/set in scope
        let prior = SettingsBackend::get(b, *key)?;
        SettingsBackend::set(b, *key, value)?;
        setting_backups.push(SettingBackup { key: *key, prior });
    }
    plan.setting_changes = setting_changes;

    // converge declared sysctl params after settings, capturing prior value per
    // change so rollback can restore it; a param already at its value is skipped
    let sysctl_changes = plan_sysctl(b, desired)?;
    let mut sysctl_backups = Vec::with_capacity(sysctl_changes.len());
    for param in &sysctl_changes {
        let prior = crate::backend::sysctl::SysctlBackend::get(b, &param.name)?;
        crate::backend::sysctl::SysctlBackend::set(b, &param.name, &param.value)?;
        sysctl_backups.push(SysctlBackup {
            name: param.name.clone(),
            prior,
        });
    }
    plan.sysctl_changes = sysctl_changes;

    Ok(Applied {
        plan,
        file_backups,
        service_backups,
        setting_backups,
        runtime_backups,
        sysctl_backups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{
            PackageBackend, fake::Call, fake::FakeBackend, file::FileBackend,
            service::ServiceBackend, settings::SettingsBackend,
        },
        journal::{FileBackup, RuntimeBackup, ServiceBackup, SettingBackup, SysctlBackup},
        state::{
            DesiredState, ManagedFile, ServiceIntent, SettingKey, SettingsIntent, SysctlParam,
        },
    };

    fn desired_files(files: Vec<ManagedFile>) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files,
            services: ServiceIntent::default(),
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        }
    }

    #[test]
    fn additive_apply_writes_new_file_and_backs_up_absent_prior() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
            ..Default::default()
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
                prior_mode: None,
                ..Default::default()
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
            ..Default::default()
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
                prior_mode: None,
                ..Default::default()
            }]
        );
    }

    #[test]
    fn apply_skips_file_whose_content_already_matches() {
        let mut backend = FakeBackend::with_files(&[("/etc/hostname", "gelbox\n")]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
            ..Default::default()
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
    fn apply_sets_declared_mode_and_backs_up_prior_mode() {
        // content already matches but the mode differs, so the apply must correct
        // the mode and record the prior mode, without rewriting the content
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        backend.set_file("/etc/exports", "x\n");
        backend.set_file_mode("/etc/exports", 0o600);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            mode: Some(0o644),
            ..Default::default()
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the mode is corrected on the backend
        assert_eq!(backend.mode("/etc/exports").expect("mode"), Some(0o644));
        // a set_mode was issued but no content write, since the content matched
        assert!(
            backend
                .calls()
                .contains(&Call::SetFileMode("/etc/exports".to_owned()))
        );
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::WriteFile(_)))
        );
        // the prior mode is captured so rollback can restore it
        assert_eq!(
            applied.file_backups,
            vec![FileBackup {
                path: "/etc/exports".to_owned(),
                prior: Some("x\n".to_owned()),
                prior_mode: Some(0o600),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn apply_creates_file_with_declared_mode_and_backs_up_none_prior_mode() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            mode: Some(0o640),
            ..Default::default()
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the file is created with both its content and the declared mode
        assert_eq!(
            backend.read_file("/etc/exports").expect("read"),
            Some("x\n".to_owned())
        );
        assert_eq!(backend.mode("/etc/exports").expect("mode"), Some(0o640));
        // a newly created file records None for both prior content and prior mode,
        // so rollback knows to delete it
        assert_eq!(
            applied.file_backups,
            vec![FileBackup {
                path: "/etc/exports".to_owned(),
                prior: None,
                prior_mode: None,
                ..Default::default()
            }]
        );
    }

    #[test]
    fn apply_with_unmanaged_mode_issues_no_set_mode() {
        // a file with mode None manages content only, so even a content change must
        // never touch the mode
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/motd".to_owned(),
            content: "hi\n".to_owned(),
            mode: None,
            ..Default::default()
        }]);

        apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetFileMode(_)))
        );
    }

    #[test]
    fn apply_sets_declared_owner_and_group_and_backs_up_prior() {
        // content and mode match, but owner and group differ, so the apply sets
        // them and records the prior owner and group, without rewriting content
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        backend.set_file("/etc/exports", "x\n");
        backend.set_file_owner("/etc/exports", "nobody");
        backend.set_file_group("/etc/exports", "nogroup");
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            owner: Some("root".to_owned()),
            group: Some("nfs".to_owned()),
            ..Default::default()
        }]);

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        assert_eq!(
            backend.owner("/etc/exports").expect("owner"),
            Some("root".to_owned())
        );
        assert_eq!(
            backend.group("/etc/exports").expect("group"),
            Some("nfs".to_owned())
        );
        // set_owner and set_group were issued, and no content write since it matched
        assert!(
            backend
                .calls()
                .contains(&Call::SetFileOwner("/etc/exports".to_owned()))
        );
        assert!(
            backend
                .calls()
                .contains(&Call::SetFileGroup("/etc/exports".to_owned()))
        );
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::WriteFile(_)))
        );
        // the prior owner and group are captured for rollback
        assert_eq!(
            applied.file_backups,
            vec![FileBackup {
                path: "/etc/exports".to_owned(),
                prior: Some("x\n".to_owned()),
                prior_mode: None,
                prior_owner: Some("nobody".to_owned()),
                prior_group: Some("nogroup".to_owned()),
            }]
        );
    }

    #[test]
    fn apply_with_unmanaged_ownership_issues_no_chown() {
        // a file that manages neither owner nor group must never chown, even when
        // its content changes
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = desired_files(vec![ManagedFile {
            path: "/etc/motd".to_owned(),
            content: "hi\n".to_owned(),
            ..Default::default()
        }]);

        apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetFileOwner(_) | Call::SetFileGroup(_)))
        );
    }

    #[test]
    fn apply_starts_declared_inactive_unit_and_backs_up_prior() {
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                start: vec!["nfs-server.service".to_owned()],
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the unit is now active on the backend
        assert!(backend.is_active("nfs-server.service").expect("query"));
        // its prior inactive state is recorded so rollback can stop it
        assert_eq!(
            applied.runtime_backups,
            vec![RuntimeBackup {
                unit: "nfs-server.service".to_owned(),
                prior_active: false,
            }]
        );
        // the effective plan reflects the start
        assert_eq!(
            applied.plan.service_start,
            vec!["nfs-server.service".to_owned()]
        );
        assert!(
            backend
                .calls()
                .contains(&Call::StartService("nfs-server.service".to_owned()))
        );
    }

    #[test]
    fn apply_skips_already_active_unit() {
        let mut backend = FakeBackend::with_active(&["nfs-server.service"]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                start: vec!["nfs-server.service".to_owned()],
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // an already-active unit is neither started nor backed up
        assert!(applied.runtime_backups.is_empty());
        assert!(applied.plan.service_start.is_empty());
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::StartService(_)))
        );
    }

    #[test]
    fn apply_stops_declared_active_unit_then_rollback_restarts_it() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        let mut backend = FakeBackend::with_active(&["bluetooth.service"]);
        let desired = DesiredState {
            services: ServiceIntent {
                stop: vec!["bluetooth.service".to_owned()],
                ..Default::default()
            },
            ..Default::default()
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the unit is now inactive, its prior active-state recorded
        assert!(!backend.is_active("bluetooth.service").expect("query"));
        assert_eq!(
            applied.plan.service_stop,
            vec!["bluetooth.service".to_owned()]
        );
        assert!(
            backend
                .calls()
                .contains(&Call::StopService("bluetooth.service".to_owned()))
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
            setting_backups: applied.setting_backups,
            runtime_backups: applied.runtime_backups,
            sysctl_backups: applied.sysctl_backups,
        };
        write_entry(dir.path(), &entry).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // rollback restarts the unit the transaction stopped
        assert!(backend.is_active("bluetooth.service").expect("query"));
    }

    #[test]
    fn apply_then_rollback_stops_a_unit_it_started() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        // the apply starts nfs (prior inactive); rollback must stop it again
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                start: vec!["nfs-server.service".to_owned()],
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");
        assert!(backend.is_active("nfs-server.service").expect("query"));

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
            setting_backups: applied.setting_backups,
            runtime_backups: applied.runtime_backups,
            sysctl_backups: applied.sysctl_backups,
        };
        write_entry(dir.path(), &entry).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // the unit the transaction started is stopped again
        assert!(!backend.is_active("nfs-server.service").expect("query"));
    }

    #[test]
    fn apply_sets_declared_sysctl_and_backs_up_prior() {
        let mut backend = FakeBackend::with_sysctl(&[("net.ipv4.ip_forward", "0")]);
        let desired = DesiredState {
            sysctl: vec![SysctlParam {
                name: "net.ipv4.ip_forward".to_owned(),
                value: "1".to_owned(),
            }],
            ..Default::default()
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // fully-qualified get because SettingsBackend's get is also in scope
        assert_eq!(
            crate::backend::sysctl::SysctlBackend::get(&backend, "net.ipv4.ip_forward")
                .expect("get"),
            Some("1".to_owned())
        );
        assert_eq!(
            applied.sysctl_backups,
            vec![SysctlBackup {
                name: "net.ipv4.ip_forward".to_owned(),
                prior: Some("0".to_owned()),
            }]
        );
        assert!(backend.calls().contains(&Call::SetSysctl(
            "net.ipv4.ip_forward".to_owned(),
            "1".to_owned()
        )));
    }

    #[test]
    fn apply_then_rollback_restores_prior_sysctl() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        let mut backend = FakeBackend::with_sysctl(&[("net.ipv4.ip_forward", "0")]);
        let desired = DesiredState {
            sysctl: vec![SysctlParam {
                name: "net.ipv4.ip_forward".to_owned(),
                value: "1".to_owned(),
            }],
            ..Default::default()
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
            setting_backups: applied.setting_backups,
            runtime_backups: applied.runtime_backups,
            sysctl_backups: applied.sysctl_backups,
        };
        write_entry(dir.path(), &entry).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // the parameter is restored to its recorded prior value
        assert_eq!(
            crate::backend::sysctl::SysctlBackend::get(&backend, "net.ipv4.ip_forward")
                .expect("get"),
            Some("0".to_owned())
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
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
    fn apply_changes_declared_setting_and_backs_up_prior_value() {
        // seed a prior value so the apply must change it and record the old value
        let mut backend = FakeBackend::with_settings(&[(SettingKey::Hostname, "old")]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            sysctl: Vec::new(),
            settings: SettingsIntent {
                hostname: Some("gelbox".to_owned()),
                ..SettingsIntent::default()
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the setting now holds the desired value on the backend
        assert_eq!(
            backend.get(SettingKey::Hostname).expect("get"),
            Some("gelbox".to_owned())
        );
        // its prior value is recorded so rollback can restore it
        assert_eq!(
            applied.setting_backups,
            vec![SettingBackup {
                key: SettingKey::Hostname,
                prior: Some("old".to_owned()),
            }]
        );
        // the effective plan reflects the change
        assert_eq!(
            applied.plan.setting_changes,
            vec![(SettingKey::Hostname, "gelbox".to_owned())]
        );
        // the set is observable in the call log
        assert!(
            backend
                .calls()
                .contains(&Call::SetSetting(SettingKey::Hostname, "gelbox".to_owned()))
        );
    }

    #[test]
    fn apply_sets_unset_declared_setting_with_none_prior() {
        // no seed, so the setting reads None before the apply
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            sysctl: Vec::new(),
            settings: SettingsIntent {
                timezone: Some("UTC".to_owned()),
                ..SettingsIntent::default()
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the setting now holds the desired value
        assert_eq!(
            backend.get(SettingKey::Timezone).expect("get"),
            Some("UTC".to_owned())
        );
        // a previously unset setting records a None prior, so rollback leaves it be
        assert_eq!(
            applied.setting_backups,
            vec![SettingBackup {
                key: SettingKey::Timezone,
                prior: None,
            }]
        );
    }

    #[test]
    fn apply_skips_setting_already_equal_to_desired() {
        let mut backend = FakeBackend::with_settings(&[(SettingKey::Hostname, "gelbox")]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            sysctl: Vec::new(),
            settings: SettingsIntent {
                hostname: Some("gelbox".to_owned()),
                ..SettingsIntent::default()
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // an already-matching setting is neither changed nor backed up
        assert!(applied.plan.setting_changes.is_empty());
        assert!(applied.setting_backups.is_empty());
        assert!(
            !backend
                .calls()
                .iter()
                .any(|call| matches!(call, Call::SetSetting(..)))
        );
    }

    #[test]
    fn apply_then_rollback_leaves_a_none_prior_setting_as_is() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        // no seed: the timezone is unset, so the apply records a None prior
        let mut backend = FakeBackend::with_explicit(&[], &[]);
        let desired = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            sysctl: Vec::new(),
            settings: SettingsIntent {
                timezone: Some("UTC".to_owned()),
                ..SettingsIntent::default()
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: applied.plan,
            snapshot: None,
            file_backups: applied.file_backups,
            service_backups: applied.service_backups,
            setting_backups: applied.setting_backups,
            ..Default::default()
        };
        write_entry(dir.path(), &entry).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // a None-prior setting cannot be unset, so rollback leaves the value in
        // place rather than erroring or clearing it
        assert_eq!(
            backend.get(SettingKey::Timezone).expect("get"),
            Some("UTC".to_owned())
        );
    }

    #[test]
    fn full_apply_then_rollback_restores_files_services_and_inverts_packages() {
        use crate::journal::{JournalEntry, rollback_last, write_entry};

        // start: git + vim installed, one pre-existing managed file with old
        // content, bluetooth enabled (to be disabled), sshd disabled (to be
        // enabled), hostname set to "old" (to be changed to "gelbox")
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        backend.set_file("/etc/changed", "old\n");
        backend.enable("bluetooth.service").expect("seed enable");
        backend
            .set(SettingKey::Hostname, "old")
            .expect("seed setting");
        // the seeding calls must not leak into the transaction call log below
        let seeded_calls = backend.calls().len();
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![
                ManagedFile {
                    path: "/etc/new".to_owned(),
                    content: "created\n".to_owned(),
                    ..Default::default()
                },
                ManagedFile {
                    path: "/etc/changed".to_owned(),
                    content: "new\n".to_owned(),
                    ..Default::default()
                },
            ],
            services: ServiceIntent {
                enable: vec!["sshd.service".to_owned()],
                disable: vec!["bluetooth.service".to_owned()],
                ..Default::default()
            },
            sysctl: Vec::new(),
            settings: SettingsIntent {
                hostname: Some("gelbox".to_owned()),
                ..SettingsIntent::default()
            },
        };

        let applied = apply(&mut backend, &desired, ApplyOpts { prune: false }).expect("apply");

        // the apply enabled sshd and disabled bluetooth, and recorded each prior
        assert!(backend.is_enabled("sshd.service").expect("query"));
        assert!(!backend.is_enabled("bluetooth.service").expect("query"));
        // the apply changed the hostname and recorded its prior value
        assert_eq!(
            backend.get(SettingKey::Hostname).expect("query"),
            Some("gelbox".to_owned())
        );
        assert_eq!(
            applied.setting_backups,
            vec![SettingBackup {
                key: SettingKey::Hostname,
                prior: Some("old".to_owned()),
            }]
        );
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
            setting_backups: applied.setting_backups,
            ..Default::default()
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
        // the setting is restored to its prior value
        assert_eq!(
            backend.get(SettingKey::Hostname).expect("query"),
            Some("old".to_owned())
        );
    }

    #[test]
    fn additive_default_installs_but_does_not_prune() {
        let mut backend = FakeBackend::with_explicit(&["git", "vim"], &[]);
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec![],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            setting_backups: applied.setting_backups,
            ..Default::default()
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
