use std::process::Command;

fn shipper_facade() -> Command {
    Command::new(env!("CARGO_BIN_EXE_shipper"))
}

fn normalize_windows_binary_suffix(raw: String) -> String {
    raw.replace("shipper.exe", "shipper")
}

#[test]
fn facade_help_uses_shipper_binary_name() {
    let output = shipper_facade()
        .arg("--help")
        .output()
        .expect("run shipper --help");

    assert!(output.status.success());
    let stdout =
        normalize_windows_binary_suffix(String::from_utf8(output.stdout).expect("help is utf8"));

    assert!(stdout.contains("Usage: shipper [OPTIONS] <COMMAND>"));
    assert!(!stdout.contains("shipper-cli"));
}

#[test]
fn facade_subcommand_help_uses_shipper_binary_name() {
    let output = shipper_facade()
        .args(["doctor", "--help"])
        .output()
        .expect("run shipper doctor --help");

    assert!(output.status.success());
    let stdout =
        normalize_windows_binary_suffix(String::from_utf8(output.stdout).expect("help is utf8"));

    assert!(stdout.contains("Usage: shipper doctor [OPTIONS]"));
    assert!(stdout.contains("EXAMPLES:"));
    assert!(!stdout.contains("shipper-cli"));
}

/// Parity guard for the centralized error renderer (PR #417 regression):
/// the `shipper` facade binary must surface errors with the `Error:` prefix
/// and the multi-line `Caused by:` section — never the flattened single-line
/// `{e:#}` form. The `shipper-cli` binary is covered by the same centralized
/// `shipper_cli::report_error` plus e2e_expanded snapshots; this test proves
/// the facade reaches the same renderer. If it fails, do not relax the
/// assertion — the renderer diverged or regressed.
#[test]
fn facade_error_output_uses_multi_line_cause_chain() {
    use std::fs;
    let td = tempfile::tempdir().expect("tempdir");
    let cfg = td.path().join("bad.toml");
    // Malformed TOML so the config loader returns a two-level anyhow chain
    // (Failed to load -> Failed to parse -> TOML parse error).
    fs::write(&cfg, "this is = = not valid toml\n").expect("write");

    let output = shipper_facade()
        .args(["config", "validate", "-p"])
        .arg(&cfg)
        .output()
        .expect("run shipper config validate");

    assert!(
        !output.status.success(),
        "config validate on malformed TOML should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.starts_with("Error: "),
        "missing `Error:` prefix; got:\n{stderr}"
    );
    assert!(
        stderr.contains("\n\nCaused by:"),
        "missing blank-line + `Caused by:` section (regressed to single-line); got:\n{stderr}"
    );
    // The flattened `{e:#}` form joins with `: ` on one line — its absence is
    // the regression signal.
    let first_line = stderr.lines().next().unwrap();
    assert!(
        !(first_line.contains(": Failed to parse") || first_line.contains(": TOML parse")),
        "error chain collapsed to single line (`{{e:#}}` regression); got:\n{stderr}"
    );
}
