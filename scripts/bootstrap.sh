#!/usr/bin/env bash
#
# bootstrap.sh - get gel onto a fresh Arch machine (and optionally apply a config)
#
# Run this on a freshly installed, booted Arch system (base install done: disk,
# bootloader, network up). It installs gel's prerequisites, bootstraps an AUR
# helper if you need one, installs the `gel` binary, and can immediately converge
# the machine toward a config you provide. gel does NOT install the base OS; do
# that first with archinstall or a manual pacstrap.
#
# Usage:
#   ./bootstrap.sh                        # install gel only, print next steps
#   ./bootstrap.sh --config ./my-config   # eval a gel config crate and apply it
#   ./bootstrap.sh --artifact desired.json --apply   # apply a prebuilt artifact
#   ./bootstrap.sh --artifact https://host/desired.json --apply
#
# Flags:
#   --config <dir>       a gel config crate (dir with Cargo.toml) to eval + apply
#   --artifact <path|url> a prebuilt desired-state artifact to apply
#   --apply              actually run `gel apply` (otherwise just `gel diff`)
#   --prune              pass --prune to apply (remove undeclared explicit packages)
#   --no-paru            skip installing paru (native-only; no AUR packages)
#
# gel is opinionated: paru is its AUR helper, and it is installed by default.
# Run as a normal user with sudo rights (makepkg and cargo must not run as root).
set -euo pipefail

CONFIG_DIR=""
ARTIFACT=""
DO_APPLY=0
PRUNE=""
WITH_PARU=1
GEL_REPO="https://github.com/omnidotdev/gel"

green() { printf '\033[32m%s\033[0m\n' "$*"; }
red() { printf '\033[31m%s\033[0m\n' "$*"; }
step() { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --config) CONFIG_DIR="$2"; shift 2 ;;
    --artifact) ARTIFACT="$2"; shift 2 ;;
    --apply) DO_APPLY=1; shift ;;
    --prune) PRUNE="--prune"; shift ;;
    --no-paru) WITH_PARU=0; shift ;;
    *) red "unknown flag: $1"; exit 1 ;;
  esac
done

# --- preflight ---------------------------------------------------------------

command -v pacman >/dev/null 2>&1 || { red "this is not an Arch system (no pacman)"; exit 1; }
[ "$(id -u)" -ne 0 ] || { red "run as a normal user with sudo, not as root (makepkg/cargo refuse root)"; exit 1; }
command -v sudo >/dev/null 2>&1 || { red "sudo is required"; exit 1; }

# --- prerequisites -----------------------------------------------------------

step "installing prerequisites (base-devel, git)"
sudo pacman -S --needed --noconfirm base-devel git

has_paru() { command -v paru >/dev/null 2>&1; }

if [ "$WITH_PARU" -eq 1 ] && ! has_paru; then
  step "bootstrapping paru (gel's AUR helper)"
  tmp="$(mktemp -d)"
  git clone --depth 1 https://aur.archlinux.org/paru-bin.git "$tmp/paru-bin"
  ( cd "$tmp/paru-bin" && makepkg -si --noconfirm )
  rm -rf "$tmp"
fi

# --- install gel -------------------------------------------------------------

install_gel() {
  if command -v gel >/dev/null 2>&1; then green "gel already installed ($(command -v gel))"; return 0; fi

  # 1) AUR (once published): prebuilt binary package via paru
  if has_paru; then
    step "installing gel via paru (AUR)"
    if paru -S --needed --noconfirm omnidotdev-gel-bin; then return 0; fi
    red "AUR install failed (package may not be published yet), falling back"
  fi

  # 2) crates.io (once published)
  if command -v cargo >/dev/null 2>&1; then
    step "installing gel via cargo"
    if cargo install gel 2>/dev/null; then return 0; fi
    red "cargo install failed (crate may not be published yet), falling back to source"
  fi

  # 3) build from source (always works)
  step "building gel from source"
  command -v cargo >/dev/null 2>&1 || sudo pacman -S --needed --noconfirm rust
  tmp="$(mktemp -d)"
  git clone --depth 1 "$GEL_REPO" "$tmp/gel"
  ( cd "$tmp/gel" && cargo build --release --features arch )
  sudo install -Dm755 "$tmp/gel/target/release/gel" /usr/local/bin/gel
  rm -rf "$tmp"
}
install_gel
green "gel: $(gel --version 2>/dev/null || echo installed)"

# --- optional: converge toward a config --------------------------------------

resolve_artifact() {
  # produce an artifact path in $ART from --config (eval) or --artifact (path/url)
  ART=""
  if [ -n "$CONFIG_DIR" ]; then
    ART="$(mktemp --suffix=.json)"
    step "evaluating config crate: $CONFIG_DIR"
    gel eval "$CONFIG_DIR" --out "$ART"
  elif [ -n "$ARTIFACT" ]; then
    case "$ARTIFACT" in
      http://*|https://*) ART="$(mktemp --suffix=.json)"; step "fetching artifact"; curl -fsSL "$ARTIFACT" -o "$ART" ;;
      *) ART="$ARTIFACT" ;;
    esac
  fi
}

resolve_artifact
if [ -n "$ART" ]; then
  step "plan (gel diff)"
  gel diff --artifact "$ART" || true
  if [ "$DO_APPLY" -eq 1 ]; then
    step "converging (gel apply $PRUNE)"
    gel apply $PRUNE --artifact "$ART"
    green "machine converged. roll back the last apply with: gel rollback"
  else
    printf '\nreview the plan above, then apply with:\n  gel apply %s--artifact %s\n' "${PRUNE:+--prune }" "$ART"
  fi
else
  cat <<'NEXT'

gel is installed. Next steps:
  1. Author a config (copy the starter): the example config crate lives in the
     gel repo at examples/host-config. Edit its src/main.rs to declare your
     packages, files, services, and settings.
  2. Evaluate it to an artifact:   gel eval ./my-config --out desired.json
  3. Preview:                       gel diff --artifact desired.json
  4. Converge the machine:          gel apply --artifact desired.json
  5. Undo the last apply anytime:   gel rollback

Notes:
  - Foreign (AUR) packages use paru (installed by default; pass --no-paru to skip).
  - Snapshot rollback needs a btrfs root with snapper; otherwise gel uses its journal.
NEXT
fi
