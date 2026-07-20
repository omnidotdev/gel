//! Human-readable rendering of a plan for the system-touching commands

use gel_core::plan::Plan;

/// Print a plan as a `+N to install, -N to remove` summary plus package lists
///
/// When the plan carries managed file writes, they are summarized as
/// `~N files to write` and each target path is listed. Service intent is
/// summarized as `+N to enable, -N to disable` on a second line with each unit
/// listed, and setting changes as `~N settings to change` on a third line with
/// each key/value listed. A plan produced by [`Plan::compute`] alone carries no
/// file writes, service actions, or setting changes (those require reading
/// current state); callers that want them surfaced populate `file_writes` via
/// [`gel_core::plan::plan_files`], `service_enable`/`service_disable` via
/// [`gel_core::plan::plan_services`], and `setting_changes` via
/// [`gel_core::plan::plan_settings`] first.
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

    let enable = plan.service_enable.len();
    let disable = plan.service_disable.len();
    println!("+{enable} to enable, -{disable} to disable");
    for unit in &plan.service_enable {
        println!("  + {unit} (service)");
    }
    for unit in &plan.service_disable {
        println!("  - {unit} (service)");
    }

    let settings = plan.setting_changes.len();
    println!("~{settings} settings to change");
    for (key, value) in &plan.setting_changes {
        println!("  ~ {key:?} = {value} (setting)");
    }
}
