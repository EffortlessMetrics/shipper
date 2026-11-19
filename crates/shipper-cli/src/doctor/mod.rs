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
mod summary;

#[cfg(test)]
#[path = "tests/state_dir.rs"]
mod state_dir_tests;

#[cfg(test)]
pub(crate) use checks::tools::print_cmd_version;
pub(crate) use redaction::redact_diagnostic_value;

#[derive(Debug, Serialize)]
pub(crate) struct DoctorOutput {
    schema_version: &'static str,
    summary: summary::DoctorSummary,
    /// Run-scoped workspace failure, including the legacy single-target
    /// planning failure. Mixed-target graph failures live in their reports.
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

/// Whether workspace discovery or a particular target's planning succeeded.
///
/// `doctor` is the "why is my environment broken" command, so it must
/// still run when `cargo metadata` cannot produce a plan — wrong
/// directory, missing manifest, unparseable `Cargo.toml`. In that case
/// the caller passes [`WorkspaceStatus::Unavailable`] with the reason,
/// and every environment-level check (auth, tools, connectivity, git,
/// encryption) still runs against the fallback root.
/// With multiple registries and successful metadata discovery, target graph
/// failures are carried separately from this run-scoped workspace condition.
#[derive(Debug, Clone)]
pub(crate) enum WorkspaceStatus {
    /// Workspace discovery or target planning succeeded.
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

