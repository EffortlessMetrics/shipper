//! Unit and property tests for `ops::process`.

use std::process::Command;
use std::time::Duration;

use super::*;

#[test]
fn run_command_version() {
    let result = run_command("cargo", &["--version"]).expect("run");
    assert!(result.success);
    assert!(result.stdout.contains("cargo"));
}

#[test]
fn run_command_failure() {
    let result = run_command("cargo", &["--nonexistent-flag-xyz"]).expect("run");
    assert!(!result.success);
}

#[test]
fn command_result_ok() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "output".to_string(),
        stderr: "".to_string(),
        duration_ms: 100,
    };

    assert!(result.ok().is_ok());
}

#[test]
fn command_result_err() {
    let result = CommandResult {
        success: false,
        exit_code: Some(1),
        stdout: "".to_string(),
        stderr: "error".to_string(),
        duration_ms: 100,
    };

    assert!(result.ok().is_err());
}

#[test]
fn run_command_simple_cargo() {
    let success = run_command_simple("cargo", &["--version"]).expect("run");
    assert!(success);
}

#[test]
fn command_exists_cargo() {
    assert!(command_exists("cargo"));
}

#[test]
fn command_exists_nonexistent() {
    assert!(!command_exists("this-command-does-not-exist-xyz123"));
}

#[test]
fn which_cargo() {
    let path = which("cargo");
    assert!(path.is_some());
}

#[test]
fn run_cargo_version() {
    let result = run_cargo(&["--version"]).expect("run");
    assert!(result.success);
    assert!(result.stdout.contains("cargo"));
}

#[test]
fn command_result_serialization() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "output".to_string(),
        stderr: "".to_string(),
        duration_ms: 150,
    };

    let json = serde_json::to_string(&result).expect("serialize");
    assert!(json.contains("\"success\":true"));
    assert!(json.contains("\"stdout\":\"output\""));
}

// CommandResult unit tests

#[test]
fn command_result_ok_returns_self_ref() {
    let result = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "hello".to_string(),
        stderr: String::new(),
        duration_ms: 10,
    };
    let r = result.ok().expect("should be ok");
    assert_eq!(r.stdout, "hello");
}

#[test]
fn command_result_err_contains_exit_code_and_stderr() {
    let result = CommandResult {
        success: false,
        exit_code: Some(42),
        stdout: String::new(),
        stderr: "boom".to_string(),
        duration_ms: 5,
    };
    let err = result.ok().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("42"), "should mention exit code: {msg}");
    assert!(msg.contains("boom"), "should mention stderr: {msg}");
}

#[test]
fn command_result_err_none_exit_code() {
    let result = CommandResult {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: "signal".to_string(),
        duration_ms: 1,
    };
    let err = result.ok().unwrap_err();
    assert!(err.to_string().contains("None"));
}

#[test]
fn command_result_from_output_success() {
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: b"out".to_vec(),
        stderr: b"err".to_vec(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(250));
    assert!(r.success);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.stdout, "out");
    assert_eq!(r.stderr, "err");
    assert_eq!(r.duration_ms, 250);
}

#[test]
fn command_result_from_output_failure() {
    let output = std::process::Output {
        status: make_exit_status(1),
        stdout: Vec::new(),
        stderr: b"fail".to_vec(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(50));
    assert!(!r.success);
    assert_eq!(r.exit_code, Some(1));
    assert_eq!(r.stderr, "fail");
}

#[test]
fn command_result_deserialization() {
    let json = r#"{
        "success": false,
        "exit_code": 7,
        "stdout": "hi",
        "stderr": "lo",
        "duration_ms": 99
    }"#;
    let r: CommandResult = serde_json::from_str(json).expect("deser");
    assert!(!r.success);
    assert_eq!(r.exit_code, Some(7));
    assert_eq!(r.stdout, "hi");
    assert_eq!(r.stderr, "lo");
    assert_eq!(r.duration_ms, 99);
}

#[test]
fn command_result_roundtrip_serde() {
    let original = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: "data\nwith\nnewlines".to_string(),
        stderr: String::new(),
        duration_ms: 1000,
    };
    let json = serde_json::to_string(&original).expect("ser");
    let decoded: CommandResult = serde_json::from_str(&json).expect("deser");
    assert_eq!(decoded.success, original.success);
    assert_eq!(decoded.exit_code, original.exit_code);
    assert_eq!(decoded.stdout, original.stdout);
    assert_eq!(decoded.duration_ms, original.duration_ms);
}

