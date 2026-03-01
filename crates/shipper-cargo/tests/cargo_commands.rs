//! Integration tests for cargo command execution and output handling.
//!
//! These tests exercise `cargo_publish`, `cargo_publish_dry_run_package`, and
//! `cargo_publish_dry_run_workspace` against real (but minimal) temporary Cargo
//! projects.  No env-var mutation is needed because we test against the real
//! `cargo` binary.

use std::fs;
use std::path::Path;
use std::time::Duration;

use shipper_cargo::{
    WorkspaceMetadata, cargo_publish, cargo_publish_dry_run_package,
    cargo_publish_dry_run_workspace, load_metadata,
};

// ── Helpers ────────────────────────────────────────────────────────────

/// Create a minimal, valid Cargo library crate in `dir`.
fn create_minimal_crate(dir: &Path) {
    fs::create_dir_all(dir.join("src")).expect("create src/");
    fs::write(
        dir.join("Cargo.toml"),
        r#"[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write Cargo.toml");
    fs::write(dir.join("src/lib.rs"), "").expect("write src/lib.rs");
}

/// Create a minimal Cargo workspace with two member crates.
fn create_minimal_workspace(dir: &Path) {
    fs::write(
        dir.join("Cargo.toml"),
        r#"[workspace]
members = ["crate-a", "crate-b"]
resolver = "2"
"#,
    )
    .expect("write workspace Cargo.toml");

    for name in &["crate-a", "crate-b"] {
        let crate_dir = dir.join(name);
        fs::create_dir_all(crate_dir.join("src")).expect("create member src/");
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
"#
            ),
        )
        .expect("write member Cargo.toml");
        fs::write(crate_dir.join("src/lib.rs"), "").expect("write member src/lib.rs");
    }
}

// ── cargo_publish_dry_run_package ──────────────────────────────────────

#[test]
fn dry_run_package_succeeds_for_valid_crate() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result =
        cargo_publish_dry_run_package(tmp.path(), "test-crate", "crates-io", true, 50).unwrap();

    assert!(!result.timed_out, "dry run should not time out");
    assert!(result.duration > Duration::ZERO);
    // Exit code 0 means packaging succeeded (dry-run only)
    assert_eq!(
        result.exit_code, 0,
        "dry run failed: stderr={}",
        result.stderr_tail
    );
}

#[test]
fn dry_run_package_captures_stderr_on_missing_package() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    // Ask for a package name that doesn't exist in the manifest
    let result =
        cargo_publish_dry_run_package(tmp.path(), "nonexistent-pkg", "crates-io", true, 50)
            .unwrap();

    assert_ne!(result.exit_code, 0, "should fail for nonexistent package");
    assert!(
        !result.stderr_tail.is_empty(),
        "stderr should have error output"
    );
}

#[test]
fn dry_run_package_returns_positive_duration() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result =
        cargo_publish_dry_run_package(tmp.path(), "test-crate", "crates-io", true, 50).unwrap();

    assert!(
        result.duration > Duration::ZERO,
        "duration must be positive"
    );
}

#[test]
fn dry_run_package_timed_out_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result =
        cargo_publish_dry_run_package(tmp.path(), "test-crate", "crates-io", true, 50).unwrap();

    assert!(!result.timed_out);
}

#[test]
fn dry_run_package_output_lines_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result =
        cargo_publish_dry_run_package(tmp.path(), "test-crate", "crates-io", true, 2).unwrap();

    let stdout_lines = result.stdout_tail.lines().count();
    let stderr_lines = result.stderr_tail.lines().count();
    assert!(
        stdout_lines <= 2,
        "stdout should have at most 2 lines, got {stdout_lines}"
    );
    assert!(
        stderr_lines <= 2,
        "stderr should have at most 2 lines, got {stderr_lines}"
    );
}

// ── cargo_publish_dry_run_workspace ────────────────────────────────────

#[test]
fn dry_run_workspace_succeeds_for_valid_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let result = cargo_publish_dry_run_workspace(tmp.path(), "crates-io", true, 50).unwrap();

    assert!(!result.timed_out);
    assert!(result.duration > Duration::ZERO);
}

