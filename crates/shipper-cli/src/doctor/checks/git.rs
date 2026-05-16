//! Git working-tree context probe.

use shipper_core::plan;

use crate::doctor::findings::{Finding, FindingLevel};

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace) -> Vec<Finding> {
    let mut findings = Vec::new();

    match shipper_core::git::collect_git_context_at(&ws.workspace_root) {
        Some(git) => {
            let dirty = git.dirty.unwrap_or(false);
            println!("git_commit: {}", git.commit.unwrap_or_else(|| "-".into()));
            println!("git_branch: {}", git.branch.unwrap_or_else(|| "-".into()));
            println!("git_dirty: {}", dirty);
            if dirty {
                findings.push(Finding {
                    id: "git-working-tree-dirty",
                    severity: FindingLevel::Blocked,
                    status: FindingLevel::Blocked,
                    title: "git working tree is dirty",
                    why_it_matters:
                        "release evidence must describe the exact source tree being planned, proven, published, and resumed",
                    evidence: "git_dirty: true".to_string(),
                    try_next: vec![
                        "commit, stash, or revert unrelated changes before release",
                        "rerun `shipper doctor` and `shipper preflight`",
                        "use `--allow-dirty` only for intentional local rehearsal",
                    ],
                    docs: Some("docs/failure-modes.md"),
                });
            }
        }
        None => println!("git_context: not a git repository"),
    }

    findings
}