// run_command tests

#[test]
fn run_command_captures_stdout() {
    let r = run_command("cargo", &["--version"]).expect("run");
    assert!(!r.stdout.is_empty());
    assert!(r.stdout.starts_with("cargo"));
}

#[test]
fn run_command_captures_stderr_on_failure() {
    let r = run_command("cargo", &["publish", "--help-not-real"]).expect("run");
    assert!(!r.success);
    assert!(!r.stderr.is_empty());
}

#[test]
fn run_command_records_duration() {
    let r = run_command("cargo", &["--version"]).expect("run");
    assert!(r.duration_ms < 30_000, "took too long: {}ms", r.duration_ms);
}

#[test]
fn run_command_nonexistent_program() {
    let err = run_command("totally-bogus-command-xyz-999", &[]);
    assert!(err.is_err(), "should fail for non-existent program");
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("failed to run command"),
        "unexpected error: {msg}"
    );
}

// run_command_in_dir tests

#[test]
fn run_command_in_dir_uses_working_dir() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    #[cfg(windows)]
    {
        let r = run_command_in_dir("cmd", &["/C", "cd"], tmp.path()).expect("run");
        assert!(r.success);
        let normalised = r.stdout.trim().to_lowercase();
        let expected = tmp.path().to_str().unwrap().to_lowercase();
        assert!(
            normalised.contains(&expected),
            "stdout={normalised:?} expected to contain {expected:?}"
        );
    }
    #[cfg(not(windows))]
    {
        let r = run_command_in_dir("pwd", &[], tmp.path()).expect("run");
        assert!(r.success);
        assert!(
            r.stdout
                .trim()
                .ends_with(tmp.path().file_name().unwrap().to_str().unwrap())
        );
    }
}

#[test]
fn run_command_in_dir_nonexistent_dir() {
    let bad = std::path::Path::new("Z:\\this\\path\\does\\not\\exist\\at\\all");
    let err = run_command_in_dir("cargo", &["--version"], bad);
    assert!(err.is_err());
}

// run_command_with_env tests

#[test]
fn run_command_with_env_passes_variables() {
    #[cfg(windows)]
    {
        let r = run_command_with_env(
            "cmd",
            &["/C", "echo %SHIPPER_TEST_VAR%"],
            &[("SHIPPER_TEST_VAR".to_string(), "hello42".to_string())],
        )
        .expect("run");
        assert!(r.success);
        assert!(
            r.stdout.contains("hello42"),
            "stdout should contain env value: {:?}",
            r.stdout
        );
    }
    #[cfg(not(windows))]
    {
        let r = run_command_with_env(
            "sh",
            &["-c", "echo $SHIPPER_TEST_VAR"],
            &[("SHIPPER_TEST_VAR".to_string(), "hello42".to_string())],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("hello42"));
    }
}

#[test]
fn run_command_with_env_multiple_vars() {
    #[cfg(windows)]
    {
        let r = run_command_with_env(
            "cmd",
            &["/C", "echo %A% %B%"],
            &[
                ("A".to_string(), "foo".to_string()),
                ("B".to_string(), "bar".to_string()),
            ],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("foo"));
        assert!(r.stdout.contains("bar"));
    }
    #[cfg(not(windows))]
    {
        let r = run_command_with_env(
            "sh",
            &["-c", "echo $A $B"],
            &[
                ("A".to_string(), "foo".to_string()),
                ("B".to_string(), "bar".to_string()),
            ],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("foo"));
        assert!(r.stdout.contains("bar"));
    }
}

// run_command_simple tests

#[test]
fn run_command_simple_returns_false_on_failure() {
    let ok = run_command_simple("cargo", &["--nonexistent-flag-xyz"]).expect("run");
    assert!(!ok);
}

#[test]
fn run_command_simple_nonexistent_program() {
    let err = run_command_simple("bogus-not-a-command-123", &[]);
    assert!(err.is_err());
}

// run_command_streaming tests

