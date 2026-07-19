//! Human-readable rendering of a plan for the system-touching commands

use gel_core::plan::Plan;

/// Print a plan as a `+N to install, -N to remove` summary plus package lists
pub fn print_plan(plan: &Plan) {
    let install = plan.native_install.len() + plan.foreign_install.len();
    let remove = plan.native_remove.len() + plan.foreign_remove.len();
    println!("+{install} to install, -{remove} to remove");
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
}
