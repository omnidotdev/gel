//! Starter gel machine config
//!
//! This is an ordinary Rust program: it builds a [`System`] describing the
//! machine you want and prints it as JSON. `gel eval` runs it and captures the
//! JSON into a desired-state artifact, which `gel diff`/`gel apply` then use.
//!
//! Edit the declarations below, then:
//!   gel eval  . --out desired.json
//!   gel diff  --artifact desired.json
//!   gel apply --artifact desired.json

use gel_core::config::System;

fn main() {
    let system = System::new()
        // official-repo packages (managed with pacman)
        .native(["git", "ripgrep", "neovim"])
        // AUR packages (needs paru/yay present); uncomment to use:
        // .foreign(["paru"])
        // managed config files: gel writes the content on apply and restores the
        // prior content on rollback. Point these at real dotfiles you want managed
        .file(
            "/etc/gel-demo.conf",
            "# managed by gel\ngreeting = hello\n",
        )
        // services to ensure enabled (or .disable(...) to ensure disabled)
        .enable("systemd-timesyncd.service")
        // system settings (drop any you do not want gel to manage)
        .hostname("my-machine")
        .timezone("Etc/UTC");

    let json = serde_json::to_string(&system.build()).expect("serialize desired state");
    print!("{json}");
}
