use std::process::Command;

use insta::assert_snapshot;

/// Normalize line endings, platform-specific binary names, and versions so
/// snapshots stay stable across environments.
fn normalize_output(raw: &str) -> String {
    raw.replace("\r\n", "\n")
        .replace("shipper.exe", "shipper")
        .replace(env!("CARGO_PKG_VERSION"), "[VERSION]")
}

fn normalize_help_output(raw: &str) -> String {
    normalize_output(raw)
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

// ── Help texts ───────────────────────────────────────────────────────

#[test]
fn help_text() {
    let output = shipper_cmd().arg("--help").output().expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_text", normalize_help_output(&stdout));
}

#[test]
fn plan_help() {
    let output = shipper_cmd()
        .args(["plan", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("plan_help", normalize_help_output(&stdout));
}

#[test]
fn publish_help() {
    let output = shipper_cmd()
        .args(["publish", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("publish_help", normalize_help_output(&stdout));
}

#[test]
fn resume_help() {
    let output = shipper_cmd()
        .args(["resume", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("resume_help", normalize_help_output(&stdout));
}

#[test]
fn preflight_help() {
    let output = shipper_cmd()
        .args(["preflight", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("preflight_help", normalize_help_output(&stdout));
}

#[test]
fn status_help() {
    let output = shipper_cmd()
        .args(["status", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("status_help", normalize_help_output(&stdout));
}

#[test]
fn doctor_help() {
    let output = shipper_cmd()
        .args(["doctor", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("doctor_help", normalize_help_output(&stdout));
}

#[test]
fn config_help() {
    let output = shipper_cmd()
        .args(["config", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("config_help", normalize_help_output(&stdout));
}

#[test]
fn ci_help() {
    let output = shipper_cmd()
        .args(["ci", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("ci_help", normalize_help_output(&stdout));
}

#[test]
fn clean_help() {
    let output = shipper_cmd()
        .args(["clean", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("clean_help", normalize_help_output(&stdout));
}

// ── Version ──────────────────────────────────────────────────────────

#[test]
fn version_flag() {
    let output = shipper_cmd()
        .arg("--version")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("version_flag", normalize_output(&stdout));
}

// ── Error cases ──────────────────────────────────────────────────────

#[test]
fn no_subcommand_shows_error() {
    let output = shipper_cmd().output().expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("no_subcommand_error", normalize_output(&stderr));
}

#[test]
fn unknown_subcommand_shows_error() {
    let output = shipper_cmd()
        .arg("nonexistent")
        .output()
        .expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("unknown_subcommand_error", normalize_output(&stderr));
}

#[test]
fn completion_missing_shell_arg() {
    let output = shipper_cmd()
        .arg("completion")
        .output()
        .expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("completion_missing_shell", normalize_output(&stderr));
}
