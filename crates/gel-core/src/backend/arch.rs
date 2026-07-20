//! Real Arch Linux package backend
//!
//! Compiled only with the `arch` feature. It drives `pacman` for native
//! (official-repo) packages and an AUR helper (`paru` or `yay`) for foreign
//! packages, through a [`CommandRunner`] seam so the built argv is unit testable
//! without touching the host.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    backend::{PackageBackend, file::FileBackend, service::ServiceBackend},
    error::GelError,
    state::SystemState,
    sys::{CommandRunner, SystemRunner},
};

/// AUR helpers to probe, in preference order
const AUR_HELPERS: [&str; 2] = ["paru", "yay"];

/// The real system package backend for Arch Linux
///
/// Native packages are managed with `pacman`; foreign (AUR) packages are managed
/// with the first available AUR helper. All process execution goes through a
/// [`CommandRunner`] so the built argv can be asserted in tests.
///
/// Package state is read by shelling out to `pacman -Qq*`. A future optimization
/// is to query the local database via the `alpm` library directly, avoiding a
/// subprocess, but the subprocess path is dependency-light and fine for phase 1.
#[derive(Debug, Default, Clone)]
pub struct ArchBackend<R: CommandRunner = SystemRunner> {
    runner: R,
}

impl ArchBackend<SystemRunner> {
    /// Construct a backend that runs real system commands
    #[must_use]
    pub const fn new() -> Self {
        Self {
            runner: SystemRunner,
        }
    }
}

impl<R: CommandRunner> ArchBackend<R> {
    /// Construct a backend backed by a custom command runner, for tests
    pub const fn with_runner(runner: R) -> Self {
        Self { runner }
    }

    /// Locate the first available AUR helper
    fn aur_helper(&self) -> Result<&'static str, GelError> {
        AUR_HELPERS
            .into_iter()
            .find(|helper| self.runner.is_available(helper))
            .ok_or_else(|| {
                GelError::Backend("no AUR helper found (install paru or yay)".to_owned())
            })
    }

    /// Query explicitly installed package names matching the given pacman args
    fn query_names(&self, args: &[&str]) -> Result<Vec<String>, GelError> {
        let output = self.runner.run("pacman", args)?;
        if !output.success {
            // `pacman -Q` with a filter exits non-zero when zero packages match,
            // printing nothing to either stream. That is a legitimately empty
            // result (e.g. no foreign packages), not a failure. A real error
            // (locked db, bad args) writes a diagnostic to stderr, so only treat
            // a non-empty stderr as an error. stderr is for server-side logs
            // only, never surfaced to users
            let stderr = output.stderr.trim();
            if stderr.is_empty() {
                return Ok(Vec::new());
            }
            return Err(GelError::Backend(format!("package query failed: {stderr}")));
        }
        Ok(parse_names(&output.stdout))
    }

    /// Run a command and map a non-zero exit into a backend error
    ///
    /// The captured stderr is included in the error string for server-side logs
    /// only; it must never be surfaced in user-facing output.
    fn run_checked(&self, program: &str, args: &[&str], action: &str) -> Result<(), GelError> {
        let output = self.runner.run(program, args)?;
        if output.success {
            return Ok(());
        }
        Err(GelError::Backend(format!(
            "{action} failed: {}",
            output.stderr.trim()
        )))
    }
}

/// Parse newline-separated package names, dropping blank lines
fn parse_names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Build install argv shared by pacman and AUR helpers
fn install_args(pkgs: &[String]) -> Vec<&str> {
    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(pkgs.iter().map(String::as_str));
    args
}

/// A temp path in the same directory as `target`, for an atomic write + rename
///
/// Placing the temp file beside the target keeps it on the same filesystem, so
/// the subsequent rename is a same-device atomic swap rather than a copy.
fn sibling_tmp(target: &Path) -> PathBuf {
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("gel");
    let tmp_name = format!(".{name}.gel.tmp");
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
        _ => PathBuf::from(tmp_name),
    }
}