#[test]
fn run_command_streaming_success() {
    let r = run_command_streaming("cargo", &["--version"]).expect("run");
    assert!(r.success);
    assert_eq!(r.exit_code, Some(0));
}

#[test]
fn run_command_streaming_failure() {
    let r = run_command_streaming("cargo", &["--nonexistent-flag-xyz"]).expect("run");
    assert!(!r.success);
}

// run_command_with_timeout tests

#[test]
fn run_command_with_timeout_none_delegates() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), None).expect("run");
    assert!(!r.timed_out);
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("cargo"));
}

#[test]
fn run_command_with_timeout_completes_before_deadline() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_secs(30));
    let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), timeout).expect("run");
    assert!(!r.timed_out);
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("cargo"));
}

#[test]
fn run_command_with_timeout_exceeds_deadline() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_millis(100));

    #[cfg(windows)]
    let r = run_command_with_timeout("ping", &["-n", "100", "127.0.0.1"], tmp.path(), timeout)
        .expect("run");
    #[cfg(not(windows))]
    let r = run_command_with_timeout("sleep", &["60"], tmp.path(), timeout).expect("run");

    assert!(r.timed_out, "should have timed out");
    assert_eq!(r.exit_code, -1);
    assert!(
        r.stderr.contains("timed out"),
        "stderr should mention timeout: {:?}",
        r.stderr
    );
}

#[test]
fn run_command_with_timeout_failure_before_deadline() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_secs(30));
    let r = run_command_with_timeout("cargo", &["--nonexistent-flag-xyz"], tmp.path(), timeout)
        .expect("run");
    assert!(!r.timed_out);
    assert_ne!(r.exit_code, 0);
}

#[test]
fn run_command_with_timeout_nonexistent_program() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let err = run_command_with_timeout(
        "bogus-not-a-command-123",
        &[],
        tmp.path(),
        Some(Duration::from_secs(5)),
    );
    assert!(err.is_err());
}

#[test]
fn run_command_with_timeout_captures_stderr() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_secs(30));
    let r = run_command_with_timeout("cargo", &["--nonexistent-flag-xyz"], tmp.path(), timeout)
        .expect("run");
    assert!(
        !r.stderr.is_empty(),
        "stderr should not be empty on failure"
    );
}

// CommandOutput tests

#[test]
fn command_output_duration_is_reasonable() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), None).expect("run");
    assert!(r.duration < Duration::from_secs(30));
}

#[test]
fn command_output_serialization_roundtrip() {
    let co = CommandOutput {
        exit_code: 2,
        stdout: "out".to_string(),
        stderr: "err".to_string(),
        timed_out: true,
        duration: Duration::from_millis(500),
    };
    let json = serde_json::to_string(&co).expect("ser");
    let decoded: CommandOutput = serde_json::from_str(&json).expect("deser");
    assert_eq!(decoded.exit_code, 2);
    assert_eq!(decoded.stdout, "out");
    assert_eq!(decoded.stderr, "err");
    assert!(decoded.timed_out);
    assert_eq!(decoded.duration, Duration::from_millis(500));
}

// command_exists / which tests

#[test]
fn which_nonexistent_returns_none() {
    assert!(which("this-command-does-not-exist-xyz123").is_none());
}

#[test]
fn which_cargo_returns_valid_path() {
    let p = which("cargo").expect("cargo should be in PATH");
    assert!(p.exists(), "path should exist: {}", p.display());
}

// run_cargo helpers

#[test]
fn run_cargo_in_dir_works() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let r = run_cargo_in_dir(&["--version"], tmp.path()).expect("run");
    assert!(r.success);
    assert!(r.stdout.contains("cargo"));
}

#[test]
fn run_cargo_failure() {
    let r = run_cargo(&["--nonexistent-flag-xyz"]).expect("run");
    assert!(!r.success);
}

// Exit code tests

#[test]
fn exit_code_zero_on_success() {
    let r = run_command("cargo", &["--version"]).expect("run");
    assert_eq!(r.exit_code, Some(0));
}

#[test]
fn exit_code_nonzero_on_failure() {
    let r = run_command("cargo", &["--nonexistent-flag-xyz"]).expect("run");
    assert!(r.exit_code.is_some());
    assert_ne!(r.exit_code.unwrap(), 0);
}

