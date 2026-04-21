//! Build-time metadata for `shipper --version`.
//!
//! Emits three `rustc-env` values consumed by `src/lib.rs`:
//!
//! - `SHIPPER_GIT_SHA`        — short git SHA (`git rev-parse --short HEAD`),
//!   or the literal string `unknown` if the workspace is not a git checkout
//!   or the `git` binary is missing.
//! - `SHIPPER_BUILD_PROFILE`  — cargo build profile (`debug`, `release`, etc.),
//!   sourced from the `PROFILE` env var cargo sets for build scripts.
//! - `SHIPPER_RUSTC_VERSION`  — the first line of `rustc --version`, e.g.
//!   `rustc 1.92.0 (abc1234 2026-01-01)`, or `unknown` on failure.
//!
//! Kept deliberately stdlib-only — no `vergen` — so the build cost is a few
//! milliseconds and operators auditing our supply chain have one fewer
//! transitive dependency to vet.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Resolve git internals through `git rev-parse --git-path` so this
    // stays correct in linked worktrees, where `.git` is a file pointing
    // at the real git dir rather than a directory of its own.
    println!("cargo:rerun-if-changed=build.rs");
    emit_git_rerun_hints();

    let git_sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo::rustc-env=SHIPPER_GIT_SHA={git_sha}");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo::rustc-env=SHIPPER_BUILD_PROFILE={profile}");

    let rustc_version =
        Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string()))
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
    println!("cargo::rustc-env=SHIPPER_RUSTC_VERSION={rustc_version}");
}

fn emit_git_rerun_hints() {
    if let Some(head_path) = git_path("HEAD") {
        println!("cargo:rerun-if-changed={}", head_path.display());
    }

    if let Some(current_ref) = git_stdout(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(ref_path) = git_path(&current_ref) {
            println!("cargo:rerun-if-changed={}", ref_path.display());
        }
    }

    if let Some(packed_refs) = git_path("packed-refs") {
        println!("cargo:rerun-if-changed={}", packed_refs.display());
    }
}

fn git_path(spec: &str) -> Option<PathBuf> {
    git_stdout(&["rev-parse", "--git-path", spec]).map(PathBuf::from)
}

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|stdout| !stdout.is_empty())
}
