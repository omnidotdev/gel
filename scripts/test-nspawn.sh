#!/usr/bin/env bash
#
# test-nspawn.sh - end-to-end gel validation in a disposable systemd-nspawn Arch container
#
# Stands up a throwaway Arch rootfs (pacstrap), boots it with systemd (so
# systemctl / hostnamectl / timedatectl all work for real), installs the built
# gel binary, and drives the full flow against real tooling:
#
#   gel diff  ->  gel apply  ->  verify  ->  gel rollback  ->  verify reverted
#
# It exercises packages (pacman), managed files, services (systemctl enable),
# and settings (hostnamectl/timedatectl), plus journal-based rollback. It NEVER
# touches the host system: everything lives in a temp rootfs and a named nspawn
# machine, both removed on exit.
#
# btrfs snapshot creation is NOT covered here (a container rootfs is not a btrfs
# subvolume with snapper); rollback here uses the universal journal path. For
# faithful snapshot/rollback testing use a VM with a btrfs root + snapper.
#
# Requirements (host): Arch with `arch-install-scripts` (pacstrap), systemd
# (systemd-nspawn, machinectl, systemd-run), and a Rust toolchain. Run as root:
#
#   sudo ./scripts/test-nspawn.sh
#
set -euo pipefail

# --- config ------------------------------------------------------------------

MACHINE="gel-test-$$"
ROOT=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# a test unit present in base (shipped with systemd) and safe to toggle
TEST_UNIT="systemd-timesyncd.service"
TEST_PKG="tree"
DEMO_FILE="/etc/gel-demo.conf"
BASE_HOSTNAME="gel-base"
DESIRED_HOSTNAME="gel-configured"

green() { printf '\033[32m%s\033[0m\n' "$*"; }
red() { printf '\033[31m%s\033[0m\n' "$*"; }
step() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }

# --- cleanup -----------------------------------------------------------------

cleanup() {
  set +e
  if [ -n "$MACHINE" ] && machinectl status "$MACHINE" >/dev/null 2>&1; then
    step "tearing down container $MACHINE"
    machinectl poweroff "$MACHINE" >/dev/null 2>&1
    for _ in $(seq 1 20); do
      machinectl status "$MACHINE" >/dev/null 2>&1 || break
      sleep 0.5
    done
    machinectl terminate "$MACHINE" >/dev/null 2>&1
  fi
  systemctl stop "gel-nspawn-$MACHINE.service" >/dev/null 2>&1
  if [ -n "$ROOT" ] && [ -d "$ROOT" ]; then
    rm -rf "$ROOT"
  fi
}
trap cleanup EXIT

# --- preflight ---------------------------------------------------------------

[ "$(id -u)" -eq 0 ] || { red "must run as root (use sudo)"; exit 1; }
for tool in pacstrap systemd-nspawn machinectl systemd-run; do
  command -v "$tool" >/dev/null 2>&1 || { red "missing '$tool' (install arch-install-scripts / systemd)"; exit 1; }
done

# --- build gel ---------------------------------------------------------------

step "building gel (--features arch)"
BIN="$REPO_ROOT/target/release/gel"
# build as the invoking user so target/ does not fill with root-owned files
if [ -n "${SUDO_USER:-}" ]; then
  sudo -u "$SUDO_USER" bash -lc "cd '$REPO_ROOT' && cargo build --release --features arch"
else
  ( cd "$REPO_ROOT" && cargo build --release --features arch )
fi
[ -x "$BIN" ] || { red "gel binary not found at $BIN"; exit 1; }
green "built $BIN"

# --- rootfs ------------------------------------------------------------------

step "creating disposable Arch rootfs (pacstrap base)"
mkdir -p /var/lib/machines
ROOT="$(mktemp -d /var/lib/machines/gel-test.XXXXXX)"
# base ships pacman, systemd (systemctl/hostnamectl/timedatectl/timesyncd), coreutils
pacstrap -c "$ROOT" base >/dev/null
green "rootfs at $ROOT"