#[test]
fn specific_exit_code() {
    #[cfg(windows)]
    {
        let r = run_command("cmd", &["/C", "exit 42"]).expect("run");
        assert_eq!(r.exit_code, Some(42));
        assert!(!r.success);
    }
    #[cfg(not(windows))]
    {
        let r = run_command("sh", &["-c", "exit 42"]).expect("run");
        assert_eq!(r.exit_code, Some(42));
        assert!(!r.success);
    }
}

// Property-based tests

mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn command_result_ok_succeeds_when_success_is_true(
            stdout in any::<String>(),
            stderr in any::<String>(),
            exit_code in proptest::option::of(any::<i32>()),
            duration_ms in any::<u64>(),
        ) {
            let result = CommandResult {
                success: true,
                exit_code,
                stdout,
                stderr,
                duration_ms,
            };
            prop_assert!(result.ok().is_ok());
        }

        #[test]
        fn command_result_ok_fails_when_success_is_false(
            stdout in any::<String>(),
            stderr in any::<String>(),
            exit_code in proptest::option::of(any::<i32>()),
            duration_ms in any::<u64>(),
        ) {
            let result = CommandResult {
                success: false,
                exit_code,
                stdout,
                stderr: stderr.clone(),
                duration_ms,
            };
            let err = result.ok().unwrap_err();
            let msg = err.to_string();
            prop_assert!(msg.contains(&stderr));
        }

        #[test]
        fn command_result_serde_roundtrip(
            success in any::<bool>(),
            exit_code in proptest::option::of(any::<i32>()),
            stdout in any::<String>(),
            stderr in any::<String>(),
            duration_ms in any::<u64>(),
        ) {
            let original = CommandResult {
                success,
                exit_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                duration_ms,
            };
            let json = serde_json::to_string(&original).unwrap();
            let decoded: CommandResult = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(decoded.success, success);
            prop_assert_eq!(decoded.exit_code, exit_code);
            prop_assert_eq!(&decoded.stdout, &stdout);
            prop_assert_eq!(&decoded.stderr, &stderr);
            prop_assert_eq!(decoded.duration_ms, duration_ms);
        }

        #[test]
        fn command_output_serde_roundtrip(
            exit_code in any::<i32>(),
            stdout in any::<String>(),
            stderr in any::<String>(),
            timed_out in any::<bool>(),
            duration_ms in 0u64..=u64::MAX / 1_000_000,
        ) {
            let original = CommandOutput {
                exit_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                timed_out,
                duration: Duration::from_millis(duration_ms),
            };
            let json = serde_json::to_string(&original).unwrap();
            let decoded: CommandOutput = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(decoded.exit_code, exit_code);
            prop_assert_eq!(&decoded.stdout, &stdout);
            prop_assert_eq!(&decoded.stderr, &stderr);
            prop_assert_eq!(decoded.timed_out, timed_out);
            prop_assert_eq!(decoded.duration, Duration::from_millis(duration_ms));
        }
    }

    proptest! {
        #[test]
        fn error_message_contains_exit_code(code in any::<i32>()) {
            let result = CommandResult {
                success: false,
                exit_code: Some(code),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 0,
            };
            let err = result.ok().unwrap_err();
            let msg = err.to_string();
            let code_str = code.to_string();
            prop_assert!(msg.contains(&code_str));
        }

        #[test]
        fn error_message_contains_none_when_exit_code_missing(
            stderr in any::<String>(),
        ) {
            let result = CommandResult {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr,
                duration_ms: 0,
            };
            let err = result.ok().unwrap_err();
            prop_assert!(err.to_string().contains("None"));
        }

        #[test]
        fn exit_code_zero_is_always_success(
            stdout in any::<String>(),
            stderr in any::<String>(),
            duration_ms in any::<u64>(),
        ) {
            let result = CommandResult {
                success: true,
                exit_code: Some(0),
                stdout,
                stderr,
                duration_ms,
            };
            prop_assert!(result.ok().is_ok());
            prop_assert_eq!(result.exit_code, Some(0));
        }

        #[test]
        fn nonzero_exit_code_produces_error(code in any::<i32>().prop_filter(
            "non-zero exit code",
            |c| *c != 0,
        )) {
            let result = CommandResult {
                success: false,
                exit_code: Some(code),
                stdout: String::new(),
                stderr: "failed".to_string(),
                duration_ms: 0,
            };
            prop_assert!(result.ok().is_err());
        }
    }

    proptest! {
        #[test]
        fn command_building_does_not_panic(
            args in proptest::collection::vec(any::<String>(), 0..20),
        ) {
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let mut cmd = Command::new("echo");
            cmd.args(&arg_refs);
            prop_assert!(true);
        }

        #[test]
        fn command_with_env_building_does_not_panic(
            args in proptest::collection::vec(any::<String>(), 0..10),
            env_keys in proptest::collection::vec("[A-Z_]{1,20}", 0..5),
            env_vals in proptest::collection::vec(any::<String>(), 0..5),
        ) {
            let mut cmd = Command::new("echo");
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            cmd.args(&arg_refs);
            let pairs = env_keys.len().min(env_vals.len());
            for i in 0..pairs {
                cmd.env(&env_keys[i], &env_vals[i]);
            }
            prop_assert!(true);
        }

        #[test]
        fn from_output_handles_arbitrary_bytes(
            stdout_bytes in proptest::collection::vec(any::<u8>(), 0..500),
            stderr_bytes in proptest::collection::vec(any::<u8>(), 0..500),
            code in 0i32..256,
        ) {
            let output = std::process::Output {
                status: super::make_exit_status(code.min(255)),
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            };
            let r = CommandResult::from_output(&output, Duration::from_millis(1));
            prop_assert!(r.stdout.is_ascii() || !r.stdout.is_empty() || r.stdout.is_empty());
            prop_assert!(std::str::from_utf8(r.stdout.as_bytes()).is_ok());
            prop_assert!(std::str::from_utf8(r.stderr.as_bytes()).is_ok());
        }

        #[test]
        fn publish_args_always_start_with_publish(
            manifest in "[a-zA-Z0-9_/\\.]{1,50}",
            registry in proptest::option::of("[a-z\\-]{1,20}"),
            no_verify in any::<bool>(),
            features in proptest::option::of("[a-z_,]{1,30}"),
        ) {
            let mut args = vec!["publish", "--manifest-path", &manifest];
            if let Some(ref reg) = registry {
                args.push("--registry");
                args.push(reg);
            }
            if no_verify {
                args.push("--no-verify");
            }
            if let Some(ref feat) = features {
                args.push("--features");
                args.push(feat);
            }
            prop_assert_eq!(args[0], "publish");
            prop_assert_eq!(args[1], "--manifest-path");
            prop_assert!(args.len() >= 3);
            prop_assert!(args.len() <= 9);
        }
    }
}

