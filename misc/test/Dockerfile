# Disposable Arch container for exercising the REAL gel arch-feature backend
#
# This image is throwaway: it exists only so `scripts/test-arch.sh` can run the
# `#[ignore]`d integration tests against a real pacman WITHOUT touching a
# developer host. Nothing here is a production artifact.
FROM archlinux:base-devel

# Refresh the package databases and base system so installs resolve, then add
# the toolchain we need. Clean the package cache to keep the image small.
RUN pacman -Syu --noconfirm \
    && pacman -S --needed --noconfirm rustup git sudo \
    && pacman -Scc --noconfirm

# Pin the Rust toolchain to the repo channel (stable, per rust-toolchain.toml)
RUN rustup default stable

# Non-root build user with passwordless sudo, kept for the AUR-helper path
# (paru/yay refuse to run as root). The current integration tests cover only the
# native pacman path plus the snapper-absent snapshot probe, and run as root
# (pacman needs root); wiring an AUR helper in-container is a documented
# follow-up. See crates/gel-core/TESTING.md
RUN useradd -m -G wheel builder \
    && install -d -m 0750 /etc/sudoers.d \
    && echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder \
    && chmod 0440 /etc/sudoers.d/builder

WORKDIR /work