/// Build removal argv shared by pacman and AUR helpers
fn remove_args(pkgs: &[String]) -> Vec<&str> {
    let mut args = vec!["-Rns", "--noconfirm"];
    args.extend(pkgs.iter().map(String::as_str));
    args
}

impl<R: CommandRunner> PackageBackend for ArchBackend<R> {
    fn query_explicit(&self) -> Result<SystemState, GelError> {
        // explicit native (official-repo) packages
        let native = self.query_names(&["-Qqen"])?;
        // explicit foreign (AUR / not in any sync db) packages
        let foreign = self.query_names(&["-Qqem"])?;
        // A package backend does not report managed files
        Ok(SystemState {
            native,
            foreign,
            files: Vec::new(),
            services: crate::state::ServiceIntent::default(),
            settings: crate::state::SettingsIntent::default(),
        })
    }

    fn install_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        if pkgs.is_empty() {
            return Ok(());
        }
        self.run_checked("pacman", &install_args(pkgs), "package installation")
    }

    fn remove_native(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        if pkgs.is_empty() {
            return Ok(());
        }
        self.run_checked("pacman", &remove_args(pkgs), "package removal")
    }

    fn install_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        if pkgs.is_empty() {
            return Ok(());
        }
        let helper = self.aur_helper()?;
        self.run_checked(helper, &install_args(pkgs), "package installation")
    }

    fn remove_foreign(&mut self, pkgs: &[String]) -> Result<(), GelError> {
        if pkgs.is_empty() {
            return Ok(());
        }
        let helper = self.aur_helper()?;
        self.run_checked(helper, &remove_args(pkgs), "package removal")
    }
}

