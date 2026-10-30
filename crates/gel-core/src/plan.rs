use std::collections::BTreeSet;

use crate::{
    backend::{
        file::FileBackend, service::ServiceBackend, settings::SettingsBackend,
        sysctl::SysctlBackend,
    },
    error::GelError,
    state::{DesiredState, ManagedFile, SettingKey, SysctlParam},
};

/// A deterministic set of package and file changes to reconcile current with desired
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Plan {
    pub native_install: Vec<String>,
    pub native_remove: Vec<String>,
    pub foreign_install: Vec<String>,
    pub foreign_remove: Vec<String>,
    pub file_writes: Vec<ManagedFile>,
    /// Defaulted so journal entries predating the service model still deserialize
    #[serde(default)]
    pub service_enable: Vec<String>,
    /// Defaulted so journal entries predating the service model still deserialize
    #[serde(default)]
    pub service_disable: Vec<String>,
    /// Units to start (ensure active)
    ///
    /// Defaulted so journal entries predating runtime management still deserialize
    #[serde(default)]
    pub service_start: Vec<String>,
    /// Units to stop (ensure inactive)
    ///
    /// Defaulted so journal entries predating runtime management still deserialize
    #[serde(default)]
    pub service_stop: Vec<String>,
    /// Settings to change, as `(key, desired value)` pairs
    ///
    /// Defaulted so journal entries predating the settings model still deserialize
    #[serde(default)]
    pub setting_changes: Vec<(SettingKey, String)>,
    /// Sysctl parameters to change
    ///
    /// Defaulted so journal entries predating sysctl management still deserialize
    #[serde(default)]
    pub sysctl_changes: Vec<SysctlParam>,
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
            // Service actions require querying unit state, which is impure, so
            // they are planned separately via `plan_services`
            service_enable: Vec::new(),
            service_disable: Vec::new(),
            // Runtime start/stop actions require querying active state, which is
            // impure, so they are planned separately via `plan_runtime`
            service_start: Vec::new(),
            service_stop: Vec::new(),
            // Setting changes require reading current values, which is impure, so
            // they are planned separately via `plan_settings`
            setting_changes: Vec::new(),
            // Sysctl changes require reading current values too, planned via
            // `plan_sysctl`
            sysctl_changes: Vec::new(),
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
            && self.service_enable.is_empty()
            && self.service_disable.is_empty()
            && self.service_start.is_empty()
            && self.service_stop.is_empty()
            && self.setting_changes.is_empty()
            && self.sysctl_changes.is_empty()
    }
}

/// Compute the managed files that need a write to reach `desired`
///
/// A desired file is included when it is absent on the backend, its current
/// content differs, or it declares a mode, owner, or group that differs from the
/// current one. Any of mode/owner/group left `None` is unmanaged and never forces
/// a write. Unlike [`Plan::compute`], this reads current state through `backend`,
/// so it is kept out of the pure planner.
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
        let content_differs = current.as_deref() != Some(file.content.as_str());
        // a declared mode that differs from the current mode also warrants a write,
        // so a mode-only correction is not silently skipped when content matches. A
        // mode of None means gel does not manage the mode, so it never forces a write
        let mode_differs = match file.mode {
            Some(desired_mode) => backend.mode(&file.path)? != Some(desired_mode),
            None => false,
        };
        // ownership is managed independently: a declared owner or group that
        // differs from the current one warrants a write, and a None leaves that
        // facet unmanaged (never forcing a write)
        let owner_differs = match &file.owner {
            Some(owner) => backend.owner(&file.path)?.as_deref() != Some(owner.as_str()),
            None => false,
        };
        let group_differs = match &file.group {
            Some(group) => backend.group(&file.path)?.as_deref() != Some(group.as_str()),
            None => false,
        };
        if content_differs || mode_differs || owner_differs || group_differs {
            writes.push(file.clone());
        }
    }
    writes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(writes)
}

