//! Behavioral CLI tests that are safe to run on any host
//!
//! These drive the built `gel` binary but never touch system package state.
//! `gel eval` is pure (it runs cargo and writes a file), so exercising its error
//! paths here is safe. The full `gel eval examples/host-config` round-trip is
//! exercised manually (it compiles a separate crate); see README.md.

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
}
