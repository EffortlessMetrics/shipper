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
    /// Reason no publish plan could be built, when that is the case.
    ///
    /// Envelope-level, not per-report: "this directory has no workspace"
    /// is one condition about the run, and `--registries` fans the
    /// per-registry reports out below. Repeating it inside each report
    /// would make a consumer counting blockers over-count by the number
    /// of registries.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_unavailable: Option<String>,
    /// Run-scoped findings — those that hold regardless of registry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    workspace_findings: Vec<findings::Finding>,
    reports: Vec<DoctorReport>,
}

/// Whether `shipper doctor` is reporting on a real workspace plan.
///
/// `doctor` is the "why is my environment broken" command, so it must
/// still run when `cargo metadata` cannot produce a plan — wrong
/// directory, missing manifest, unparseable `Cargo.toml`. In that case
/// the caller passes [`WorkspaceStatus::Unavailable`] with the reason,
/// and every environment-level check (auth, tools, connectivity, git,
/// encryption) still runs against the fallback root.
#[derive(Debug, Clone)]
pub(crate) enum WorkspaceStatus {
    /// `cargo metadata` produced a plan; `workspace_root` is authoritative.
    Planned,
    /// No plan could be built. Carries the short reason for the report.
    Unavailable(String),
}

impl WorkspaceStatus {
    fn reason(&self) -> Option<&str> {
        match self {
            WorkspaceStatus::Planned => None,
            WorkspaceStatus::Unavailable(reason) => Some(reason.as_str()),
        }
    }

    fn finding(&self) -> Option<findings::Finding> {
        let reason = self.reason()?;
        Some(findings::Finding {
            id: "workspace-plan-unavailable",
            severity: findings::FindingLevel::Blocked,
            status: findings::FindingLevel::Blocked,
            title: "no publish plan could be built for this directory",
            why_it_matters: "plan, preflight, publish, and resume all need a workspace plan; \
                 the environment checks below still ran, but no release command will work here",
            evidence: reason.to_string(),
            try_next: vec![
                "cd to the workspace root that owns the crates you want to publish",
                "or pass `--manifest-path <workspace>/Cargo.toml`",
                "then rerun `shipper doctor` followed by `shipper plan`",
            ],
            docs: Some("docs/tutorials/getting-started-5-minutes.md"),
        })
    }
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

pub(crate) fn print_json(reports: Vec<DoctorReport>, workspace: &WorkspaceStatus) -> Result<()> {
    let output = DoctorOutput {
        schema_version: "shipper.doctor.v1",
        workspace_unavailable: workspace.reason().map(str::to_string),
        workspace_findings: workspace.finding().into_iter().collect(),
        reports,
    };
    let json = serde_json::to_string_pretty(&output).context("serialize doctor report")?;
    println!("{json}");
    Ok(())
}

/// Render one registry's diagnostics as text.
///
/// `emit_workspace_finding` exists because `--registries` calls this
/// once per registry, while a missing workspace plan is a property of
/// the run, not of any registry. The header line repeats it as context
/// for whichever block you are reading; the finding itself is listed
/// once, by the first block, so the findings list stays a list of
/// distinct problems.
pub(crate) fn run(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    workspace: &WorkspaceStatus,
    emit_workspace_finding: bool,
) -> Result<()> {
    let mut all = Vec::new();

    println!("Shipper Doctor - Diagnostics Report");
    println!("----------------------------------");
    match workspace.reason() {
        None => println!("workspace_root: {}", ws.workspace_root.display()),
        Some(reason) => {
            println!(
                "workspace_root: {} (no publish plan — {reason})",
                ws.workspace_root.display()
            );
        }
    }
    println!(
        "registry: {} ({})",
        ws.plan.registry.name,
        redact_diagnostic_value(&ws.plan.registry.api_base)
    );

    if emit_workspace_finding {
        all.extend(workspace.finding());
    }
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