/// Compute the service enable/disable actions needed to reach `desired`
///
/// Returns `(enable, disable)` where `enable` is the declared-enable units that
/// are currently disabled and `disable` is the declared-disable units that are
/// currently enabled. Both are sorted and deduplicated so the plan is
/// deterministic, and a unit never appears in both lists.
///
/// This is explicit-intent planning, not full-set convergence: only units named
/// in `desired.services` are ever considered, so a unit absent from both lists is
/// left untouched and gel never disables a unit it was not told about.
///
/// Conflict rule: when a unit is declared in both `enable` and `disable`,
/// **disable wins**. It is removed from the enable candidates and only ever
/// disabled, so an ambiguous declaration can never leave a unit running.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to query a unit.
pub fn plan_services(
    backend: &impl ServiceBackend,
    desired: &DesiredState,
) -> Result<(Vec<String>, Vec<String>), GelError> {
    let disable_set: BTreeSet<&String> = desired.services.disable.iter().collect();

    let mut enable = Vec::new();
    for unit in &desired.services.enable {
        // disable wins over an ambiguous enable/disable declaration
        if disable_set.contains(unit) {
            continue;
        }
        if !backend.is_enabled(unit)? {
            enable.push(unit.clone());
        }
    }

    let mut disable = Vec::new();
    for unit in &desired.services.disable {
        if backend.is_enabled(unit)? {
            disable.push(unit.clone());
        }
    }

    Ok((sorted_unique(enable), sorted_unique(disable)))
}

/// Compute the runtime start/stop actions needed to reach `desired`
///
/// Returns `(start, stop)` where `start` is the declared-start units currently
/// inactive and `stop` is the declared-stop units currently active, each sorted
/// and deduplicated. Explicit-intent planning: only units named in
/// `desired.services.start`/`stop` are considered. Runtime is an independent axis
/// from enablement (a unit may be both enabled and started). Conflict rule: a unit
/// declared both start and stop is only stopped (stop wins), so it never appears
/// in `start`.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to query a unit's active state.
pub fn plan_runtime(
    backend: &impl ServiceBackend,
    desired: &DesiredState,
) -> Result<(Vec<String>, Vec<String>), GelError> {
    let stop_set: BTreeSet<&String> = desired.services.stop.iter().collect();

    let mut start = Vec::new();
    for unit in &desired.services.start {
        // stop wins over an ambiguous start/stop declaration
        if stop_set.contains(unit) {
            continue;
        }
        if !backend.is_active(unit)? {
            start.push(unit.clone());
        }
    }

    let mut stop = Vec::new();
    for unit in &desired.services.stop {
        if backend.is_active(unit)? {
            stop.push(unit.clone());
        }
    }

    Ok((sorted_unique(start), sorted_unique(stop)))
}

/// Compute the setting changes needed to reach `desired`
///
/// Returns the declared settings whose current value differs from the desired
/// value, as `(key, desired value)` pairs. A setting is included when it is
/// unset or unreadable on the backend, or when its current value differs; a
/// setting already equal to the desired value is skipped. Results follow the
/// deterministic order of [`SettingsIntent::declared`](crate::state::SettingsIntent::declared)
/// (Hostname, Timezone, Locale) so the plan is reproducible.
///
/// This is explicit-intent planning, not full-set convergence: only settings
/// named in `desired.settings` are ever considered, so a setting gel was not
/// told about is left untouched.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to read a setting.
pub fn plan_settings(
    backend: &impl SettingsBackend,
    desired: &DesiredState,
) -> Result<Vec<(SettingKey, String)>, GelError> {
    let mut changes = Vec::new();
    for (key, value) in desired.settings.declared() {
        let current = backend.get(key)?;
        if current.as_deref() != Some(value.as_str()) {
            changes.push((key, value));
        }
    }
    Ok(changes)
}