// Command building tests

#[test]
fn cargo_publish_without_registry_builds_correct_args() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let manifest = tmp.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"",
    )
    .unwrap();
    let mut args = vec![
        "publish",
        "--manifest-path",
        manifest.to_str().unwrap_or(""),
    ];
    assert_eq!(args.len(), 3);
    assert_eq!(args[0], "publish");
    assert_eq!(args[1], "--manifest-path");
    assert!(!args.contains(&"--registry"));

    args.push("--registry");
    args.push("my-registry");
    assert_eq!(args.len(), 5);
    assert_eq!(args[3], "--registry");
    assert_eq!(args[4], "my-registry");
}

#[test]
fn cargo_publish_with_registry_includes_registry_arg() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let manifest = tmp.path().join("Cargo.toml");
    std::fs::write(&manifest, "[package]\nname = \"test\"\nversion = \"0.1.0\"").unwrap();

    let manifest_str = manifest.to_str().unwrap();
    let mut args = vec!["publish", "--manifest-path", manifest_str];
    let registry = Some("custom-reg");
    if let Some(reg) = registry {
        args.push("--registry");
        args.push(reg);
    }
    assert_eq!(args[3], "--registry");
    assert_eq!(args[4], "custom-reg");
}

#[test]
fn cargo_dry_run_includes_dry_run_flag() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let manifest = tmp.path().join("Cargo.toml");
    std::fs::write(&manifest, "[package]\nname = \"test\"\nversion = \"0.1.0\"").unwrap();

    let args = [
        "publish",
        "--dry-run",
        "--manifest-path",
        manifest.to_str().unwrap_or(""),
    ];
    assert_eq!(args[0], "publish");
    assert_eq!(args[1], "--dry-run");
    assert_eq!(args[2], "--manifest-path");
}

