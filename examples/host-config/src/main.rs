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
        .foreign(["paru"]);

    // print the desired state as JSON on stdout for `gel eval` to capture
    let json = serde_json::to_string(&system.build()).expect("serialize desired state");
    print!("{json}");
}
