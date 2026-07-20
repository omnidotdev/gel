//! System-touching commands: `import`, `diff`, `apply`, `rollback`
//!
//! Compiled only with the `arch` feature. Every command here drives the real
//! [`ArchBackend`]; the transaction id and timestamp are generated in this layer
//! so that gel-core stays pure and clock-free. Without the `arch` feature these
//! commands are replaced by a fast-fail stub in `main`.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use gel_core::{
    apply::{ApplyOpts, apply},
    backend::{PackageBackend, arch::ArchBackend},
    journal::{self, JournalEntry},
    plan::{Plan, plan_files, plan_services},
    snapshot::{SnapshotId, SnapshotProvider},
    snapshot_btrfs::BtrfsSnapshot,
    state::DesiredState,
};

use crate::{paths, render};

/// Load the desired-state artifact, defaulting to [`paths::default_artifact`]
fn load_artifact(artifact: Option<PathBuf>) -> anyhow::Result<DesiredState> {
    let path = match artifact {
        Some(path) => path,
        None => paths::default_artifact()?,
    };
    let json = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read desired-state artifact at {} (run `gel eval` first)",
            path.display()
        )
    })?;
    serde_json::from_str(&json).with_context(|| {
        format!(
            "failed to parse desired-state artifact at {}",
            path.display()
        )
    })
}

/// `gel diff`: print the plan to converge toward the artifact, without mutating
///
/// # Errors
///
/// Returns an error when the artifact cannot be loaded or the backend cannot be
/// queried.
pub fn diff(artifact: Option<PathBuf>) -> anyhow::Result<()> {
    let desired = load_artifact(artifact)?;
    let backend = ArchBackend::new();
    let current = backend
        .query_explicit()
        .context("failed to query current package state")?;
    let mut plan = Plan::compute(&current, &desired);
    // surface managed file writes alongside the package plan; this reads current
    // file content but writes nothing, so diff stays read-only
    plan.file_writes = plan_files(&backend, &desired).context("failed to plan managed files")?;
    // surface service enable/disable actions; this queries unit state but changes
    // nothing, so diff stays read-only
    let (service_enable, service_disable) =
        plan_services(&backend, &desired).context("failed to plan service changes")?;
    plan.service_enable = service_enable;
    plan.service_disable = service_disable;
    render::print_plan(&plan);
    Ok(())
}