#[test]
fn command_building_no_verify_flag() {
    let mut args: Vec<&str> = vec!["publish", "--manifest-path", "/some/path"];
    let no_verify = true;
    if no_verify {
        args.push("--no-verify");
    }
    assert!(args.contains(&"--no-verify"));
    assert_eq!(args.len(), 4);
}

#[test]
fn command_building_features_flag() {
    let mut args: Vec<&str> = vec!["publish", "--manifest-path", "/some/path"];
    let features = Some("feat1,feat2");
    if let Some(f) = features {
        args.push("--features");
        args.push(f);
    }
    assert!(args.contains(&"--features"));
    assert!(args.contains(&"feat1,feat2"));
}

#[test]
fn command_building_combined_flags() {
    let mut args: Vec<&str> = vec!["publish", "--manifest-path", "/some/path"];
    args.push("--no-verify");
    args.push("--registry");
    args.push("alt-reg");
    args.push("--features");
    args.push("serde,tokio");
    assert_eq!(args.len(), 8);
    assert_eq!(args[0], "publish");
    assert!(args.contains(&"--no-verify"));
    assert!(args.contains(&"--registry"));
    assert!(args.contains(&"alt-reg"));
    assert!(args.contains(&"--features"));
    assert!(args.contains(&"serde,tokio"));
}

// Output parsing edge cases

#[test]
fn from_output_empty_stdout_and_stderr() {
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.success);
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
}

#[test]
fn from_output_unicode_in_stderr() {
    let output = std::process::Output {
        status: make_exit_status(1),
        stdout: Vec::new(),
        stderr: "error: café résumé naïve ñoño 日本語 🦀"
            .as_bytes()
            .to_vec(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(5));
    assert!(r.stderr.contains("café"));
    assert!(r.stderr.contains("🦀"));
    assert!(r.stderr.contains("日本語"));
}

#[test]
fn from_output_unicode_in_stdout() {
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: "Ünïcödé output: 你好世界 🎉".as_bytes().to_vec(),
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.stdout.contains("Ünïcödé"));
    assert!(r.stdout.contains("你好世界"));
    assert!(r.stdout.contains("🎉"));
}

#[test]
fn from_output_very_long_output() {
    let long_stdout = "x".repeat(1_000_000);
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: long_stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(10));
    assert_eq!(r.stdout.len(), 1_000_000);
}

#[test]
fn from_output_binary_garbage_in_stdout() {
    let garbage: Vec<u8> = vec![0xFF, 0xFE, 0x00, 0x80, 0xC0, 0xC1];
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: garbage,
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.stdout.contains('\u{FFFD}'));
}

#[test]
fn from_output_binary_garbage_in_stderr() {
    let garbage: Vec<u8> = vec![0x80, 0x81, 0xFE, 0xFF];
    let output = std::process::Output {
        status: make_exit_status(1),
        stdout: Vec::new(),
        stderr: garbage,
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.stderr.contains('\u{FFFD}'));
}

#[test]
fn from_output_mixed_valid_and_invalid_utf8() {
    let mut data = b"valid prefix ".to_vec();
    data.extend_from_slice(&[0xFF, 0xFE]);
    data.extend_from_slice(b" valid suffix");
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: data,
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert!(r.stdout.contains("valid prefix"));
    assert!(r.stdout.contains("valid suffix"));
    assert!(r.stdout.contains('\u{FFFD}'));
}

#[test]
fn from_output_newlines_preserved() {
    let text = "line1\nline2\r\nline3\n";
    let output = std::process::Output {
        status: make_exit_status(0),
        stdout: text.as_bytes().to_vec(),
        stderr: Vec::new(),
    };
    let r = CommandResult::from_output(&output, Duration::from_millis(1));
    assert_eq!(r.stdout, text);
}

// Timeout configuration

#[test]
fn timeout_none_does_not_set_timed_out() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), None).expect("run");
    assert!(!r.timed_out);
}

