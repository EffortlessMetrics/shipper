//! Workspace- and package-scope dry-run verification for preflight.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cargo;
use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::runtime::policy::PolicyEffects;
use crate::types::{RuntimeOptions, VerifyMode};

/// Combined result of the workspace and per-package dry-run phases.
///
/// `workspace_passed` / `workspace_output` represent the workspace-level
/// dry-run when `VerifyMode::Workspace` is active (or a synthesized
/// "skipped" summary otherwise). `per_package` is populated only when
/// `VerifyMode::Package` is active.
pub(in crate::engine) struct DryRunOutcome {
    pub workspace_passed: bool,
    pub workspace_output: String,
    pub per_package: BTreeMap<String, (bool, Option<String>)>,
}

pub(in crate::engine) fn execute(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    effects: &PolicyEffects,
    state_dir: &Path,
    reporter: &mut dyn Reporter,
) -> DryRunOutcome {
    let (workspace_passed, workspace_output) =
        workspace_dry_run(ws, opts, effects, state_dir, reporter);

    let per_package = per_package_dry_run(ws, opts, effects, reporter);

    DryRunOutcome {
        workspace_passed,
        workspace_output,
        per_package,
    }
}

fn workspace_dry_run(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    effects: &PolicyEffects,
    state_dir: &Path,
    reporter: &mut dyn Reporter,
) -> (bool, String) {
    // Event-payload handling (#92): the raw dry-run stderr is cargo's
    // human-facing log with embedded ANSI escapes - historically ~2KB per
    // event and not useful in a structured log. We now:
    //   1. Strip ANSI from the full captured output,
    //   2. Write the full stripped output to a sidecar at
    //      <state_dir>/preflight_workspace_verify.txt,
    //   3. Put only a short summary (exit_code + last ~200 chars tail) into
    //      the event's `output` field, preserving the field shape for
    //      backward compatibility.
    let workspace_root = &ws.workspace_root;
    if effects.run_dry_run && opts.verify_mode == VerifyMode::Workspace {
        reporter.info("running workspace dry-run verification...");
        let dry_run_result = cargo::cargo_publish_dry_run_workspace(
            workspace_root,
            &ws.plan.registry.name,
            opts.allow_dirty,
            opts.output_lines,
        );
        match &dry_run_result {
            Ok(output) => {
                let passed = output.exit_code == 0;
                let full_stripped = format!(
                    "workspace dry-run: exit_code={}\n\n--- stdout ---\n{}\n\n--- stderr ---\n{}\n",
                    output.exit_code,
                    shipper_output_sanitizer::strip_ansi(&output.stdout_tail),
                    shipper_output_sanitizer::strip_ansi(&output.stderr_tail),
                );
                let sidecar_path = state_dir.join("preflight_workspace_verify.txt");
                if let Some(parent) = sidecar_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&sidecar_path, &full_stripped) {
                    reporter.warn(&format!(
                        "failed to write preflight workspace-verify sidecar at {}: {e}",
                        sidecar_path.display()
                    ));
                }
                // The sidecar path is deterministic (<state_dir>/preflight_
                // workspace_verify.txt); documented in the runbook. Keeping
                // the success path quiet avoids churn in operator output
                // snapshots and byte-count variability across platforms.
                // Slim summary for the event log: exit code + tail of
                // ANSI-stripped stderr (the interesting signal).
                let tail_summary = shipper_output_sanitizer::tail_lines(
                    &shipper_output_sanitizer::strip_ansi(&output.stderr_tail),
                    6,
                );
                let summary = format!(
                    "workspace dry-run: exit_code={}; sidecar={}; stderr_tail_summary={:?}",
                    output.exit_code,
                    sidecar_path.display(),
                    tail_summary
                );
                (passed, summary)
            }
            Err(err) => (false, format!("workspace dry-run failed: {err:#}")),
        }
    } else if !effects.run_dry_run || opts.verify_mode == VerifyMode::None {
        reporter.info("skipping dry-run (policy, --no-verify, or verify_mode=none)");
        (
            true,
            "workspace dry-run skipped (policy, --no-verify, or verify_mode=none)".to_string(),
        )
    } else {
        // Package mode - handled per-package below
        (
            true,
            "workspace dry-run skipped (verify_mode=package)".to_string(),
        )
    }
}

fn per_package_dry_run(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    effects: &PolicyEffects,
    reporter: &mut dyn Reporter,
) -> BTreeMap<String, (bool, Option<String>)> {
    if effects.run_dry_run && opts.verify_mode == VerifyMode::Package {
        reporter.info("running per-package dry-run verification...");
        let mut results = BTreeMap::new();
        for p in &ws.plan.packages {
            let result = cargo::cargo_publish_dry_run_package(
                &ws.workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.output_lines,
            );
            let (passed, output) = match &result {
                Ok(out) => (
                    out.exit_code == 0,
                    Some(format!(
                        "exit_code={}; stdout_tail={:?}; stderr_tail={:?}",
                        out.exit_code, out.stdout_tail, out.stderr_tail
                    )),
                ),
                Err(e) => (false, Some(format!("dry-run failed: {e:#}"))),
            };
            if !passed {
                reporter.warn(&format!("{}@{}: dry-run failed", p.name, p.version));
            }
            results.insert(p.name.clone(), (passed, output));
        }
        results
    } else {
        BTreeMap::new()
    }
}
