//! Real btrfs snapshot provider backed by snapper
//!
//! Compiled only with the `arch` feature. When `snapper` is not installed the
//! provider reports no snapshot (`Ok(None)`) so the caller can fall back to the
//! no-op provider; the pure core never depends on snapshot tooling. Execution
//! goes through the [`CommandRunner`] seam so the built argv and the
//! snapper-absent path are unit testable without running anything.

use crate::{
    error::GelError,
    snapshot::{SnapshotId, SnapshotProvider},
    sys::{CommandRunner, SystemRunner},
};

/// Creates pre-apply snapshots with snapper on a btrfs filesystem
#[derive(Debug, Default, Clone)]
pub struct BtrfsSnapshot<R: CommandRunner = SystemRunner> {
    runner: R,
}

impl BtrfsSnapshot<SystemRunner> {
    /// Construct a provider that runs real system commands
    #[must_use]
    pub const fn new() -> Self {
        Self {
            runner: SystemRunner,
        }
    }
}

impl<R: CommandRunner> BtrfsSnapshot<R> {
    /// Construct a provider backed by a custom command runner, for tests
    pub const fn with_runner(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: CommandRunner> SnapshotProvider for BtrfsSnapshot<R> {
    fn snapshot(&self, tag: &str) -> Result<Option<SnapshotId>, GelError> {
        // no snapper means no snapshots available; caller falls back to Noop
        if !self.runner.is_available("snapper") {
            return Ok(None);
        }
        let args = [
            "create",
            "--type",
            "pre",
            "--print-number",
            "--description",
            tag,
        ];
        let output = self.runner.run("snapper", &args)?;
        if !output.success {
            // stderr is kept for server-side logs only, never surfaced to users
            return Err(GelError::Backend(format!(
                "snapshot creation failed: {}",
                output.stderr.trim()
            )));
        }
        let number = output.stdout.trim();
        if number.is_empty() {
            return Err(GelError::Backend(
                "snapshot creation returned no snapshot number".to_owned(),
            ));
        }
        Ok(Some(SnapshotId(number.to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;
    use crate::sys::{CommandOutput, CommandRunner};

    /// Ordered log of `(program, args)` pairs a mock runner was asked to run
    type CallLog = Rc<RefCell<Vec<(String, Vec<String>)>>>;

    /// A recording [`CommandRunner`] that never executes anything
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

    #[test]
    fn returns_none_without_snapper() {
        let runner = MockRunner::new(true, "", "", &[]);
        let provider = BtrfsSnapshot::with_runner(runner.clone());

        let result = provider.snapshot("tx-1").expect("snapshot");

        assert_eq!(result, None);
        // nothing executed when snapper is absent
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn builds_snapper_argv_and_parses_number() {
        let runner = MockRunner::new(true, "42\n", "", &["snapper"]);
        let provider = BtrfsSnapshot::with_runner(runner.clone());

        let result = provider.snapshot("tx-1").expect("snapshot");

        assert_eq!(result, Some(SnapshotId("42".to_owned())));
        assert_eq!(
            runner.calls(),
            vec![(
                "snapper".to_owned(),
                vec![
                    "create".to_owned(),
                    "--type".to_owned(),
                    "pre".to_owned(),
                    "--print-number".to_owned(),
                    "--description".to_owned(),
                    "tx-1".to_owned(),
                ],
            )]
        );
    }

    #[test]
    fn empty_number_is_error() {
        let runner = MockRunner::new(true, "\n", "", &["snapper"]);
        let provider = BtrfsSnapshot::with_runner(runner);

        let result = provider.snapshot("tx-1");

        assert!(matches!(result, Err(GelError::Backend(_))));
    }

    #[test]
    fn nonzero_exit_is_error() {
        let runner = MockRunner::new(false, "", "not a btrfs subvolume", &["snapper"]);
        let provider = BtrfsSnapshot::with_runner(runner);

        let result = provider.snapshot("tx-1");

        assert!(matches!(result, Err(GelError::Backend(_))));
    }
}