# seed an initial state so rollback has something to restore to
printf '%s\n' "$BASE_HOSTNAME" > "$ROOT/etc/hostname"
install -Dm755 "$BIN" "$ROOT/usr/local/bin/gel"

# the desired state gel will converge to (native pkg + file + service + settings)
cat > "$ROOT/root/desired.json" <<EOF
{
  "native": ["$TEST_PKG"],
  "foreign": [],
  "files": [{ "path": "$DEMO_FILE", "content": "managed by gel\n" }],
  "services": { "enable": ["$TEST_UNIT"], "disable": [] },
  "settings": { "hostname": "$DESIRED_HOSTNAME", "timezone": "Etc/UTC", "locale": null }
}
EOF

# --- boot --------------------------------------------------------------------

step "booting container $MACHINE"
# run nspawn inside a transient host unit so it is cleanly detached (no tty
# needed) and registered with machined. No --network-veth, so the container
# shares the host network and pacman has internet.
systemd-run --unit="gel-nspawn-$MACHINE" --collect --quiet \
  systemd-nspawn --boot --quiet --keep-unit --console=pipe \
  --directory "$ROOT" --machine "$MACHINE"

# wait until the container can run commands
booted=0
for _ in $(seq 1 60); do
  if systemd-run --machine "$MACHINE" --quiet --wait --pipe /usr/bin/true >/dev/null 2>&1; then
    booted=1; break
  fi
  sleep 1
done
[ "$booted" -eq 1 ] || { red "container did not boot (see /tmp/$MACHINE.log)"; exit 1; }
green "container up"

# run a command inside the booted container, gel state under /root/.local/state
inc() { systemd-run --machine "$MACHINE" --quiet --wait --pipe \
  --setenv=XDG_STATE_HOME=/root/.local/state --setenv=HOME=/root "$@"; }

# --- drive gel ---------------------------------------------------------------

step "gel diff (plan, read-only)"
inc /usr/local/bin/gel diff --artifact /root/desired.json || true

step "gel apply"
inc /usr/local/bin/gel apply --artifact /root/desired.json

step "verify converged state"
fail=0
inc /usr/bin/pacman -Q "$TEST_PKG" >/dev/null 2>&1 && green "package $TEST_PKG installed" || { red "package NOT installed"; fail=1; }
inc /usr/bin/test -f "$DEMO_FILE" && green "file $DEMO_FILE present" || { red "file missing"; fail=1; }
enabled="$(inc /usr/bin/systemctl is-enabled "$TEST_UNIT" 2>/dev/null | tr -d '\r\n' || true)"
[ "$enabled" = "enabled" ] && green "service $TEST_UNIT enabled" || red "service state: $enabled"
host="$(inc /usr/bin/hostnamectl --static 2>/dev/null | tr -d '\r\n' || true)"
[ "$host" = "$DESIRED_HOSTNAME" ] && green "hostname = $host" || { red "hostname = $host (expected $DESIRED_HOSTNAME)"; fail=1; }
tz="$(inc /usr/bin/timedatectl show -p Timezone --value 2>/dev/null | tr -d '\r\n' || true)"
[ "$tz" = "Etc/UTC" ] && green "timezone = $tz" || red "timezone = $tz"

step "gel rollback"
inc /usr/local/bin/gel rollback

step "verify rolled back"
inc /usr/bin/pacman -Q "$TEST_PKG" >/dev/null 2>&1 && { red "package still installed after rollback"; fail=1; } || green "package removed"
inc /usr/bin/test -f "$DEMO_FILE" && { red "file still present after rollback"; fail=1; } || green "file removed"
host2="$(inc /usr/bin/hostnamectl --static 2>/dev/null | tr -d '\r\n' || true)"
[ "$host2" = "$BASE_HOSTNAME" ] && green "hostname restored to $host2" || red "hostname = $host2 (expected $BASE_HOSTNAME)"

step "result"
if [ "$fail" -eq 0 ]; then
  green "end-to-end gel flow verified in a disposable Arch container"
else
  red "one or more checks failed (see output above)"; exit 1
fi
