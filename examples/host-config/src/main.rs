//! Example user-authored gel host configuration
//!
//! A gel config is an ordinary Rust program: it builds a [`System`] and prints
//! the resulting desired state as JSON to stdout. `gel eval` runs this binary,
//! captures that JSON, and writes it to a desired-state artifact. Edit the
//! package lists below to describe the machine you want.

use gel_core::config::System;

fn main() {
    let system = System::new()
        // native (official-repo) packages, managed with pacman
        .native(["git", "ripgrep", "fd", "bat"])
        // foreign (AUR) packages, managed with an AUR helper
        .foreign(["paru"])
        // a managed file: gel writes this content verbatim on apply and restores
        // the prior content on rollback. The path here is deliberately a harmless
        // demo file; point it at a real dotfile to manage one for real
        .file(
            "/tmp/gel-demo.conf",
            "# managed by gel; edit examples/host-config to change\ngreeting = hello\n",
        );

    // print the desired state as JSON on stdout for `gel eval` to capture
    let json = serde_json::to_string(&system.build()).expect("serialize desired state");
    print!("{json}");
}