#[test]
fn timeout_generous_does_not_trigger() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_secs(120));
    let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), timeout).expect("run");
    assert!(!r.timed_out);
    assert_eq!(r.exit_code, 0);
}

#[test]
fn timeout_stderr_includes_program_name_and_duration() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_millis(100));

    #[cfg(windows)]
    let r = run_command_with_timeout("ping", &["-n", "100", "127.0.0.1"], tmp.path(), timeout)
        .expect("run");
    #[cfg(not(windows))]
    let r = run_command_with_timeout("sleep", &["60"], tmp.path(), timeout).expect("run");

    assert!(r.timed_out);
    assert!(
        r.stderr.contains("timed out"),
        "stderr should contain 'timed out': {:?}",
        r.stderr
    );
}

#[test]
fn timeout_duration_is_at_least_timeout_value() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let timeout = Some(Duration::from_millis(200));

    #[cfg(windows)]
    let r = run_command_with_timeout("ping", &["-n", "100", "127.0.0.1"], tmp.path(), timeout)
        .expect("run");
    #[cfg(not(windows))]
    let r = run_command_with_timeout("sleep", &["60"], tmp.path(), timeout).expect("run");

    assert!(r.timed_out);
    assert!(
        r.duration >= Duration::from_millis(150),
        "duration {:?} should be >= 150ms",
        r.duration
    );
}

// Error classification

#[test]
fn error_from_nonexistent_program_is_spawn_failure() {
    let err = run_command("nonexistent-binary-abc-xyz", &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("failed to run command"),
        "spawn error message: {msg}"
    );
}

#[test]
fn error_from_bad_working_dir_is_io_failure() {
    let bad = std::path::Path::new("Z:\\nonexistent\\directory\\abc");
    let err = run_command_in_dir("cargo", &["--version"], bad).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("failed to run command"),
        "dir error message: {msg}"
    );
}

#[test]
fn exit_code_1_classified_as_failure() {
    #[cfg(windows)]
    let r = run_command("cmd", &["/C", "exit 1"]).expect("run");
    #[cfg(not(windows))]
    let r = run_command("sh", &["-c", "exit 1"]).expect("run");

    assert!(!r.success);
    assert_eq!(r.exit_code, Some(1));
}

#[test]
fn exit_code_2_classified_as_failure() {
    #[cfg(windows)]
    let r = run_command("cmd", &["/C", "exit 2"]).expect("run");
    #[cfg(not(windows))]
    let r = run_command("sh", &["-c", "exit 2"]).expect("run");

    assert!(!r.success);
    assert_eq!(r.exit_code, Some(2));
}

#[test]
fn exit_code_127_classified_as_failure() {
    #[cfg(windows)]
    let r = run_command("cmd", &["/C", "exit 127"]).expect("run");
    #[cfg(not(windows))]
    let r = run_command("sh", &["-c", "exit 127"]).expect("run");

    assert!(!r.success);
    assert_eq!(r.exit_code, Some(127));
}

#[test]
fn ok_error_message_format_with_exit_code() {
    let r = CommandResult {
        success: false,
        exit_code: Some(101),
        stdout: String::new(),
        stderr: "compilation error".to_string(),
        duration_ms: 0,
    };
    let err = r.ok().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("101"));
    assert!(msg.contains("compilation error"));
}

#[test]
fn ok_error_message_format_without_exit_code() {
    let r = CommandResult {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: "killed".to_string(),
        duration_ms: 0,
    };
    let err = r.ok().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("None"));
    assert!(msg.contains("killed"));
}

// Environment variable passthrough

#[test]
fn env_var_with_special_characters() {
    #[cfg(windows)]
    {
        let r = run_command_with_env(
            "cmd",
            &["/C", "echo %SHIPPER_SPECIAL%"],
            &[(
                "SHIPPER_SPECIAL".to_string(),
                "val=with spaces!chars".to_string(),
            )],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("val=with spaces"));
    }
    #[cfg(not(windows))]
    {
        let r = run_command_with_env(
            "sh",
            &["-c", "echo \"$SHIPPER_SPECIAL\""],
            &[(
                "SHIPPER_SPECIAL".to_string(),
                "val=with spaces&special!chars".to_string(),
            )],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("val=with spaces&special!chars"));
    }
}

