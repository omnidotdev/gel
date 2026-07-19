//! Filesystem snapshot abstraction taken before a mutating apply
//!
//! Snapshot creation is behind a trait so the pure core stays free of any real
//! filesystem tooling; the concrete btrfs implementation lives outside this
//! crate. A no-op provider is supplied for filesystems without snapshots.

use crate::error::GelError;

/// Opaque handle to a created snapshot
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotId(pub String);

/// Takes a filesystem snapshot before a mutating apply so it can be rolled back
pub trait SnapshotProvider {
    /// Create a snapshot tagged with the transaction tag; None when snapshots are unavailable
    ///
    /// # Errors
    ///
    /// Returns [`GelError`] if the provider fails to create a snapshot.
    fn snapshot(&self, tag: &str) -> Result<Option<SnapshotId>, GelError>;
}

/// A provider that takes no snapshots, used when the filesystem does not support them
pub struct NoopSnapshot;

impl SnapshotProvider for NoopSnapshot {
    fn snapshot(&self, _tag: &str) -> Result<Option<SnapshotId>, GelError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_snapshot_returns_none() {
        let provider = NoopSnapshot;

        let result = provider.snapshot("tx-1").expect("snapshot");

        assert_eq!(result, None);
    }
}
