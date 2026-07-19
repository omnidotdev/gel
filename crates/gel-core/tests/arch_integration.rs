//! Integration tests that exercise the REAL Arch backend against a real pacman
//!
//! These are gated behind the `arch` feature AND marked `#[ignore]`, so a normal
//! `cargo test` never runs them. They mutate real system package state and MUST
//! only be run inside the disposable container harness (`scripts/test-arch.sh`),
//! never against a developer host. See `crates/gel-core/TESTING.md`.
#![cfg(feature = "arch")]

use gel_core::{
    backend::{PackageBackend, arch::ArchBackend},
    snapshot::SnapshotProvider,
    snapshot_btrfs::BtrfsSnapshot,
};

/// A small package with no heavy dependencies, safe to install and remove
const TEST_PACKAGE: &str = "tree";

#[test]
#[ignore = "mutates real system package state; run only in the container harness"]
fn install_then_remove_native_roundtrips() {
    let mut backend = ArchBackend::new();

    // ensure a clean starting point; ignore a not-installed error
    let _ = backend.remove_native(&[TEST_PACKAGE.to_owned()]);

    backend
        .install_native(&[TEST_PACKAGE.to_owned()])
        .expect("install native test package");

    let after_install = backend.query_explicit().expect("query after install");
    assert!(
        after_install.native.iter().any(|p| p == TEST_PACKAGE),
        "expected {TEST_PACKAGE} to be explicitly installed"
    );

    backend
        .remove_native(&[TEST_PACKAGE.to_owned()])
        .expect("remove native test package");

    let after_remove = backend.query_explicit().expect("query after remove");
    assert!(
        !after_remove.native.iter().any(|p| p == TEST_PACKAGE),
        "expected {TEST_PACKAGE} to be removed"
    );
}

#[test]
#[ignore = "asserts the snapper-absent path; run only in the container harness"]
fn snapshot_returns_none_without_snapper() {
    let provider = BtrfsSnapshot::new();

    let result = provider
        .snapshot("gel-integration")
        .expect("snapshot probe");

    assert_eq!(result, None, "expected no snapshot when snapper is absent");
}
