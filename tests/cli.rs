//! Behavioral CLI tests that are safe to run on any host
//!
//! These drive the built `gel` binary but never touch system package state. In
//! the default (no `arch` feature) build, every system-touching subcommand must
//! fast-fail with a clear rebuild message before doing anything, so exercising
//! them here is safe. `env!("CARGO_BIN_EXE_gel")` resolves to the binary built
//! for the current feature set, which for a plain `cargo test` is the pure one.
//!
//! The full `gel eval examples/host-config` round-trip is exercised manually
//! (it compiles a separate crate); see README.md and crates/gel-core/TESTING.md.

use std::process::Command;

/// Run `gel` with the given args and return `(success, combined stderr+stdout)`
fn run_gel(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_gel"))
        .args(args)
        .output()
        .expect("run gel binary");
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

#[cfg(not(feature = "arch"))]
#[test]
fn system_commands_fast_fail_without_arch_feature() {
    // each system-touching subcommand must exit non-zero with the rebuild hint
    // and must not touch the host; a plain `cargo test` builds the pure binary.
    // diff and apply now also plan managed file writes, service enable/disable
    // actions, and system setting changes, but that work lives behind the arch
    // feature, so they must still fast-fail before any planning (no systemctl,
    // hostnamectl, timedatectl, or localectl is ever invoked here)
    for args in [
        vec!["diff"],
        vec!["apply"],
        vec!["apply", "--prune"],
        vec!["import"],
        vec!["rollback"],
    ] {
        let (success, out) = run_gel(&args);
        assert!(
            !success,
            "`gel {}` should fail without arch support",
            args[0]
        );
        assert!(
            out.contains("rebuild with --features arch"),
            "`gel {}` should mention the rebuild path, got: {out}",
            args[0]
        );
        // service planning lives behind the arch feature: the pure build must
        // fast-fail before printing any service summary, so the enable/disable
        // line must never appear
        assert!(
            !out.contains("to enable") && !out.contains("to disable"),
            "`gel {}` must not plan services in the pure build, got: {out}",
            args[0]
        );
        // settings planning also lives behind the arch feature: the pure build
        // must fast-fail before reading any current setting, so the settings
        // summary line must never appear either
        assert!(
            !out.contains("settings to change") && !out.contains("settings to restore"),
            "`gel {}` must not plan settings in the pure build, got: {out}",
            args[0]
        );
    }
}

#[test]
fn eval_requires_a_config_directory() {
    // eval is available in every build; a missing manifest is a clean error, not
    // a crash, and this never runs cargo against a real config
    let (success, out) = run_gel(&["eval", "/nonexistent/gel-config-dir"]);

    assert!(!success, "eval on a missing directory should fail");
    assert!(
        out.contains("no Cargo.toml found"),
        "expected a missing-manifest message, got: {out}"
    );
}

#[test]
fn help_documents_the_eval_apply_split() {
    let (success, out) = run_gel(&["--help"]);

    assert!(success, "--help should succeed");
    assert!(out.contains("Eval/apply split"));
    assert!(out.contains("--features arch"));
}
