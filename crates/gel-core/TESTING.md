# Testing gel-core

## Unit and default tests (pure, always safe)

The default build is pure and free of OS side effects. Run everything with:

```bash
cargo test --workspace
```

This never requires the `arch` feature, root, a container, or network access.

The real Arch backend (`ArchBackend`) and btrfs snapshot provider
(`BtrfsSnapshot`) route all process execution through a `CommandRunner` seam, so
their argv construction and control flow are unit tested with a recording mock
that executes nothing. Those unit tests live in the modules themselves and run
safely on any host with:

```bash
cargo test -p gel-core --features arch --lib
```

## Real Arch backend integration tests (container only)

`crates/gel-core/tests/arch_integration.rs` exercises the REAL backend against a
real `pacman`. Every test there is gated behind the `arch` feature AND marked
`#[ignore]`, so a normal `cargo test` never runs it. These tests mutate real
system package state and MUST NOT be run against a developer host.

They run inside a disposable Arch container:

```bash
scripts/test-arch.sh
```

The script detects `docker` or `podman`, builds `misc/test/Dockerfile` (a
throwaway `archlinux:base-devel` image with the Rust toolchain), and runs:

```bash
cargo test --locked -p gel-core --features arch --test arch_integration -- --include-ignored
```

inside the container. The repo is mounted read-only; the build target and cargo
caches live in named volumes so nothing is written into the host checkout.

Coverage inside the container:

- native pacman path: install `tree`, assert it appears in
  `query_explicit().native`, remove it, assert it is gone
- snapshot: assert `BtrfsSnapshot::snapshot` returns `Ok(None)` when `snapper` is
  absent (safe and containerable)

### Known gaps / manual steps

- AUR helper path: pacman requires root, so the container runs the tests as
  root; AUR helpers (`paru`/`yay`) refuse to run as root. The image already
  provisions a passwordless-sudo `builder` user for a future in-container AUR
  test, but the foreign (AUR) install/remove path is currently covered only by
  the mock-runner unit tests, not end to end.
- btrfs snapshot creation: actually creating a snapshot requires a btrfs
  filesystem with snapper configured, which a container does not provide. Verify
  it manually in a VM or on a btrfs host:
  ```bash
  # in a throwaway btrfs VM with snapper configured for the target subvolume
  cargo test -p gel-core --features arch --test arch_integration -- --include-ignored
  # then, to prove real creation, from a small harness or gel itself:
  #   BtrfsSnapshot::new().snapshot("manual-check") -> Ok(Some(SnapshotId(<n>)))
  # confirm with: snapper list
  ```
