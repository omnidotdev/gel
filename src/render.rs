//! Human-readable rendering of a plan for the system-touching commands

use gel_core::plan::Plan;

/// Print a plan as a `+N to install, -N to remove` summary plus package lists
///
/// When the plan carries managed file writes, they are summarized as
/// `~N files to write` and each target path is listed. A plan produced by
/// [`Plan::compute`] alone carries no file writes (those require reading current
/// content); callers that want files surfaced populate `file_writes` first via
/// [`gel_core::plan::plan_files`].
pub fn print_plan(plan: &Plan) {
    let install = plan.native_install.len() + plan.foreign_install.len();
    let remove = plan.native_remove.len() + plan.foreign_remove.len();
    let files = plan.file_writes.len();
    println!("+{install} to install, -{remove} to remove, ~{files} files to write");
    for pkg in &plan.native_install {
        println!("  + {pkg} (native)");
    }
    for pkg in &plan.foreign_install {
        println!("  + {pkg} (foreign)");
    }
    for pkg in &plan.native_remove {
        println!("  - {pkg} (native)");
    }
    for pkg in &plan.foreign_remove {
        println!("  - {pkg} (foreign)");
    }
    for file in &plan.file_writes {
        println!("  ~ {} (file)", file.path);
    }
}
