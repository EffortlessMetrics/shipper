//! State-directory existence and writability check.

use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};

#[derive(Debug, serde::Serialize)]
pub(in crate::doctor) struct StateDirCheck {
    pub path: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace, opts: &RuntimeOptions) -> Vec<Finding> {
    let check = inspect(ws, opts);
    println!("state_dir: {}", check.path);
    if check.exists {
        if let Some(writable) = check.writable {
            println!("state_dir_writable: {}", writable);
        }
    } else {
        println!("state_dir_exists: false (will be created)");
    }
    check.findings
}

pub(in crate::doctor) fn inspect(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> StateDirCheck {
    let abs_state = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };

    let mut findings = Vec::new();
    let exists = abs_state.exists();
    let mut writable = None;
    if exists && let Ok(meta) = std::fs::metadata(&abs_state) {
        let is_writable = !meta.permissions().readonly();
        writable = Some(is_writable);
        if !is_writable {
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
    StateDirCheck {
        path: abs_state.display().to_string(),
        exists,
        writable,
        findings,
    }
}
