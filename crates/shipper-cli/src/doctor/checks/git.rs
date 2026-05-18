//! Git working-tree context probe.

use shipper_core::plan;

use crate::doctor::findings::{Finding, FindingLevel};

#[derive(Debug, serde::Serialize)]
pub(in crate::doctor) struct GitCheck {
    pub is_repository: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace) -> Vec<Finding> {
    let check = inspect(ws);
    if check.is_repository {
        println!(
            "git_commit: {}",
            check.commit.clone().unwrap_or_else(|| "-".into())
        );
        println!(
            "git_branch: {}",
            check.branch.clone().unwrap_or_else(|| "-".into())
        );
        println!("git_dirty: {}", check.dirty.unwrap_or(false));
    } else {
        println!("git_context: not a git repository");
    }
    check.findings
}

pub(in crate::doctor) fn inspect(ws: &plan::PlannedWorkspace) -> GitCheck {
    let mut findings = Vec::new();

    match shipper_core::git::collect_git_context_at(&ws.workspace_root) {
        Some(git) => {
            let dirty = git.dirty.unwrap_or(false);
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
            GitCheck {
                is_repository: true,
                commit: git.commit,
                branch: git.branch,
                dirty: Some(dirty),
                findings,
            }
        }
        None => GitCheck {
            is_repository: false,
            commit: None,
            branch: None,
            dirty: None,
            findings,
        },
    }
}
