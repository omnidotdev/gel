//! Transaction journal recording each apply so it can be rolled back
//!
//! This module is the one place in the core that touches the filesystem, and
//! only under a directory path the caller provides. It never reads a clock or
//! generates ids; the caller injects the transaction id and timestamp so the
//! journal stays deterministic and pure with respect to time and randomness.
//!
//! Ordering: entries carry no clock-derived value, so "latest" is defined by
//! the caller-injected `(timestamp, id)` pair compared lexicographically. The
//! caller is responsible for supplying monotonically increasing timestamps
//! (ties broken by id) if it wants chronological recency.

use std::{fs, path::Path};

use crate::{backend::PackageBackend, error::GelError, plan::Plan, snapshot::SnapshotId};

/// A single recorded transaction: the plan that was applied and its snapshot
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// Caller-injected transaction id, also used as the journal file name
    pub id: String,
    /// Caller-injected timestamp, an opaque string never read from a clock here
    pub timestamp: String,
    /// The plan that was applied in this transaction
    pub plan: Plan,
    /// The snapshot taken before the apply, when one was available
    pub snapshot: Option<SnapshotId>,
}

/// Write `entry` as JSON to `dir`, named by its id, creating `dir` if needed
///
/// This is the only function in the core that writes to the filesystem.
///
/// # Errors
///
/// Returns [`GelError`] if the directory cannot be created, serialization
/// fails, or the file cannot be written.
pub fn write_entry(dir: &Path, entry: &JournalEntry) -> Result<(), GelError> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", entry.id));
    let json = serde_json::to_string_pretty(entry)?;
    fs::write(path, json)?;
    Ok(())
}

/// Read the most recent entry from `dir`, or `None` when the journal is empty
///
/// Recency is the maximum of the caller-injected `(timestamp, id)` pair by
/// lexicographic order (see the module docs). A missing directory is treated
/// as an empty journal.
///
/// # Errors
///
/// Returns [`GelError`] if the directory cannot be read or an entry cannot be
/// deserialized.
pub fn read_latest(dir: &Path) -> Result<Option<JournalEntry>, GelError> {
    let latest = read_all(dir)?
        .into_iter()
        .max_by(|a, b| (&a.timestamp, &a.id).cmp(&(&b.timestamp, &b.id)));
    Ok(latest)
}

/// Roll back the latest journalled transaction by inverting its plan
///
/// The inverse of an apply reinstalls what was removed and removes what was
/// installed. This is a package-level undo; snapshot-based restore lives in a
/// later layer. A missing or empty journal is a no-op.
///
/// # Errors
///
/// Returns [`GelError`] if the journal cannot be read or a backend mutation
/// fails.
pub fn rollback_last(dir: &Path, b: &mut impl PackageBackend) -> Result<(), GelError> {
    let Some(entry) = read_latest(dir)? else {
        return Ok(());
    };
    let plan = &entry.plan;
    // invert: what was removed is reinstalled, what was installed is removed
    if !plan.native_remove.is_empty() {
        b.install_native(&plan.native_remove)?;
    }
    if !plan.foreign_remove.is_empty() {
        b.install_foreign(&plan.foreign_remove)?;
    }
    if !plan.native_install.is_empty() {
        b.remove_native(&plan.native_install)?;
    }
    if !plan.foreign_install.is_empty() {
        b.remove_foreign(&plan.foreign_install)?;
    }
    Ok(())
}

/// Read and deserialize every `.json` entry in `dir`
///
/// A missing directory yields an empty vector rather than an error, so an
/// unwritten journal reads cleanly.
fn read_all(dir: &Path) -> Result<Vec<JournalEntry>, GelError> {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut entries = Vec::new();
    for item in read_dir {
        let path = item?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let json = fs::read_to_string(&path)?;
        let entry: JournalEntry = serde_json::from_str(&json)?;
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        PackageBackend,
        fake::{Call, FakeBackend},
    };

    fn entry(id: &str, timestamp: &str, plan: Plan) -> JournalEntry {
        JournalEntry {
            id: id.to_owned(),
            timestamp: timestamp.to_owned(),
            plan,
            snapshot: Some(SnapshotId("snap-1".to_owned())),
        }
    }

    #[test]
    fn write_then_read_latest_roundtrips_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let written = entry(
            "tx-1",
            "2026-07-19T00:00:00Z",
            Plan {
                native_install: vec!["ripgrep".to_owned()],
                ..Plan::default()
            },
        );

        write_entry(dir.path(), &written).expect("write");
        let read = read_latest(dir.path()).expect("read").expect("some entry");

        assert_eq!(read, written);
    }

    #[test]
    fn read_latest_returns_newer_of_two_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let older = entry("tx-1", "2026-07-19T00:00:00Z", Plan::default());
        let newer = entry(
            "tx-2",
            "2026-07-19T01:00:00Z",
            Plan {
                native_install: vec!["fd".to_owned()],
                ..Plan::default()
            },
        );

        write_entry(dir.path(), &older).expect("write older");
        write_entry(dir.path(), &newer).expect("write newer");

        let latest = read_latest(dir.path()).expect("read").expect("some entry");
        assert_eq!(latest, newer);
    }

    #[test]
    fn read_latest_on_missing_journal_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");

        let latest = read_latest(&missing).expect("read");

        assert_eq!(latest, None);
    }

    #[test]
    fn rollback_last_inverts_the_recorded_plan() {
        // the recorded transaction installed ripgrep/yay and removed vim/old-aur,
        // so the POST-apply backend has ripgrep + yay but not vim + old-aur
        let dir = tempfile::tempdir().expect("tempdir");
        let mut backend = FakeBackend::with_explicit(&["git", "ripgrep"], &["yay"]);
        let recorded = entry(
            "tx-1",
            "2026-07-19T00:00:00Z",
            Plan {
                native_install: vec!["ripgrep".to_owned()],
                native_remove: vec!["vim".to_owned()],
                foreign_install: vec!["yay".to_owned()],
                foreign_remove: vec!["old-aur".to_owned()],
            },
        );
        write_entry(dir.path(), &recorded).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // inverse: vim + old-aur reinstalled, ripgrep + yay removed
        let state = backend.query_explicit().expect("query");
        assert!(state.native.contains(&"vim".to_owned()));
        assert!(!state.native.contains(&"ripgrep".to_owned()));
        assert!(state.foreign.contains(&"old-aur".to_owned()));
        assert!(!state.foreign.contains(&"yay".to_owned()));

        // the call log reflects the inverse operations in order
        assert_eq!(
            backend.calls(),
            &[
                Call::InstallNative(vec!["vim".to_owned()]),
                Call::InstallForeign(vec!["old-aur".to_owned()]),
                Call::RemoveNative(vec!["ripgrep".to_owned()]),
                Call::RemoveForeign(vec!["yay".to_owned()]),
            ]
        );
    }
}