    fn check_status(&self) -> summary::DoctorCheckStatus {
        match self {
            WorkspaceStatus::Planned => summary::DoctorCheckStatus::Passed,
            WorkspaceStatus::Unavailable(_) => summary::DoctorCheckStatus::Blocked,
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

    fn registry_finding(&self, registry: &str) -> Option<findings::Finding> {
        let reason = self.reason()?;
        Some(findings::Finding {
            id: "registry-plan-unavailable",
            severity: findings::FindingLevel::Blocked,
            status: findings::FindingLevel::Blocked,
            title: "no publish plan could be built for this registry",
            why_it_matters: "release commands for this registry need a valid target plan; other registry targets are evaluated independently",
            evidence: redact_diagnostic_value(&format!("registry '{registry}': {reason}")),
            try_next: vec![
                "check this registry's package publish restrictions and workspace dependencies",
                "run `shipper plan --registries <name>` for the registry named in this report",
                "resolve the reported planning error, then rerun `shipper doctor`",
            ],
            docs: Some("docs/configuration.md"),
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct DoctorReport {
    workspace_root: String,
    registry: DoctorRegistryReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_unavailable: Option<String>,
    auth: checks::auth::AuthCheck,
    state_dir: checks::state_dir::StateDirCheck,
    tools: Vec<checks::tools::ToolCheck>,
    connectivity: checks::connectivity::ConnectivityCheck,
    git: checks::git::GitCheck,
    encryption: checks::encryption::EncryptionCheck,
    summary: summary::DoctorSummary,
    findings: Vec<findings::Finding>,
}

#[derive(Debug, Serialize)]
struct DoctorRegistryReport {
    name: String,
    api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_base: Option<String>,
}

fn summarize_report(
    auth: &checks::auth::AuthCheck,
    state_dir: &checks::state_dir::StateDirCheck,
    tools: &[checks::tools::ToolCheck],
    connectivity: &checks::connectivity::ConnectivityCheck,
    git: &checks::git::GitCheck,
    encryption: &checks::encryption::EncryptionCheck,
    target_plan: Option<&WorkspaceStatus>,
) -> summary::DoctorSummary {
    let mut statuses = vec![(
        "registry auth",
        summary::status_from_findings(&auth.findings),
    )];

    statuses.push((
        "state directory",
        summary::status_from_findings(&state_dir.findings),
    ));

    statuses.extend(tools.iter().map(|tool| {
        (
            tool.command,
            if tool.version.is_some() {
                summary::DoctorCheckStatus::Passed
            } else {
                summary::DoctorCheckStatus::Unknown
            },
        )
    }));

    statuses.push((
        "registry connectivity",
        if connectivity.registry_reachable {
            summary::status_from_findings(&connectivity.findings)
        } else {
            summary::DoctorCheckStatus::Blocked
        },
    ));

    statuses.push((
        "git context",
        if git.is_repository {
            summary::status_from_findings(&git.findings)
        } else {
            summary::DoctorCheckStatus::Unknown
        },
    ));

    statuses.push((
        "encryption",
        if encryption.enabled && encryption.key_source.is_none() {
            summary::DoctorCheckStatus::Unknown
        } else {
            summary::status_from_findings(&encryption.findings)
        },
    ));

    if let Some(target_plan) = target_plan {
        statuses.push(("registry plan", target_plan.check_status()));
    }

    summary::DoctorSummary::from_checks(statuses)
}

pub(crate) fn collect_report(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    target_plan: Option<&WorkspaceStatus>,
) -> Result<DoctorReport> {
    let auth = checks::auth::inspect(ws, opts)?;
    let state_dir = checks::state_dir::inspect(ws, opts);
    let tools = checks::tools::inspect();
    let connectivity = checks::connectivity::inspect(ws, opts)?;
    let git = checks::git::inspect(ws);
    let encryption = checks::encryption::inspect(opts);

    let mut findings = Vec::new();
    findings.extend(auth.findings.clone());
    findings.extend(state_dir.findings.clone());
    findings.extend(connectivity.findings.clone());
    findings.extend(git.findings.clone());
    findings.extend(encryption.findings.clone());
    if let Some(target_plan) = target_plan {
        findings.extend(target_plan.registry_finding(&ws.plan.registry.name));
    }

    let summary = summarize_report(
        &auth,
        &state_dir,
        &tools,
        &connectivity,
        &git,
        &encryption,
        target_plan,
    );

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
        plan_unavailable: target_plan
            .and_then(WorkspaceStatus::reason)
            .map(redact_diagnostic_value),
        auth,
        state_dir,
        tools,
        connectivity,
        git,
        encryption,
        summary,
        findings,
    })
}

pub(crate) fn print_json(reports: Vec<DoctorReport>, workspace: &WorkspaceStatus) -> Result<()> {
    let summary = summary::DoctorSummary::combine(
        workspace.check_status(),
        reports.iter().map(|report| report.summary.clone()),
    );
    let output = DoctorOutput {
        schema_version: "shipper.doctor.v1",
        summary,
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
/// once per registry, while a metadata discovery failure is a property of
/// the run. A target graph failure is separate. The header repeats a global
/// failure as context for whichever block you are reading; the finding itself is listed
/// once, by the first block, so the findings list stays a list of
/// distinct problems.
pub(crate) fn run(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    workspace: &WorkspaceStatus,
    target_plan: Option<&WorkspaceStatus>,
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
    if let Some(target_plan) = target_plan {
        match target_plan.reason() {
            None => println!("registry_plan: available"),
            Some(reason) => println!(
                "registry_plan: unavailable ({})",
                redact_diagnostic_value(reason)
            ),
        }
        all.extend(target_plan.registry_finding(&ws.plan.registry.name));
    }

    if emit_workspace_finding {
        all.extend(workspace.finding());
    }

    let auth = checks::auth::check(ws, opts)?;
    all.extend(auth.findings.clone());

    let state_dir = checks::state_dir::check(ws, opts);
    all.extend(state_dir.findings.clone());

    println!();
    let tools = checks::tools::check(reporter);

    println!();
    let connectivity = checks::connectivity::check(ws, opts, reporter)?;
    all.extend(connectivity.findings.clone());

    println!();
    let git = checks::git::check(ws);
    all.extend(git.findings.clone());

    let encryption = checks::encryption::check(opts);
    all.extend(encryption.findings.clone());

    findings::print_findings(&all);

    let report_summary = summarize_report(
        &auth,
        &state_dir,
        &tools,
        &connectivity,
        &git,
        &encryption,
        target_plan,
    );
    if opts.registries.len() > 1 {
        // Multi-registry text output is one block per registry. Keep these
        // totals registry-scoped and do not count the run-scoped workspace
        // condition once per registry. The workspace finding remains visible
        // exactly once in the first block; JSON retains one true run total.
        report_summary.print_registry_human();
    } else {
        let run_summary =
            summary::DoctorSummary::combine(workspace.check_status(), [report_summary]);
        run_summary.print_human();
    }

    println!();
    println!("Diagnostics complete.");

    Ok(())
}
