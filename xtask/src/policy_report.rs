//! `cargo xtask policy-report`
//!
//! Runs all advisory checks (package-surface, file-policy, generated,
//! executable, dependency-surface, workflow, process, network, doc-contracts,
//! no-panic), reads each one's
//! `target/policy/*-report.json` artifact, and writes a unified
//! `target/policy/policy-report.{md,json}`.
//!
//! Always advisory. Promotion to blocking lands in PRs 10/11/12 by
//! changing how CI invokes the individual sub-checks, not by adding modes
//! here.
//!
//! See `docs/policy/NON_RUST_ROLLOUT.md`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::{check_file_policy, checks, doc_contracts, no_panic, package_surface, workflow_checks};

const OUTPUT_DIR_REL: &str = "target/policy";

#[derive(Debug, Serialize)]
struct UnifiedReport {
    tool: &'static str,
    generated_at: String,
    areas: Vec<AreaSummary>,
    headline: Vec<HeadlineRow>,
}

#[derive(Debug, Serialize)]
struct AreaSummary {
    label: &'static str,
    artifact: String,
    summary: Value,
}

#[derive(Debug, Serialize)]
struct HeadlineRow {
    area: &'static str,
    metric: &'static str,
    value: u64,
}

pub fn policy_report() -> Result<()> {
    let workspace_root = workspace_root()?;

    // Step 1: run every advisory check. Each writes its own
    // `target/policy/*-report.json`. Advisory mode never bails, so we
    // don't have to worry about short-circuiting.
    println!("running all advisory checks for unified report…");
    package_surface::package_surface()?;
    check_file_policy::check(check_file_policy::Mode::Advisory)?;
    checks::check_generated(checks::Mode::Advisory)?;
    checks::check_executable_files(checks::Mode::Advisory)?;
    checks::check_dependency_surfaces(checks::Mode::Advisory)?;
    workflow_checks::check_workflow_surfaces(workflow_checks::Mode::Advisory)?;
    workflow_checks::check_process_policy(workflow_checks::Mode::Advisory)?;
    workflow_checks::check_network_policy(workflow_checks::Mode::Advisory)?;
    doc_contracts::check(doc_contracts::Mode::Advisory)?;
    no_panic::check(no_panic::Mode::Advisory)?;

    // Step 2: read each sub-report and lift its `summary` block.
    let registrations: &[(&'static str, &str)] = &[
        ("Package surface", "package-surface-report"),
        ("Non-Rust file policy", "file-policy-report"),
        ("Generated files", "generated-policy-report"),
        ("Executable files", "executable-policy-report"),
        ("Dependency surfaces", "dependency-surface-policy-report"),
        ("Workflow surfaces", "workflow-policy-report"),
        ("Process policy", "process-policy-report"),
        ("Network policy", "network-policy-report"),
        ("Doc contracts", "doc-contracts-report"),
        ("No-panic baseline", "no-panic-report"),
    ];

    let mut areas: Vec<AreaSummary> = Vec::with_capacity(registrations.len());
    let mut headline: Vec<HeadlineRow> = Vec::new();
    for (label, basename) in registrations {
        let path = workspace_root
            .join(OUTPUT_DIR_REL)
            .join(format!("{basename}.json"));
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let value: Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        let summary = value.get("summary").cloned().unwrap_or(Value::Null);

        if let Some(row) = headline_for(label, &summary) {
            headline.push(row);
        }

        areas.push(AreaSummary {
            label,
            artifact: format!("{basename}.json"),
            summary,
        });
    }

    // Step 3: write the unified artifacts.
    let unified = UnifiedReport {
        tool: "cargo xtask policy-report",
        generated_at: today_iso(),
        areas,
        headline,
    };

    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(&unified).context("serializing unified report")?;
    fs::write(out_dir.join("policy-report.json"), json).context("writing policy-report.json")?;
    fs::write(out_dir.join("policy-report.md"), render_md(&unified))
        .context("writing policy-report.md")?;

    println!(
        "wrote unified policy-report ({} areas, {} headline rows)",
        unified.areas.len(),
        unified.headline.len(),
    );
    Ok(())
}

/// Lift a single "what's the most useful first number to surface" from
/// each area's summary block. Returns `None` when the summary doesn't
/// carry a meaningful headline (e.g., a check whose universe is empty by
/// design).
fn headline_for(area: &'static str, summary: &Value) -> Option<HeadlineRow> {
    let s = summary.as_object()?;
    let metric_priority = [
        "unreceipted",
        "invalid_policy_refs",
        "unknown_total",
        "blocking_findings",
        "findings",
        "violations",
        "missing_fields",
        "expired",
        "stale",
        "unused",
    ];
    for metric in metric_priority {
        if let Some(v) = s.get(metric).and_then(|v| v.as_u64())
            && v > 0
        {
            return Some(HeadlineRow {
                area,
                metric: static_metric(metric),
                value: v,
            });
        }
    }
    // Everything is zero. Surface a "clean" row keyed on the universe metric.
    let universe = [
        "tracked_non_rust",
        "universe_size",
        "tracked_workflow_files",
        "workflows",
        "documents_checked",
        "baseline_entries",
        "workspace_packages",
    ]
    .into_iter()
    .find_map(|k| s.get(k).and_then(|v| v.as_u64()))
    .unwrap_or(0);
    Some(HeadlineRow {
        area,
        metric: "clean",
        value: universe,
    })
}

fn static_metric(name: &str) -> &'static str {
    match name {
        "unreceipted" => "unreceipted",
        "invalid_policy_refs" => "invalid_policy_refs",
        "unknown_total" => "unknown_total",
        "violations" => "violations",
        "missing_fields" => "missing_fields",
        "expired" => "expired",
        "stale" => "stale",
        "unused" => "unused",
        _ => "other",
    }
}

fn render_md(r: &UnifiedReport) -> String {
    let mut out = String::new();
    out.push_str("# Unified Policy Report\n\n");
    out.push_str(&format!(
        "Generated by `{}` on {}.\n\n",
        r.tool, r.generated_at
    ));

    out.push_str("## Headline\n\n");
    out.push_str("| Area | Headline metric | Value |\n");
    out.push_str("|---|---|---:|\n");
    for row in &r.headline {
        let label = if row.metric == "clean" {
            "_clean_"
        } else {
            row.metric
        };
        let value = if row.metric == "clean" {
            format!("{} (universe)", row.value)
        } else {
            row.value.to_string()
        };
        out.push_str(&format!("| {} | {} | {} |\n", row.area, label, value));
    }
    out.push('\n');

    out.push_str("## Areas\n\n");
    for area in &r.areas {
        out.push_str(&format!("### {} (`{}`)\n\n", area.label, area.artifact));
        match &area.summary {
            Value::Object(map) => {
                for (k, v) in map {
                    out.push_str(&format!("- `{}`: {}\n", k, render_value(v)));
                }
                out.push('\n');
            }
            other => {
                out.push_str(&format!("```json\n{}\n```\n\n", render_value(other)));
            }
        }
    }

    out
}

fn render_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via `cargo xtask`")?;
    let xtask_dir = PathBuf::from(manifest_dir);
    let root = xtask_dir
        .parent()
        .with_context(|| format!("xtask manifest dir has no parent: {}", xtask_dir.display()))?
        .to_path_buf();
    let _: &Path = &root;
    Ok(root)
}

fn today_iso() -> String {
    chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}