/// `gel import`: capture the current explicit packages as a desired state
///
/// # Errors
///
/// Returns an error when the backend cannot be queried or the output cannot be
/// written.
pub fn import(out: Option<PathBuf>) -> anyhow::Result<()> {
    let backend = ArchBackend::new();
    let desired =
        gel_core::import::import(&backend).context("failed to import current package state")?;
    let json = serde_json::to_string_pretty(&desired)?;
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::write(&path, json)
                .with_context(|| format!("failed to write import to {}", path.display()))?;
            println!(
                "imported {} native, {} foreign -> {}",
                desired.native.len(),
                desired.foreign.len(),
                path.display()
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// `gel apply`: converge the system toward the artifact and journal the result
///
/// # Errors
///
/// Returns an error when the artifact cannot be loaded, the backend fails, or
/// the journal entry cannot be written.
pub fn apply_cmd(prune: bool, artifact: Option<PathBuf>) -> anyhow::Result<()> {
    let desired = load_artifact(artifact)?;
    let mut backend = ArchBackend::new();

    // generate the transaction id/timestamp here so gel-core stays clock-free.
    // zero-padded nanoseconds sort lexicographically in chronological order,
    // matching the journal's `(timestamp, id)` recency ordering
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is set before the Unix epoch")?
        .as_nanos();
    let timestamp = format!("{nanos:020}");
    let id = timestamp.clone();

    // preview the EFFECTIVE plan (what will actually run) before mutating
    let current = backend
        .query_explicit()
        .context("failed to query current package state")?;
    let mut preview = Plan::compute(&current, &desired);
    if !prune {
        // additive mode executes no removals, so do not advertise them
        preview.native_remove.clear();
        preview.foreign_remove.clear();
    }
    // include pending file writes so the preview mentions them and the
    // already-converged short-circuit below does not skip a needed file write
    preview.file_writes = plan_files(&backend, &desired).context("failed to plan managed files")?;
    // include pending service actions for the same reason: a service-only change
    // must be previewed and must not be treated as "nothing to apply"
    let (service_enable, service_disable) =
        plan_services(&backend, &desired).context("failed to plan service changes")?;
    preview.service_enable = service_enable;
    preview.service_disable = service_disable;
    render::print_plan(&preview);
    if preview.is_empty() {
        println!("system already matches the desired state; nothing to apply");
        return Ok(());
    }

    // take a pre-apply snapshot, degrading gracefully when unavailable
    let snapshot = take_snapshot(&id);

    let applied = apply(&mut backend, &desired, ApplyOpts { prune })
        .context("failed to apply the desired state")?;

    let entry = JournalEntry {
        id,
        timestamp,
        plan: applied.plan,
        snapshot,
        file_backups: applied.file_backups,
        service_backups: applied.service_backups,
    };
    journal::write_entry(&paths::journal_dir()?, &entry)
        .context("failed to record the transaction journal entry")?;

    let installed = entry.plan.native_install.len() + entry.plan.foreign_install.len();
    let removed = entry.plan.native_remove.len() + entry.plan.foreign_remove.len();
    let files = entry.plan.file_writes.len();
    let enabled = entry.plan.service_enable.len();
    let disabled = entry.plan.service_disable.len();
    println!(
        "applied: +{installed} installed, -{removed} removed, {files} files written, +{enabled} enabled, -{disabled} disabled"
    );
    Ok(())
}

/// `gel rollback`: invert the most recent apply at the package level
///
/// # Errors
///
/// Returns an error when the journal cannot be read or a backend mutation fails.
pub fn rollback() -> anyhow::Result<()> {
    let dir = paths::journal_dir()?;
    let mut backend = ArchBackend::new();

    let Some(latest) =
        journal::read_latest(&dir).context("failed to read the transaction journal")?
    else {
        println!("nothing to roll back");
        return Ok(());
    };

    // report the inverse before performing it
    let plan = &latest.plan;
    let reinstall = plan.native_remove.len() + plan.foreign_remove.len();
    let uninstall = plan.native_install.len() + plan.foreign_install.len();
    let files = latest.file_backups.len();
    let services = latest.service_backups.len();
    println!("rolling back transaction {}", latest.id);
    println!(
        "+{reinstall} to reinstall, -{uninstall} to remove, {files} files to restore, {services} services to restore"
    );

    journal::rollback_last(&dir, &mut backend)
        .context("failed to roll back the last transaction")?;

    println!("rolled back transaction {}", latest.id);
    // rollback reverses packages, restores gel-managed files to their prior
    // content, and restores each touched unit's prior enabled state; a full
    // snapshot-based filesystem restore is planned for later
    println!(
        "note: this reverses packages, restores managed files, and restores prior service enabled state; snapshot-based filesystem restore is planned for a later phase"
    );
    Ok(())
}

/// Take a pre-apply snapshot, warning and continuing when one is unavailable
///
/// Snapshot support is best-effort: a missing snapshot provider or a snapshot
/// failure never aborts a converge, because phase-1 rollback is package-level
/// (via the journal) and does not depend on a filesystem snapshot.
fn take_snapshot(tag: &str) -> Option<SnapshotId> {
    let provider = BtrfsSnapshot::new();
    match provider.snapshot(tag) {
        Ok(Some(id)) => {
            println!("took pre-apply snapshot {}", id.0);
            Some(id)
        }
        Ok(None) => {
            println!(
                "note: no filesystem snapshot taken (snapshots unavailable); package-level rollback is still recorded"
            );
            None
        }
        Err(_) => {
            println!(
                "warning: could not create a filesystem snapshot; continuing with package-level rollback only"
            );
            None
        }
    }
}
