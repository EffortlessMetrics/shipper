//! Git working-tree context probe (informational; no findings emitted).

pub(in crate::doctor) fn check() {
    match shipper_core::git::collect_git_context() {
        Some(git) => {
            let dirty = git.dirty.unwrap_or(false);
            println!("git_commit: {}", git.commit.unwrap_or_else(|| "-".into()));
            println!("git_branch: {}", git.branch.unwrap_or_else(|| "-".into()));
            println!("git_dirty: {}", dirty);
        }
        None => println!("git_context: not a git repository"),
    }
}