#[test]
fn dry_run_workspace_timed_out_always_false() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let result = cargo_publish_dry_run_workspace(tmp.path(), "crates-io", true, 50).unwrap();

    assert!(
        !result.timed_out,
        "dry run workspace should never set timed_out"
    );
}

// ── cargo_publish: timeout handling ────────────────────────────────────

#[test]
fn publish_with_very_short_timeout_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    // 1ms is far too short for cargo to do anything → guaranteed timeout
    let result = cargo_publish(
        tmp.path(),
        "test-crate",
        "crates-io",
        true,
        true,
        50,
        Some(Duration::from_millis(1)),
    )
    .unwrap();

    assert!(result.timed_out, "1ms should be too short for cargo");
    assert_eq!(result.exit_code, -1);
    assert!(
        result.stderr_tail.contains("timed out"),
        "stderr should mention timeout: {}",
        result.stderr_tail
    );
    assert!(
        result.stderr_tail.contains("cargo publish timed out after"),
        "expected human-readable timeout message, got: {}",
        result.stderr_tail
    );
}

// ── cargo_publish: failure handling ────────────────────────────────────

#[test]
fn publish_fails_for_nonexistent_package() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result = cargo_publish(
        tmp.path(),
        "nonexistent-pkg",
        "crates-io",
        true,
        true,
        50,
        None,
    )
    .unwrap();

    assert_ne!(result.exit_code, 0);
    assert!(!result.timed_out);
    assert!(!result.stderr_tail.is_empty());
}

#[test]
fn publish_returns_positive_duration_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let result = cargo_publish(
        tmp.path(),
        "nonexistent-pkg",
        "crates-io",
        true,
        true,
        50,
        None,
    )
    .unwrap();

    assert!(result.duration > Duration::ZERO);
}

// ── load_metadata with temp crate ──────────────────────────────────────

#[test]
fn load_metadata_works_for_temp_crate() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = load_metadata(&manifest).expect("should load metadata for temp crate");

    assert!(!meta.packages.is_empty());
    assert!(meta.packages.iter().any(|p| p.name == "test-crate"));
}

#[test]
fn load_metadata_works_for_temp_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = load_metadata(&manifest).expect("should load workspace metadata");

    assert!(meta.packages.iter().any(|p| p.name == "crate-a"));
    assert!(meta.packages.iter().any(|p| p.name == "crate-b"));
}

// ── WorkspaceMetadata with temp crate ──────────────────────────────────

#[test]
fn workspace_metadata_loads_temp_crate() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load temp crate metadata");

    assert!(meta.workspace_root().exists());
    assert!(!meta.all_packages().is_empty());
}

#[test]
fn workspace_metadata_publishable_packages_temp_crate() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load metadata");
    let publishable = meta.publishable_packages();

    assert!(
        publishable.iter().any(|p| p.name == "test-crate"),
        "test-crate should be publishable"
    );
}

#[test]
fn workspace_metadata_workspace_name_temp_crate() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_crate(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load metadata");

    assert_eq!(meta.workspace_name(), "test-crate");
}

#[test]
fn workspace_metadata_topological_order_temp_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load metadata");
    let order = meta.topological_order().expect("should compute topo order");

    assert!(order.contains(&"crate-a".to_string()));
    assert!(order.contains(&"crate-b".to_string()));
}

#[test]
fn workspace_metadata_get_package_temp_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load metadata");

    assert!(meta.get_package("crate-a").is_some());
    assert!(meta.get_package("crate-b").is_some());
    assert!(meta.get_package("nonexistent").is_none());
}

#[test]
fn workspace_metadata_workspace_members_temp_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    create_minimal_workspace(tmp.path());

    let manifest = tmp.path().join("Cargo.toml");
    let meta = WorkspaceMetadata::load(&manifest).expect("load metadata");
    let members = meta.workspace_members();

    assert_eq!(members.len(), 2);
    let names: Vec<&str> = members.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"crate-a"));
    assert!(names.contains(&"crate-b"));
}
