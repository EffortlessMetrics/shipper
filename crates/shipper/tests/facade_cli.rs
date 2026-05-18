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