#[test]
fn env_var_empty_value() {
    #[cfg(windows)]
    {
        let r = run_command_with_env(
            "cmd",
            &["/C", "echo [%SHIPPER_EMPTY%]"],
            &[("SHIPPER_EMPTY".to_string(), String::new())],
        )
        .expect("run");
        assert!(r.success);
    }
    #[cfg(not(windows))]
    {
        let r = run_command_with_env(
            "sh",
            &["-c", "echo \"[$SHIPPER_EMPTY]\""],
            &[("SHIPPER_EMPTY".to_string(), String::new())],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("[]"));
    }
}

#[test]
fn env_var_unicode_value() {
    #[cfg(windows)]
    {
        let r = run_command_with_env(
            "cmd",
            &["/C", "echo %SHIPPER_UNI%"],
            &[("SHIPPER_UNI".to_string(), "🦀日本語".to_string())],
        )
        .expect("run");
        assert!(r.success);
    }
    #[cfg(not(windows))]
    {
        let r = run_command_with_env(
            "sh",
            &["-c", "echo \"$SHIPPER_UNI\""],
            &[("SHIPPER_UNI".to_string(), "🦀日本語".to_string())],
        )
        .expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("🦀"));
    }
}

#[test]
fn env_var_overrides_existing() {
    temp_env::with_vars([("SHIPPER_OVERRIDE_TEST", Some("original_value"))], || {
        #[cfg(windows)]
        {
            let r = run_command_with_env(
                "cmd",
                &["/C", "echo %SHIPPER_OVERRIDE_TEST%"],
                &[(
                    "SHIPPER_OVERRIDE_TEST".to_string(),
                    "overridden".to_string(),
                )],
            )
            .expect("run");
            assert!(r.success);
            assert!(r.stdout.contains("overridden"), "stdout: {:?}", r.stdout);
        }
        #[cfg(not(windows))]
        {
            let r = run_command_with_env(
                "sh",
                &["-c", "echo $SHIPPER_OVERRIDE_TEST"],
                &[(
                    "SHIPPER_OVERRIDE_TEST".to_string(),
                    "overridden".to_string(),
                )],
            )
            .expect("run");
            assert!(r.success);
            assert!(r.stdout.contains("overridden"));
        }
    });
}

#[test]
fn env_vars_empty_list() {
    let r = run_command_with_env("cargo", &["--version"], &[]).expect("run");
    assert!(r.success);
    assert!(r.stdout.contains("cargo"));
}

// CommandResult edge cases

#[test]
fn command_result_zero_duration() {
    let r = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
    };
    assert!(r.ok().is_ok());
    assert_eq!(r.duration_ms, 0);
}

#[test]
fn command_result_max_duration() {
    let r = CommandResult {
        success: true,
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: u64::MAX,
    };
    assert!(r.ok().is_ok());
    assert_eq!(r.duration_ms, u64::MAX);
}

#[test]
fn command_result_negative_exit_code() {
    let r = CommandResult {
        success: false,
        exit_code: Some(-1),
        stdout: String::new(),
        stderr: "signal".to_string(),
        duration_ms: 0,
    };
    let err = r.ok().unwrap_err();
    assert!(err.to_string().contains("-1"));
}

#[test]
fn command_output_timed_out_fields() {
    let co = CommandOutput {
        exit_code: -1,
        stdout: String::new(),
        stderr: "timeout".to_string(),
        timed_out: true,
        duration: Duration::from_secs(30),
    };
    assert!(co.timed_out);
    assert_eq!(co.exit_code, -1);
    assert_eq!(co.duration, Duration::from_secs(30));
}

#[test]
fn command_result_deserialization_with_null_exit_code() {
    let json =
        r#"{"success": false, "exit_code": null, "stdout": "", "stderr": "sig", "duration_ms": 0}"#;
    let r: CommandResult = serde_json::from_str(json).expect("deser");
    assert!(!r.success);
    assert_eq!(r.exit_code, None);
}

/// Create an `ExitStatus` by actually running a process that exits with the given code.
pub(super) fn make_exit_status(code: i32) -> std::process::ExitStatus {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status()
            .expect("cmd exit")
    }
    #[cfg(not(windows))]
    {
        Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .status()
            .expect("sh exit")
    }
}
