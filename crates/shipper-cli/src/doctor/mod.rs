//! `shipper doctor` — environment diagnostics.
//!
//! The orchestrator [`run`] prints a header, runs each check in
//! [`checks`] in turn, and renders the aggregated [`findings`] at the
//! end. Each check is responsible for one subsystem (auth, state dir,
//! tools, connectivity, git, encryption) so that adding a new diagnostic
//! is an additive change rather than an edit of a long function.

use anyhow::Result;

use shipper_core::engine::Reporter;
use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

mod checks;
mod findings;

#[cfg(test)]
pub(crate) use checks::tools::print_cmd_version;

pub(crate) fn run(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<()> {
    let mut all = Vec::new();

    println!("Shipper Doctor - Diagnostics Report");
    println!("----------------------------------");
    println!("workspace_root: {}", ws.workspace_root.display());
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );

    all.extend(checks::auth::check(ws)?);
    all.extend(checks::state_dir::check(ws, opts));

    println!();
    checks::tools::check(reporter);

    println!();
    all.extend(checks::connectivity::check(ws, reporter)?);

    println!();
    all.extend(checks::git::check(ws));

    all.extend(checks::encryption::check(opts));

    findings::print_findings(&all);

    println!();
    println!("Diagnostics complete.");

    Ok(())
}
