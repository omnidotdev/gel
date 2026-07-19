#!/usr/bin/env bash
# Build a throwaway Arch container and run the arch-feature integration tests
# inside it, so the REAL pacman-backed gel backend is exercised WITHOUT ever
# touching the host system. See crates/gel-core/TESTING.md
set -euo pipefail

# Resolve the repo root from this script's location, independent of cwd
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Detect a container runtime; prefer docker, fall back to podman
if command -v docker >/dev/null 2>&1; then
    RUNTIME=docker
elif command -v podman >/dev/null 2>&1; then
    RUNTIME=podman
else
    echo "error: neither docker nor podman found; cannot run the Arch test harness" >&2
    exit 1
fi

IMAGE=gel-arch-test

echo "==> Building $IMAGE with $RUNTIME"
"$RUNTIME" build -t "$IMAGE" -f "$REPO_ROOT/misc/test/Dockerfile" "$REPO_ROOT/misc/test"

echo "==> Running arch integration tests inside $IMAGE"
# The repo is mounted read-only; compilation and cargo caches go to named
# volumes so the run is fast on repeat and never writes into the host checkout.
# --locked guarantees Cargo.lock is not rewritten against the read-only mount.
"$RUNTIME" run --rm \
    -v "$REPO_ROOT":/work:ro \
    -v gel-arch-target:/work/target \
    -v gel-arch-cargo:/root/.cargo/registry \
    -w /work \
    "$IMAGE" \
    cargo test --locked -p gel-core --features arch --test arch_integration -- --include-ignored