/// Compute the sysctl parameters to change to reach `desired`
///
/// Returns the declared params whose current value differs from the desired value
/// (including when the param is unset or unreadable on the backend). Explicit
/// intent: only params named in `desired.sysctl` are considered.
///
/// # Errors
///
/// Returns [`GelError`] if the backend fails to read a parameter.
pub fn plan_sysctl(
    backend: &impl SysctlBackend,
    desired: &DesiredState,
) -> Result<Vec<SysctlParam>, GelError> {
    let mut changes = Vec::new();
    for param in &desired.sysctl {
        let current = backend.get(&param.name)?;
        if current.as_deref() != Some(param.value.as_str()) {
            changes.push(param.clone());
        }
    }
    Ok(changes)
}

/// Sort a list of units and drop duplicates
fn sorted_unique(units: Vec<String>) -> Vec<String> {
    units
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
        state::{
            DesiredState, ManagedFile, ServiceIntent, SettingKey, SettingsIntent, SysctlParam,
        },
    };

    fn desired_with_sysctl(params: &[(&str, &str)]) -> DesiredState {
        DesiredState {
            sysctl: params
                .iter()
                .map(|(name, value)| SysctlParam {
                    name: (*name).to_owned(),
                    value: (*value).to_owned(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn desired_with_settings(settings: SettingsIntent) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            settings,
            sysctl: Vec::new(),
        }
    }

    fn desired_with_files(files: Vec<ManagedFile>) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files,
            services: ServiceIntent::default(),
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        }
    }

    fn desired_with_start(start: &[&str]) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                start: start.iter().map(|s| (*s).to_owned()).collect(),
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        }
    }

    fn desired_with_runtime(start: &[&str], stop: &[&str]) -> DesiredState {
        DesiredState {
            services: ServiceIntent {
                start: start.iter().map(|s| (*s).to_owned()).collect(),
                stop: stop.iter().map(|s| (*s).to_owned()).collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn desired_with_services(enable: &[&str], disable: &[&str]) -> DesiredState {
        DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent {
                enable: enable.iter().map(|s| (*s).to_owned()).collect(),
                disable: disable.iter().map(|s| (*s).to_owned()).collect(),
                ..Default::default()
            },
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        }
    }

    #[test]
    fn absent_file_is_planned_for_write() {
        let backend = FakeBackend::default();
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/hostname".to_owned(),
            content: "gelbox\n".to_owned(),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn file_with_matching_content_but_differing_mode_is_planned() {
        // content already matches, but the declared mode differs from the current
        // mode, so the file is still planned so the mode can be corrected
        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_mode("/etc/exports", 0o600);
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            mode: Some(0o644),
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn file_with_matching_content_and_mode_is_not_planned() {
        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_mode("/etc/exports", 0o644);
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            mode: Some(0o644),
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert!(writes.is_empty());
    }

    #[test]
    fn file_with_unmanaged_mode_and_matching_content_is_not_planned() {
        // mode None means gel does not manage the mode, so a content match alone
        // means nothing to do regardless of the file's actual mode on disk
        let mut backend = FakeBackend::with_files(&[("/etc/motd", "hi\n")]);
        backend.set_file_mode("/etc/motd", 0o600);
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/motd".to_owned(),
            content: "hi\n".to_owned(),
            mode: None,
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert!(writes.is_empty());
    }

    #[test]
    fn file_with_differing_owner_is_planned() {
        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_owner("/etc/exports", "nobody");
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            owner: Some("root".to_owned()),
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn file_with_differing_group_is_planned() {
        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_group("/etc/exports", "wheel");
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            group: Some("nfs".to_owned()),
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert_eq!(writes, desired.files);
    }

    #[test]
    fn file_with_matching_owner_and_group_is_not_planned() {
        let mut backend = FakeBackend::with_files(&[("/etc/exports", "x\n")]);
        backend.set_file_owner("/etc/exports", "root");
        backend.set_file_group("/etc/exports", "nfs");
        let desired = desired_with_files(vec![ManagedFile {
            path: "/etc/exports".to_owned(),
            content: "x\n".to_owned(),
            owner: Some("root".to_owned()),
            group: Some("nfs".to_owned()),
            ..Default::default()
        }]);

        let writes = plan_files(&backend, &desired).expect("plan");

        assert!(writes.is_empty());
    }

    #[test]
    fn writes_are_ordered_by_path() {
        let backend = FakeBackend::default();
        let desired = desired_with_files(vec![
            ManagedFile {
                path: "/z".to_owned(),
                content: "z".to_owned(),
                ..Default::default()
            },
            ManagedFile {
                path: "/a".to_owned(),
                content: "a".to_owned(),
                ..Default::default()
            },
            ManagedFile {
                path: "/m".to_owned(),
                content: "m".to_owned(),
                ..Default::default()
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
                ..Default::default()
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };
        let desired = DesiredState {
            native: vec!["git".to_owned(), "ripgrep".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
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
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };
        let desired = DesiredState {
            native: vec!["zsh".to_owned(), "bash".to_owned(), "bash".to_owned()],
            foreign: vec![],
            files: vec![],
            services: crate::state::ServiceIntent::default(),
            settings: SettingsIntent::default(),
            sysctl: Vec::new(),
        };

        let plan = Plan::compute(&current, &desired);

        assert_eq!(
            plan.native_install,
            vec!["bash".to_owned(), "zsh".to_owned()]
        );
        assert_eq!(plan.native_remove, vec!["git".to_owned(), "vim".to_owned()]);
    }

    #[test]
    fn declared_enable_already_enabled_is_excluded() {
        let backend = FakeBackend::with_enabled(&["sshd.service"]);
        let desired = desired_with_services(&["sshd.service"], &[]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert!(enable.is_empty());
        assert!(disable.is_empty());
    }

    #[test]
    fn declared_enable_currently_disabled_is_included() {
        let backend = FakeBackend::default();
        let desired = desired_with_services(&["sshd.service"], &[]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert_eq!(enable, vec!["sshd.service".to_owned()]);
        assert!(disable.is_empty());
    }

    #[test]
    fn declared_disable_currently_enabled_is_included() {
        let backend = FakeBackend::with_enabled(&["bluetooth.service"]);
        let desired = desired_with_services(&[], &["bluetooth.service"]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert!(enable.is_empty());
        assert_eq!(disable, vec!["bluetooth.service".to_owned()]);
    }

    #[test]
    fn declared_disable_already_disabled_is_excluded() {
        let backend = FakeBackend::default();
        let desired = desired_with_services(&[], &["bluetooth.service"]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert!(enable.is_empty());
        assert!(disable.is_empty());
    }

    #[test]
    fn conflicting_unit_resolves_disable_wins() {
        // A unit declared in both enable and disable is treated as a disable
        // target only, and must never appear in the enable list
        let backend = FakeBackend::with_enabled(&["conflict.service"]);
        let desired = desired_with_services(&["conflict.service"], &["conflict.service"]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert!(enable.is_empty());
        assert_eq!(disable, vec!["conflict.service".to_owned()]);
    }

    #[test]
    fn conflicting_unit_currently_disabled_appears_in_neither() {
        // Disable-wins removes the unit from the enable candidates, and since it
        // is already disabled it is not a disable candidate either
        let backend = FakeBackend::default();
        let desired = desired_with_services(&["conflict.service"], &["conflict.service"]);

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert!(enable.is_empty());
        assert!(disable.is_empty());
    }

    #[test]
    fn service_plan_is_sorted_and_deduplicated() {
        let backend = FakeBackend::with_enabled(&["b.service", "d.service"]);
        let desired = desired_with_services(
            &["c.service", "a.service", "a.service"],
            &["d.service", "b.service", "b.service"],
        );

        let (enable, disable) = plan_services(&backend, &desired).expect("plan");

        assert_eq!(enable, vec!["a.service".to_owned(), "c.service".to_owned()]);
        assert_eq!(
            disable,
            vec!["b.service".to_owned(), "d.service".to_owned()]
        );
    }

    #[test]
    fn service_plan_counts_toward_is_empty() {
        let enable_plan = Plan {
            service_enable: vec!["sshd.service".to_owned()],
            ..Plan::default()
        };
        assert!(!enable_plan.is_empty());

        let disable_plan = Plan {
            service_disable: vec!["bluetooth.service".to_owned()],
            ..Plan::default()
        };
        assert!(!disable_plan.is_empty());
    }

    #[test]
    fn declared_start_currently_inactive_is_included() {
        let backend = FakeBackend::default();
        let desired = desired_with_start(&["nfs-server.service"]);

        let (start, _stop) = plan_runtime(&backend, &desired).expect("plan");

        assert_eq!(start, vec!["nfs-server.service".to_owned()]);
    }

    #[test]
    fn declared_start_already_active_is_excluded() {
        let backend = FakeBackend::with_active(&["nfs-server.service"]);
        let desired = desired_with_start(&["nfs-server.service"]);

        let (start, _stop) = plan_runtime(&backend, &desired).expect("plan");

        assert!(start.is_empty());
    }

    #[test]
    fn runtime_plan_is_sorted_and_deduplicated() {
        let backend = FakeBackend::default();
        let desired = desired_with_start(&["c.service", "a.service", "a.service"]);

        let (start, _stop) = plan_runtime(&backend, &desired).expect("plan");

        assert_eq!(start, vec!["a.service".to_owned(), "c.service".to_owned()]);
    }

    #[test]
    fn service_start_counts_toward_is_empty() {
        let plan = Plan {
            service_start: vec!["nfs-server.service".to_owned()],
            ..Plan::default()
        };

        assert!(!plan.is_empty());
    }

    #[test]
    fn declared_stop_currently_active_is_included() {
        let backend = FakeBackend::with_active(&["bluetooth.service"]);
        let desired = desired_with_runtime(&[], &["bluetooth.service"]);

        let (start, stop) = plan_runtime(&backend, &desired).expect("plan");

        assert!(start.is_empty());
        assert_eq!(stop, vec!["bluetooth.service".to_owned()]);
    }

    #[test]
    fn declared_stop_already_inactive_is_excluded() {
        let backend = FakeBackend::default();
        let desired = desired_with_runtime(&[], &["bluetooth.service"]);

        let (_start, stop) = plan_runtime(&backend, &desired).expect("plan");

        assert!(stop.is_empty());
    }

    #[test]
    fn runtime_conflict_resolves_stop_wins() {
        // a unit declared both start and stop is only ever stopped, never started
        let backend = FakeBackend::with_active(&["conflict.service"]);
        let desired = desired_with_runtime(&["conflict.service"], &["conflict.service"]);

        let (start, stop) = plan_runtime(&backend, &desired).expect("plan");

        assert!(start.is_empty());
        assert_eq!(stop, vec!["conflict.service".to_owned()]);
    }

    #[test]
    fn service_stop_counts_toward_is_empty() {
        let plan = Plan {
            service_stop: vec!["bluetooth.service".to_owned()],
            ..Plan::default()
        };

        assert!(!plan.is_empty());
    }

    #[test]
    fn declared_setting_matching_current_is_excluded() {
        let backend = FakeBackend::with_settings(&[(SettingKey::Hostname, "gelbox")]);
        let desired = desired_with_settings(SettingsIntent {
            hostname: Some("gelbox".to_owned()),
            ..SettingsIntent::default()
        });

        let changes = plan_settings(&backend, &desired).expect("plan");

        assert!(changes.is_empty());
    }

    #[test]
    fn declared_setting_differing_from_current_is_included() {
        let backend = FakeBackend::with_settings(&[(SettingKey::Hostname, "old")]);
        let desired = desired_with_settings(SettingsIntent {
            hostname: Some("gelbox".to_owned()),
            ..SettingsIntent::default()
        });

        let changes = plan_settings(&backend, &desired).expect("plan");

        assert_eq!(changes, vec![(SettingKey::Hostname, "gelbox".to_owned())]);
    }

    #[test]
    fn declared_setting_unset_on_backend_is_included() {
        let backend = FakeBackend::default();
        let desired = desired_with_settings(SettingsIntent {
            timezone: Some("UTC".to_owned()),
            ..SettingsIntent::default()
        });

        let changes = plan_settings(&backend, &desired).expect("plan");

        assert_eq!(changes, vec![(SettingKey::Timezone, "UTC".to_owned())]);
    }

    #[test]
    fn undeclared_settings_never_appear() {
        // The backend carries a timezone gel was not told to manage, so it must
        // never surface as a change
        let backend = FakeBackend::with_settings(&[(SettingKey::Timezone, "UTC")]);
        let desired = desired_with_settings(SettingsIntent {
            hostname: Some("gelbox".to_owned()),
            ..SettingsIntent::default()
        });

        let changes = plan_settings(&backend, &desired).expect("plan");

        assert_eq!(changes, vec![(SettingKey::Hostname, "gelbox".to_owned())]);
    }

    #[test]
    fn setting_changes_are_ordered_hostname_timezone_locale() {
        let backend = FakeBackend::default();
        let desired = desired_with_settings(SettingsIntent {
            hostname: Some("gelbox".to_owned()),
            timezone: Some("UTC".to_owned()),
            locale: Some("en_US.UTF-8".to_owned()),
        });

        let changes = plan_settings(&backend, &desired).expect("plan");

        assert_eq!(
            changes,
            vec![
                (SettingKey::Hostname, "gelbox".to_owned()),
                (SettingKey::Timezone, "UTC".to_owned()),
                (SettingKey::Locale, "en_US.UTF-8".to_owned()),
            ]
        );
    }

    #[test]
    fn setting_changes_count_toward_is_empty() {
        let plan = Plan {
            setting_changes: vec![(SettingKey::Hostname, "gelbox".to_owned())],
            ..Plan::default()
        };

        assert!(!plan.is_empty());
    }

    #[test]
    fn declared_sysctl_differing_is_included() {
        let backend = FakeBackend::with_sysctl(&[("net.ipv4.ip_forward", "0")]);
        let desired = desired_with_sysctl(&[("net.ipv4.ip_forward", "1")]);

        let changes = plan_sysctl(&backend, &desired).expect("plan");

        assert_eq!(
            changes,
            vec![SysctlParam {
                name: "net.ipv4.ip_forward".to_owned(),
                value: "1".to_owned(),
            }]
        );
    }

    #[test]
    fn declared_sysctl_matching_is_excluded() {
        let backend = FakeBackend::with_sysctl(&[("net.ipv4.ip_forward", "1")]);
        let desired = desired_with_sysctl(&[("net.ipv4.ip_forward", "1")]);

        let changes = plan_sysctl(&backend, &desired).expect("plan");

        assert!(changes.is_empty());
    }

    #[test]
    fn declared_sysctl_unset_on_backend_is_included() {
        let backend = FakeBackend::default();
        let desired = desired_with_sysctl(&[("net.ipv4.ip_forward", "1")]);

        let changes = plan_sysctl(&backend, &desired).expect("plan");

        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn sysctl_changes_count_toward_is_empty() {
        let plan = Plan {
            sysctl_changes: vec![SysctlParam {
                name: "net.ipv4.ip_forward".to_owned(),
                value: "1".to_owned(),
            }],
            ..Plan::default()
        };

        assert!(!plan.is_empty());
    }
}