impl<R: CommandRunner> FileBackend for ArchBackend<R> {
    fn read_file(&self, path: &str) -> Result<Option<String>, GelError> {
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            // a missing file is a legitimately empty read, not an error
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<(), GelError> {
        // create any missing parent directories so a managed file can be written
        // to a fresh location without a separate mkdir step
        let target = Path::new(path);
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        // write to a temp file in the SAME directory then rename into place, so a
        // crash mid-write cannot leave a partially written config at the final
        // path (rename is atomic on POSIX). The temp sibling shares the target's
        // filesystem, so the rename never falls back to a cross-device copy. This
        // mirrors the journal writer's atomic write
        let tmp = sibling_tmp(target);
        fs::write(&tmp, content)?;
        fs::rename(&tmp, target)?;
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<(), GelError> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            // removal is idempotent: an already-absent file is the desired end state
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl<R: CommandRunner> ServiceBackend for ArchBackend<R> {
    fn is_enabled(&self, unit: &str) -> Result<bool, GelError> {
        let output = self.runner.run("systemctl", &["is-enabled", unit])?;
        // `systemctl is-enabled` prints the enablement state to stdout and exits
        // non-zero for any state other than enabled (disabled, static, masked).
        // A non-zero exit that still names a state is a legitimate not-enabled
        // result, not a failure; only a non-zero exit that printed no state (for
        // example systemctl could not reach the manager, or the unit does not
        // exist) is a real error, mirroring the pacman query's stderr handling.
        // stderr is for server-side logs only, never surfaced to users
        let state = output.stdout.trim();
        if state.is_empty() {
            if output.success {
                return Ok(false);
            }
            let stderr = output.stderr.trim();
            return Err(GelError::Backend(format!(
                "unit enablement query failed: {stderr}"
            )));
        }
        // only a genuinely enabled unit counts as enabled; static, indirect,
        // masked and disabled units are all treated as not enabled for the
        // purposes of gel's explicit enable/disable intent
        Ok(matches!(state, "enabled" | "enabled-runtime"))
    }

    fn enable(&mut self, unit: &str) -> Result<(), GelError> {
        self.run_checked("systemctl", &["enable", unit], "unit enable")
    }

    fn disable(&mut self, unit: &str) -> Result<(), GelError> {
        self.run_checked("systemctl", &["disable", unit], "unit disable")
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;
    use crate::sys::CommandOutput;

    /// Ordered log of `(program, args)` pairs a mock runner was asked to run
    type CallLog = Rc<RefCell<Vec<(String, Vec<String>)>>>;

    /// A recording [`CommandRunner`] that never executes anything
    ///
    /// It captures every `(program, args)` pair so tests can assert the exact
    /// argv built, reports a configurable success/stderr, and answers
    /// availability from a fixed set of program names.
    #[derive(Clone, Default)]
    struct MockRunner {
        available: Vec<String>,
        output: CommandOutput,
        calls: CallLog,
    }

    impl MockRunner {
        fn new(success: bool, stdout: &str, stderr: &str, available: &[&str]) -> Self {
            Self {
                available: available.iter().map(|s| (*s).to_owned()).collect(),
                output: CommandOutput {
                    success,
                    stdout: stdout.to_owned(),
                    stderr: stderr.to_owned(),
                },
                calls: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.borrow().clone()
        }
    }

    impl CommandRunner for MockRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, GelError> {
            self.calls.borrow_mut().push((
                program.to_owned(),
                args.iter().map(|a| (*a).to_owned()).collect(),
            ));
            Ok(self.output.clone())
        }

        fn is_available(&self, program: &str) -> bool {
            self.available.iter().any(|p| p == program)
        }
    }

    fn owned(pkgs: &[&str]) -> Vec<String> {
        pkgs.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn query_explicit_parses_native_and_foreign() {
        let runner = MockRunner::new(true, "git\ntree\n", "", &[]);
        let backend = ArchBackend::with_runner(runner.clone());

        let state = backend.query_explicit().expect("query");

        assert_eq!(state.native, owned(&["git", "tree"]));
        assert_eq!(state.foreign, owned(&["git", "tree"]));
        assert_eq!(
            runner.calls(),
            vec![
                ("pacman".to_owned(), owned(&["-Qqen"])),
                ("pacman".to_owned(), owned(&["-Qqem"])),
            ]
        );
    }

    #[test]
    fn query_treats_nonzero_exit_with_empty_stderr_as_empty_set() {
        // pacman -Q with a filter exits non-zero and silent when nothing matches
        let runner = MockRunner::new(false, "", "", &[]);
        let backend = ArchBackend::with_runner(runner);

        let state = backend.query_explicit().expect("query");

        assert!(state.native.is_empty());
        assert!(state.foreign.is_empty());
    }

    #[test]
    fn query_errors_when_stderr_is_nonempty() {
        let runner = MockRunner::new(false, "", "error: failed to init db", &[]);
        let backend = ArchBackend::with_runner(runner);

        let result = backend.query_explicit();

        assert!(matches!(result, Err(GelError::Backend(_))));
    }

    #[test]
    fn install_native_builds_pacman_argv() {
        let runner = MockRunner::new(true, "", "", &[]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend.install_native(&owned(&["tree"])).expect("install");

        assert_eq!(
            runner.calls(),
            vec![(
                "pacman".to_owned(),
                owned(&["-S", "--needed", "--noconfirm", "tree"]),
            )]
        );
    }

    #[test]
    fn remove_native_builds_pacman_argv() {
        let runner = MockRunner::new(true, "", "", &[]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend.remove_native(&owned(&["tree"])).expect("remove");

        assert_eq!(
            runner.calls(),
            vec![("pacman".to_owned(), owned(&["-Rns", "--noconfirm", "tree"]),)]
        );
    }

    #[test]
    fn empty_package_slices_are_noops() {
        let runner = MockRunner::new(true, "", "", &["paru"]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend.install_native(&[]).expect("install");
        backend.remove_native(&[]).expect("remove");
        backend.install_foreign(&[]).expect("install foreign");
        backend.remove_foreign(&[]).expect("remove foreign");

        assert!(runner.calls().is_empty());
    }

    #[test]
    fn install_foreign_uses_first_available_helper() {
        // both present: paru is preferred over yay
        let runner = MockRunner::new(true, "", "", &["paru", "yay"]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend
            .install_foreign(&owned(&["aur-pkg"]))
            .expect("install");

        assert_eq!(
            runner.calls(),
            vec![(
                "paru".to_owned(),
                owned(&["-S", "--needed", "--noconfirm", "aur-pkg"]),
            )]
        );
    }

    #[test]
    fn install_foreign_falls_back_to_yay() {
        let runner = MockRunner::new(true, "", "", &["yay"]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend
            .install_foreign(&owned(&["aur-pkg"]))
            .expect("install");

        assert_eq!(runner.calls()[0].0, "yay".to_owned());
    }

    #[test]
    fn foreign_ops_error_without_helper() {
        let runner = MockRunner::new(true, "", "", &[]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        let install = backend.install_foreign(&owned(&["aur-pkg"]));
        let remove = backend.remove_foreign(&owned(&["aur-pkg"]));

        assert!(matches!(install, Err(GelError::Backend(_))));
        assert!(matches!(remove, Err(GelError::Backend(_))));
        // no helper resolved, so nothing was executed
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn nonzero_exit_becomes_backend_error() {
        let runner = MockRunner::new(false, "", "conflicting dependencies", &[]);
        let mut backend = ArchBackend::with_runner(runner);

        let result = backend.install_native(&owned(&["tree"]));

        assert!(matches!(result, Err(GelError::Backend(_))));
    }

    #[test]
    fn is_enabled_true_for_enabled_state() {
        // an enabled unit prints "enabled" and exits zero
        let runner = MockRunner::new(true, "enabled\n", "", &[]);
        let backend = ArchBackend::with_runner(runner.clone());

        assert!(backend.is_enabled("sshd.service").expect("query"));
        assert_eq!(
            runner.calls(),
            vec![(
                "systemctl".to_owned(),
                owned(&["is-enabled", "sshd.service"])
            )]
        );
    }

    #[test]
    fn is_enabled_false_for_disabled_nonzero_exit() {
        // `systemctl is-enabled` prints "disabled" and exits non-zero; that is a
        // legitimate not-enabled result, not a failure
        let runner = MockRunner::new(false, "disabled\n", "", &[]);
        let backend = ArchBackend::with_runner(runner);

        assert!(!backend.is_enabled("bluetooth.service").expect("query"));
    }

    #[test]
    fn is_enabled_false_for_masked_and_static_states() {
        // masked and static units are not enabled for gel's intent
        let masked = ArchBackend::with_runner(MockRunner::new(false, "masked\n", "", &[]));
        assert!(!masked.is_enabled("foo.service").expect("query"));

        let statik = ArchBackend::with_runner(MockRunner::new(true, "static\n", "", &[]));
        assert!(!statik.is_enabled("bar.service").expect("query"));
    }

    #[test]
    fn is_enabled_errors_when_nonzero_exit_prints_no_state() {
        // a non-zero exit with no state on stdout but a diagnostic on stderr is a
        // real failure (e.g. systemctl could not reach the manager)
        let runner = MockRunner::new(false, "", "Failed to connect to bus", &[]);
        let backend = ArchBackend::with_runner(runner);

        let result = backend.is_enabled("sshd.service");

        assert!(matches!(result, Err(GelError::Backend(_))));
    }

    #[test]
    fn enable_builds_systemctl_argv() {
        let runner = MockRunner::new(true, "", "", &[]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend.enable("sshd.service").expect("enable");

        assert_eq!(
            runner.calls(),
            vec![("systemctl".to_owned(), owned(&["enable", "sshd.service"]))]
        );
    }

    #[test]
    fn disable_builds_systemctl_argv() {
        let runner = MockRunner::new(true, "", "", &[]);
        let mut backend = ArchBackend::with_runner(runner.clone());

        backend.disable("bluetooth.service").expect("disable");

        assert_eq!(
            runner.calls(),
            vec![(
                "systemctl".to_owned(),
                owned(&["disable", "bluetooth.service"])
            )]
        );
    }

    #[test]
    fn enable_nonzero_exit_becomes_backend_error() {
        let runner = MockRunner::new(false, "", "unit not found", &[]);
        let mut backend = ArchBackend::with_runner(runner);

        let result = backend.enable("missing.service");

        assert!(matches!(result, Err(GelError::Backend(_))));
    }
}
