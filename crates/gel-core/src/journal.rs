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

use crate::{
    backend::{PackageBackend, file::FileBackend},
    error::GelError,
    plan::Plan,
    snapshot::SnapshotId,
};

/// The content of a managed file before an apply overwrote or created it
///
/// Recorded per written file so rollback can restore the exact prior bytes, or
/// delete a file the transaction created. `prior` is `None` when the file did
/// not exist before the apply (the transaction created it), and `Some(old)`
/// when the apply replaced pre-existing content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileBackup {
    /// Absolute path of the managed file that was written
    pub path: String,
    /// The file's content before the apply, or `None` if it did not exist
    pub prior: Option<String>,
}

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
    /// Prior content of every managed file this transaction wrote
    ///
    /// Defaulted so journal entries written before file management (which had no
    /// such field) still deserialize cleanly as having backed up no files.
    #[serde(default)]
    pub file_backups: Vec<FileBackup>,
}

/// Write `entry` as JSON to `dir`, named by its id, creating `dir` if needed
///
/// This is the only function in the core that writes to the filesystem. The
/// write is atomic: the JSON is written to a temporary sibling file and then
/// renamed into place, so a crash mid-write cannot leave a partially written,
/// unparseable entry at the final path (rename is atomic on POSIX).
///
/// # Errors
///
/// Returns [`GelError`] if the directory cannot be created, serialization
/// fails, or the file cannot be written or renamed.
pub fn write_entry(dir: &Path, entry: &JournalEntry) -> Result<(), GelError> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", entry.id));
    let tmp = dir.join(format!("{}.json.tmp", entry.id));
    let json = serde_json::to_string_pretty(entry)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
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
/// installed, then restores managed files to their pre-apply content. Each file
/// backup is undone by writing back its prior content, or by deleting the file
/// when the transaction created it (a `None` prior). A missing or empty journal
/// is a no-op.
///
/// # Errors
///
/// Returns [`GelError`] if the journal cannot be read or a backend mutation
/// fails.
pub fn rollback_last<B>(dir: &Path, b: &mut B) -> Result<(), GelError>
where
    B: PackageBackend + FileBackend,
{
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
    // restore files after packages: a backup with prior content is written
    // back, one with no prior was created by this transaction so it is deleted
    for backup in &entry.file_backups {
        match &backup.prior {
            Some(old) => b.write_file(&backup.path, old)?,
            None => b.remove_file(&backup.path)?,
        }
    }
    Ok(())
}

/// Read and deserialize every `.json` entry in `dir`
///
/// A missing directory yields an empty vector rather than an error, so an
/// unwritten journal reads cleanly. Files that fail to deserialize are skipped
/// rather than failing the whole read, so a single corrupt entry (for example
/// a torn write from an earlier crash) cannot block rollback of the good
/// entries. Non-`.json` files are ignored.
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
        // skip entries that fail to parse so one corrupt file cannot block the rest
        if let Ok(entry) = serde_json::from_str::<JournalEntry>(&json) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        PackageBackend,
        fake::{Call, FakeBackend},
        file::FileBackend,
    };

    fn entry(id: &str, timestamp: &str, plan: Plan) -> JournalEntry {
        JournalEntry {
            id: id.to_owned(),
            timestamp: timestamp.to_owned(),
            plan,
            snapshot: Some(SnapshotId("snap-1".to_owned())),
            file_backups: Vec::new(),
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
    fn read_latest_skips_corrupt_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid = entry("tx-1", "2026-07-19T00:00:00Z", Plan::default());
        write_entry(dir.path(), &valid).expect("write");
        // a corrupt sibling json file must not block reading the good entry
        std::fs::write(dir.path().join("tx-2.json"), b"{ not valid json").expect("write corrupt");

        let latest = read_latest(dir.path()).expect("read").expect("some entry");

        assert_eq!(latest, valid);
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
                ..Plan::default()
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

    #[test]
    fn rollback_last_restores_changed_files_and_deletes_created_ones() {
        // the recorded transaction created /etc/new (no prior) and overwrote
        // /etc/changed (prior "old"), so the post-apply backend holds both
        let dir = tempfile::tempdir().expect("tempdir");
        let mut backend =
            FakeBackend::with_files(&[("/etc/new", "created\n"), ("/etc/changed", "new\n")]);
        let recorded = JournalEntry {
            id: "tx-1".to_owned(),
            timestamp: "2026-07-19T00:00:00Z".to_owned(),
            plan: Plan::default(),
            snapshot: None,
            file_backups: vec![
                FileBackup {
                    path: "/etc/changed".to_owned(),
                    prior: Some("old\n".to_owned()),
                },
                FileBackup {
                    path: "/etc/new".to_owned(),
                    prior: None,
                },
            ],
        };
        write_entry(dir.path(), &recorded).expect("write");

        rollback_last(dir.path(), &mut backend).expect("rollback");

        // a file the transaction created is deleted
        assert_eq!(backend.read_file("/etc/new").expect("read"), None);
        // a file the transaction changed is restored to its prior content
        assert_eq!(
            backend.read_file("/etc/changed").expect("read"),
            Some("old\n".to_owned())
        );
        // the restore is a write, the deletion a remove, in backup order
        assert_eq!(
            backend.calls(),
            &[
                Call::WriteFile("/etc/changed".to_owned()),
                Call::RemoveFile("/etc/new".to_owned()),
            ]
        );
    }
}
