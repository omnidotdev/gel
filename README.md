<div align="center">
  <img src="/assets/logo.png" width="100" />

  <h1 align="center">gel</h1>

[Website](https://gel.omni.dev) | [Docs](https://docs.omni.dev/armory/gel) | [Feedback](https://backfeed.omni.dev/workspaces/omni/projects/gel) | [Discord](https://discord.gg/omnidotdev) | [X](https://x.com/omnidotdev) | [Threads](https://www.threads.com/@omnidotdev)

</div>

**gel** is a command-line application built with Rust.

## Installation

| Platform | Channel | Command / Link |
| --- | --- | --- |
| All | [GitHub Releases](https://github.com/omnidotdev/gel/releases) | Download from releases page |
| All | [crates.io](https://crates.io/crates/gel) | `cargo install gel` |
| macOS / Linux | [Homebrew](https://github.com/omnidotdev/homebrew-tap/blob/master/Formula/gel.rb) | `brew install omnidotdev/tap/gel` |
| Arch Linux | [AUR](https://aur.archlinux.org/packages/omnidotdev-gel) / [AUR (bin)](https://aur.archlinux.org/packages/omnidotdev-gel-bin) | `paru -S omnidotdev-gel` or `paru -S omnidotdev-gel-bin` |

### Build from source

```bash
git clone https://github.com/omnidotdev/gel
cd gel
cargo build --release
# Binary will be at target/release/gel
```

## Quick Start

```sh
gel --help
```

## Usage

gel manages the explicitly installed packages on an Arch machine declaratively.
You describe the machine you want in a small Rust config crate, then converge the
system toward it. The workflow is an **eval/apply split**: evaluating your config
into an artifact is pure and safe, while touching the system is a separate step.

### 1. Author a config

A gel config is an ordinary Rust program that builds a `System` and prints the
resulting desired state as JSON. See [`examples/host-config`](examples/host-config)
for a complete crate:

```rust
use gel_core::config::System;

fn main() {
    let system = System::new()
        .native(["git", "ripgrep", "fd", "bat"]) // official-repo packages (pacman)
        .foreign(["paru"]) // AUR packages (AUR helper)
        // a managed file: gel writes this content verbatim on apply
        .file("/tmp/gel-demo.conf", "greeting = hello\n")
        .enable("systemd-timesyncd.service") // ensure a unit is enabled
        .disable("bluetooth.service") // ensure a unit is disabled
        .hostname("gelbox") // manage the hostname
        .timezone("Etc/UTC") // manage the timezone
        .locale("en_US.UTF-8"); // manage the locale
    print!("{}", serde_json::to_string(&system.build()).expect("serialize"));
}
```

`System::build()` sorts and deduplicates each package list, so authoring order
does not matter. Managed files are likewise sorted by path and deduplicated by
path; declaring the same path twice is last-wins (the final content is kept).
Service enable/disable lists are sorted and deduplicated too, with disable
winning any enable/disable conflict for the same unit.

### Managed files

`System::file(path, content)` declares a file whose full content gel owns. On
`apply`, gel writes each declared file whose content differs from what is on disk
(creating parent directories as needed) and records the prior content, so
`rollback` restores it: a file gel created is deleted, and a file gel overwrote
is returned to its exact prior bytes. Writes are atomic (written to a temp
sibling then renamed into place), so a crash mid-write cannot leave a partially
written config.

Not yet handled (planned for later phases): file pruning (removing a file just
because it left the config), file permissions and ownership, content templating,
and drift detection against package-provided default config files. gel manages
only the files you declare, by full content.

### Managed services

`System::enable(unit)` and `System::disable(unit)` declare explicit intent over
systemd units. This is deliberately not full-set convergence: gel only ever
touches the units you name, so a unit absent from both lists is left exactly as
it is and gel never disables a unit it was not told about.

`build()` sorts and deduplicates each list. When the same unit is both enabled
and disabled, **disable wins**: the unit is dropped from the enable list, so the
built state never names a unit in both and an ambiguous declaration can never
leave a unit enabled. On `apply`, gel enables each declared-enable unit that is
currently disabled and disables each declared-disable unit that is currently
enabled (units already in the desired state are skipped). It records each touched
unit's prior enabled state, so `rollback` restores it: a unit gel enabled is
disabled again, and a unit gel disabled is re-enabled. `is_enabled` treats
systemd's `enabled-runtime` state as enabled.

Not yet handled (planned for later phases): unit masking, drop-in override files,
`--user` units, runtime start/stop state (gel manages enablement, not whether a
unit is currently running), and templated/instanced units. gel manages only the
enable/disable state of the units you declare.

### Managed system settings

`System::hostname(name)`, `System::timezone(tz)`, and `System::locale(locale)`
declare explicit intent over three global system settings, converged with
`hostnamectl`, `timedatectl`, and `localectl` respectively. Like services, this is
explicit intent, not full-set convergence: a setting you do not declare is left
`None` and gel never touches it. Each setter is last-call-wins per field, so the
final value for a given setting is the one lowered into the state.

On `apply`, gel reads each declared setting's current value and changes only those
that differ, recording each changed setting's prior value. On `rollback`, gel
restores that prior value; a setting that had no prior value (it was previously
unset) is left as-is rather than cleared, so rollback never invents an empty
hostname, timezone, or locale.

Not yet handled (planned for later phases): users and groups, `sysctl` kernel
parameters, kernel modules, the console keymap and font, and locale categories
beyond `LANG` (for example `LC_TIME` or `LC_MESSAGES`). gel manages only the
hostname, timezone, and single `LANG` locale you declare.

### 2. Evaluate (pure, always available)

`gel eval` compiles and runs the config crate and writes the desired state to an
artifact. This runs cargo and writes a file; it never touches packages, so it
works in any build:

```sh
gel eval examples/host-config            # writes the default artifact
gel eval examples/host-config --out /tmp/desired.json
```

### 3. Diff and apply (require an Arch build)

```sh
gel diff                 # preview the plan: packages, files, service, and setting changes (read-only)
gel apply                # additive converge: install what is missing, write files, enable/disable units, change settings
gel apply --prune        # also remove explicit packages absent from the config
gel import               # capture the current explicit packages as a desired state
gel rollback             # invert the most recent apply (packages + files + services + settings)
```

`diff` also lists the managed files whose content would change (`~N files to
write` plus each target path), the service actions (`+N to enable, -N to disable`
plus each unit), and the setting changes (`~N settings to change` plus each
setting), and stays read-only. `apply` takes a filesystem snapshot first (via
snapper on btrfs; it degrades to a warning when snapshots are unavailable), prints
the plan, converges packages, writes managed files, applies the service
enable/disable actions, changes the settings, and records a transaction in the
journal so it can be rolled back. `rollback` reverses packages, restores managed
files to their prior content, restores each touched unit's prior enabled state,
and restores each changed setting's prior value (a setting that was previously
unset is left as-is); a full snapshot-based filesystem restore is planned for a
later phase.

### The `arch` feature

The system-touching commands (`import`, `diff`, `apply`, `rollback`) drive real
`pacman`/AUR-helper/snapper tooling and are only compiled with the `arch` feature:

```sh
cargo build --release --features arch    # binary with the real Arch backend
```

The default build is pure: those subcommands fast-fail with a clear rebuild
message and nothing touches the host, while `gel eval` still works.

### Locations

gel keeps its state under `${XDG_STATE_HOME:-~/.local/state}/gel`:

| Path | Purpose |
| --- | --- |
| `<state-dir>/desired.json` | default desired-state artifact (override with `--out`/`--artifact`) |
| `<state-dir>/journal/` | transaction journal used by `rollback` |

## Development

### Prerequisites

- [Rust](https://rustup.rs) 1.85+
- [Bun](https://bun.sh) 1.0+

### Commands

```sh
cargo build          # Build
cargo run -- --help  # Run
cargo test           # Test
cargo clippy         # Lint
```

### Version Syncing

This project uses a dual-package setup (Rust crate + npm package) with automated version synchronization:

- **Source of truth**: `package.json` holds the canonical version, and is used for Changesets
- **Sync script**: `scripts/syncVersion.ts` propagates the version to `Cargo.toml`
- **Changesets**: Manages version bumps and changelog generation

The sync script runs automatically during the release process via the `version` npm script:

```sh
bun run version  # syncs `package.json` version → `Cargo.toml`
```

### CI/CD

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `test.yml` | Push/PR to `master` | Runs fmt, clippy, and tests |
| `sync.yml` | PR to `master` | Validates version sync, fmt, clippy, test, build |
| `release.yml` | Push to `master` | Creates releases via Changesets, builds multi-platform binaries |

### Release Process

1. Create a changeset: `bun changeset`
2. Push to `master`
3. Changesets action creates a "Version Packages" PR
4. Merge the PR to trigger a release with binaries for:
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
5. **Manually** publish to crates.io: `cargo publish`

## Ecosystem

- **[Omni CLI](https://github.com/omnidotdev/cli)**: Agentic CLI for the Omni ecosystem
- **[Omni Terminal](https://github.com/omnidotdev/terminal)**: GPU-accelerated terminal emulator built to run everywhere

## License

The code in this repository is licensed under Apache 2.0, &copy; [Omni LLC](https://omni.dev). See [LICENSE.md](LICENSE.md) for more information.
