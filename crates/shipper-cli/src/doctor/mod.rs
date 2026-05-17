//! `shipper doctor` — environment diagnostics.
//!
//! The orchestrator [`run`] prints a header, runs each check in
//! [`checks`] in turn, and renders the aggregated [`findings`] at the
//! end. Each check is responsible for one subsystem (auth, state dir,
//! tools, connectivity, git, encryption) so that adding a new diagnostic
//! is an additive change rather than an edit of a long function.

use anyhow::{Context, Result};
use serde::Serialize;

use shipper_core::engine::Reporter;
use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

mod checks;
mod findings;
mod redaction;

#[cfg(test)]
pub(crate) use checks::tools::print_cmd_version;
pub(crate) use redaction::redact_diagnostic_value;

#[derive(Debug, Serialize)]
pub(crate) struct DoctorOutput {
    schema_version: &'static str,
    reports: Vec<DoctorReport>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DoctorReport {
    workspace_root: String,
    registry: DoctorRegistryReport,
    auth: checks::auth::AuthCheck,
    state_dir: checks::state_dir::StateDirCheck,
    tools: Vec<checks::tools::ToolCheck>,
    connectivity: checks::connectivity::ConnectivityCheck,
    git: checks::git::GitCheck,
    encryption: checks::encryption::EncryptionCheck,
    findings: Vec<findings::Finding>,
}

#[derive(Debug, Serialize)]
struct DoctorRegistryReport {
    name: String,
    api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_base: Option<String>,
}

pub(crate) fn collect_report(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<DoctorReport> {
    let auth = checks::auth::inspect(ws)?;
    let state_dir = checks::state_dir::inspect(ws, opts);
    let tools = checks::tools::inspect();
    let connectivity = checks::connectivity::inspect(ws)?;
    let git = checks::git::inspect(ws);
    let encryption = checks::encryption::inspect(opts);

    let mut findings = Vec::new();
    findings.extend(auth.findings.clone());
    findings.extend(state_dir.findings.clone());
    findings.extend(connectivity.findings.clone());
    findings.extend(git.findings.clone());
    findings.extend(encryption.findings.clone());

    Ok(DoctorReport {
        workspace_root: ws.workspace_root.display().to_string(),
        registry: DoctorRegistryReport {
            name: ws.plan.registry.name.clone(),
            api_base: redact_diagnostic_value(&ws.plan.registry.api_base),
            index_base: ws
                .plan
                .registry
                .index_base
                .as_deref()
                .map(redact_diagnostic_value),
        },
        auth,
        state_dir,
        tools,
        connectivity,
        git,
        encryption,
        findings,
    })
}

pub(crate) fn print_json(reports: Vec<DoctorReport>) -> Result<()> {
    let output = DoctorOutput {
        schema_version: "shipper.doctor.v1",
        reports,
    };
    let json = serde_json::to_string_pretty(&output).context("serialize doctor report")?;
    println!("{json}");
    Ok(())
}

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
        ws.plan.registry.name,
        redact_diagnostic_value(&ws.plan.registry.api_base)
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
