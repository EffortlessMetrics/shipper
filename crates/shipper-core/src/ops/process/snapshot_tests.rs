//! Snapshot tests for `ops::process` serialization formats.

use std::time::Duration;

use insta::assert_yaml_snapshot;

use super::*;

// CommandResult snapshots

#[test]
fn command_result_success() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "hello world\n".to_string(),
        stderr: String::new(),
        duration_ms: 42,
    };
    assert_yaml_snapshot!(result);
}

#[test]
fn command_result_failure_with_exit_code() {
    let result = CommandResult {
        success: false,
        exit_code: Some(1),
        stdout: String::new(),
        stderr: "error: could not compile `foo`\n".to_string(),
        duration_ms: 1500,
    };
    assert_yaml_snapshot!(result);
}

#[test]
fn command_result_failure_no_exit_code() {
    let result = CommandResult {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: "process terminated by signal".to_string(),
        duration_ms: 300,
    };
    assert_yaml_snapshot!(result);
}

#[test]
fn command_result_with_multiline_output() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "line 1\nline 2\nline 3\n".to_string(),
        stderr: "warning: unused variable\n".to_string(),
        duration_ms: 200,
    };
    assert_yaml_snapshot!(result);
}

// CommandOutput snapshots

#[test]
fn command_output_success() {
    let output = CommandOutput {
        exit_code: 0,
        stdout: "cargo 1.80.0\n".to_string(),
        stderr: String::new(),
        timed_out: false,
        duration: Duration::from_millis(150),
    };
    assert_yaml_snapshot!(output);
}

#[test]
fn command_output_failure() {
    let output = CommandOutput {
        exit_code: 101,
        stdout: String::new(),
        stderr: "error[E0425]: cannot find value `x`\n".to_string(),
        timed_out: false,
        duration: Duration::from_millis(3200),
    };
    assert_yaml_snapshot!(output);
}

#[test]
fn command_output_timed_out() {
    let output = CommandOutput {
        exit_code: -1,
        stdout: String::new(),
        stderr: "partial output\ncargo timed out after 30s".to_string(),
        timed_out: true,
        duration: Duration::from_secs(30),
    };
    assert_yaml_snapshot!(output);
}

// Error message snapshots

#[test]
fn error_message_with_exit_code() {
    let result = CommandResult {
        success: false,
        exit_code: Some(127),
        stdout: String::new(),
        stderr: "command not found: foo".to_string(),
        duration_ms: 5,
    };
    let err = result.ok().unwrap_err();
    assert_yaml_snapshot!(err.to_string());
}

#[test]
fn error_message_without_exit_code() {
    let result = CommandResult {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: "killed by signal 9".to_string(),
        duration_ms: 0,
    };
    let err = result.ok().unwrap_err();
    assert_yaml_snapshot!(err.to_string());
}

#[test]
fn error_message_empty_stderr() {
    let result = CommandResult {
        success: false,
        exit_code: Some(2),
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 10,
    };
    let err = result.ok().unwrap_err();
    assert_yaml_snapshot!(err.to_string());
}

// Serialization format snapshots

#[test]
fn command_result_json_format() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "ok".to_string(),
        stderr: String::new(),
        duration_ms: 100,
    };
    let json: serde_json::Value = serde_json::to_value(&result).expect("serialize");
    assert_yaml_snapshot!(json);
}

#[test]
fn command_output_json_format() {
    let output = CommandOutput {
        exit_code: 1,
        stdout: "partial".to_string(),
        stderr: "fail".to_string(),
        timed_out: false,
        duration: Duration::from_millis(750),
    };
    let json: serde_json::Value = serde_json::to_value(&output).expect("serialize");
    assert_yaml_snapshot!(json);
}

// Snapshot: publish arg lists

#[test]
fn snapshot_publish_args_basic() {
    let args = vec!["publish", "--manifest-path", "crates/foo/Cargo.toml"];
    assert_yaml_snapshot!(args);
}

#[test]
fn snapshot_publish_args_with_registry_and_no_verify() {
    let args = vec![
        "publish",
        "--manifest-path",
        "crates/bar/Cargo.toml",
        "--registry",
        "my-private-registry",
        "--no-verify",
    ];
    assert_yaml_snapshot!(args);
}

#[test]
fn snapshot_publish_args_full_flags() {
    let args = vec![
        "publish",
        "--manifest-path",
        "crates/baz/Cargo.toml",
        "--registry",
        "crates-io",
        "--no-verify",
        "--features",
        "serde,tokio",
    ];
    assert_yaml_snapshot!(args);
}
