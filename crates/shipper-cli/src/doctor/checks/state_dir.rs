//! State-directory existence and writability check.

use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace, opts: &RuntimeOptions) -> Vec<Finding> {
    let abs_state = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };
    println!("state_dir: {}", abs_state.display());

    let mut findings = Vec::new();
    if abs_state.exists() {
        if let Ok(meta) = std::fs::metadata(&abs_state) {
            let writable = !meta.permissions().readonly();
            println!("state_dir_writable: {}", writable);
            if !writable {
                findings.push(Finding {
                    id: "state-dir-readonly",
                    severity: FindingLevel::Blocked,
                    status: FindingLevel::Blocked,
                    title: "state directory is read-only",
                    why_it_matters:
                        "publish and resume need to write state, events, receipts, and lock files continuously",
                    evidence: format!("state_dir: {}", abs_state.display()),
                    try_next: vec![
                        "fix filesystem permissions for the state directory",
                        "choose a writable directory with `--state-dir <path>`",
                        "rerun `shipper doctor`",
                    ],
                    docs: Some("docs/reference/state-files.md"),
                });
            }
        }
    } else {
        println!("state_dir_exists: false (will be created)");
    }
    findings
}
