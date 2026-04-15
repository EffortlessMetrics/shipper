//! Cross-platform edge-case tests for `ops::process`.

use std::time::Duration;

use super::*;

// Command quoting: args with spaces

#[test]
fn run_command_in_dir_with_spaces_in_path() {
    let td = tempfile::tempdir().expect("tmpdir");
    let spaced = td.path().join("dir with spaces");
    std::fs::create_dir_all(&spaced).expect("mkdir");

    #[cfg(windows)]
    {
        let r = run_command_in_dir("cmd", &["/C", "cd"], &spaced).expect("run");
        assert!(r.success);
    }
    #[cfg(not(windows))]
    {
        let r = run_command_in_dir("pwd", &[], &spaced).expect("run");
        assert!(r.success);
    }
}

// Command with Unicode path

#[test]
fn run_command_in_dir_unicode_path() {
    let td = tempfile::tempdir().expect("tmpdir");
    let unicode_dir = td.path().join("données");
    std::fs::create_dir_all(&unicode_dir).expect("mkdir");

    #[cfg(windows)]
    {
        let r = run_command_in_dir("cmd", &["/C", "echo ok"], &unicode_dir).expect("run");
        assert!(r.success);
    }
    #[cfg(not(windows))]
    {
        let r = run_command_in_dir("echo", &["ok"], &unicode_dir).expect("run");
        assert!(r.success);
    }
}

// CommandResult from_output with non-UTF8 bytes

#[test]
fn command_result_from_output_non_utf8_stdout() {
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: vec![0xFF, 0xFE, b'h', b'i'],
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.success);
    assert!(r.stdout.contains("hi"));
}

// CommandOutput fields when no timeout

#[test]
fn command_output_no_timeout_fields() {
    let td = tempfile::tempdir().expect("tmpdir");
    let r = run_command_with_timeout("cargo", &["--version"], td.path(), None).expect("run");
    assert!(!r.timed_out);
    assert_eq!(r.exit_code, 0);
    assert!(!r.stdout.is_empty());
}

// run_command_with_env empty env list

#[test]
fn run_command_with_empty_env_list() {
    let r = run_command_with_env("cargo", &["--version"], &[]).expect("run");
    assert!(r.success);
    assert!(r.stdout.contains("cargo"));
}

// command_exists with empty string

#[test]
fn command_exists_empty_string() {
    assert!(!command_exists(""));
}

// which returns None for empty string

#[test]
fn which_empty_string_returns_none() {
    assert!(which("").is_none());
}

/// Helper for creating ExitStatus
fn make_exit_status(code: i32) -> std::process::ExitStatus {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status()
            .expect("cmd exit")
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .status()
            .expect("sh exit")
    }
}
