//! Returned evidence errors must stop real CLI containment after an observed effect.

use super::*;
use std::io::Write;

const CHILD: &str = "containment_context::containment_evidence::evidence_fault_child";
const SECRET: &str = "containment-cargo-secret-sentinel";
const RESPONSE_BODY: &str = "untrusted-containment-response-body-sentinel";

fn change_evidence(state: &Path, fault: &str) {
    let events = state.join("events.jsonl");
    let receipt = state.join("receipt.json");
    match fault {
        "none" => {}
        "events" => {
            if !events.is_dir() {
                fs::rename(&events, state.join("events.before-conflict.jsonl"))
                    .expect("preserve prior events");
                fs::create_dir(&events).expect("conflicting event directory");
            }
        }
        "receipt-write" => {
            fs::create_dir_all(state.join("receipt.tmp")).expect("conflicting receipt temp path");
        }
        "receipt-read" => {
            fs::rename(&receipt, state.join("receipt.before-conflict.json"))
                .expect("preserve receipt");
            fs::create_dir(&receipt).expect("conflicting receipt directory");
        }
        "receipt-missing" => {
            fs::rename(&receipt, state.join("receipt.before-conflict.json"))
                .expect("preserve missing receipt");
        }
        "receipt-malformed" => {
            fs::rename(&receipt, state.join("receipt.before-conflict.json"))
                .expect("preserve receipt before malformed replacement");
            fs::write(
                &receipt,
                format!("{{\"receipt_version\":\"{SECRET}\",\"packages\":\"{RESPONSE_BODY}\"}}"),
            )
            .expect("malformed replacement receipt");
            fs::copy(&receipt, state.join("receipt.after-conflict.json"))
                .expect("retain malformed replacement receipt");
        }
        "receipt-unmatched" => {
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
            value["packages"]
                .as_array_mut()
                .unwrap()
                .retain(|package| package["name"] != "core-lib");
            fs::write(&receipt, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
            fs::copy(&receipt, state.join("receipt.after-conflict.json"))
                .expect("retain the replacement receipt");
        }
        "receipt-registry" => {
            rewrite_registry(&receipt, "other-registry", true);
            fs::copy(&receipt, state.join("receipt.after-conflict.json"))
                .expect("retain receipt for the other registry");
        }
        _ => panic!("unexpected fixture fault: {fault}"),
    }
}

#[test]
#[serial_test::serial]
#[ignore = "fake Cargo subprocess invoked by the containment evidence tests"]
fn evidence_fault_child() {
    let state = PathBuf::from(std::env::var_os("SHIPPER_CONTAINMENT_STATE_DIR").unwrap());
    let calls = fs::read_to_string(std::env::var_os("SHIPPER_FAKE_CARGO_LOG").unwrap()).unwrap();
    assert!(calls.lines().last().unwrap().starts_with("yank "));
    let fault = std::env::var("SHIPPER_CONTAINMENT_FAULT").unwrap();
    change_evidence(&state, &fault);
    println!("Authorization: Bearer {SECRET}");
    eprintln!("{RESPONSE_BODY}");
    std::io::stdout().flush().unwrap();
    std::io::stderr().flush().unwrap();
    let exit_code = std::env::var("SHIPPER_CONTAINMENT_EXIT")
        .unwrap()
        .parse()
        .unwrap();
    std::process::exit(exit_code);
}

fn fault_command(fixture: &Fixture, fault: &str, quiet: bool, exit_code: i32) -> Command {
    let original = fs::read_to_string(&fixture.cargo).expect("Cargo shim");
    #[cfg(not(windows))]
    let (needle, replacement) = (
        "  exit 0",
        format!(
            "  exec \"$SHIPPER_CONTAINMENT_FIXTURE_EXE\" --exact {CHILD} --ignored --nocapture --quiet"
        ),
    );
    #[cfg(windows)]
    let (needle, replacement) = (
        "  exit /b 0",
        format!(
            "  \"%SHIPPER_CONTAINMENT_FIXTURE_EXE%\" --exact {CHILD} --ignored --nocapture --quiet\r\n  exit /b !ERRORLEVEL!"
        ),
    );
    assert_eq!(original.matches(needle).count(), 1);
    fs::write(&fixture.cargo, original.replace(needle, &replacement)).expect("install fault shim");
    let mut command = fixture.command();
    command
        .env(
            "SHIPPER_CONTAINMENT_FIXTURE_EXE",
            std::env::current_exe().unwrap(),
        )
        .env("SHIPPER_CONTAINMENT_STATE_DIR", &fixture.state)
        .env("SHIPPER_CONTAINMENT_FAULT", fault)
        .env("SHIPPER_CONTAINMENT_EXIT", exit_code.to_string())
        .args(["--registry", "private"]);
    if quiet {
        command.arg("--quiet");
    }
    command
}

fn failure(command: &mut Command, expected: &str) -> String {
    let output = command.output().expect("run containment failure");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains(expected), "{diagnostic}");
    assert!(!diagnostic.contains(SECRET), "{diagnostic}");
    assert!(!diagnostic.contains(RESPONSE_BODY), "{diagnostic}");
    diagnostic
}

fn assert_no_cargo(fixture: &Fixture) {
    assert!(!fixture.args_log.exists(), "no yank may run before refusal");
    assert!(!fixture.cwd_log.exists());
}

fn assert_one_cargo(fixture: &Fixture) {
    let (name, version) = match fixture.mode {
        Mode::Single => ("core-lib", "0.2.0"),
        Mode::YankPlan | Mode::Remediate => ("top-app", "0.4.0"),
    };
    assert_eq!(
        fs::read_to_string(&fixture.args_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [format!(
            "yank {name} --version={version} --registry private"
        )]
    );
    let cwd = fs::read_to_string(&fixture.cwd_log).unwrap();
    assert_eq!(cwd.lines().count(), 1);
    assert_eq!(
        fs::canonicalize(cwd.trim()).unwrap(),
        fs::canonicalize(&fixture.workspace).unwrap()
    );
}

fn path_state(path: &Path) -> (bool, bool, Option<Vec<u8>>) {
    (path.exists(), path.is_dir(), fs::read(path).ok())
}

#[test]
#[serial_test::serial]
fn containment_refuses_unpreparable_event_evidence_before_cargo() {
    for mode in MODES {
        for quiet in [false, true] {
            let fixture = Fixture::new(mode, "private", "relative");
            let receipt = fs::read(fixture.state.join("receipt.json")).unwrap();
            let prior_events = fs::read(fixture.state.join("events.jsonl")).unwrap();
            change_evidence(&fixture.state, "events");
            let diagnostic = failure(
                &mut fault_command(&fixture, "none", quiet, 0),
                "cannot prepare containment event evidence",
            );
            assert!(diagnostic.contains("no Cargo yank was started"));
            assert_no_cargo(&fixture);
            assert_eq!(
                fs::read(fixture.state.join("receipt.json")).unwrap(),
                receipt
            );
            assert_eq!(
                fs::read(fixture.state.join("events.before-conflict.jsonl")).unwrap(),
                prior_events
            );
            assert!(fixture.state.join("events.jsonl").is_dir());
        }
    }
}

#[test]
#[serial_test::serial]
fn containment_stops_after_observed_yank_when_event_append_fails() {
    for mode in MODES {
        for quiet in [false, true] {
            for exit_code in [0, 55] {
                let fixture = Fixture::new(mode, "private", "relative");
                let before = fixture.evidence();
                let diagnostic = failure(
                    &mut fault_command(&fixture, "events", quiet, exit_code),
                    "containment evidence incomplete",
                );
                if exit_code == 0 {
                    assert!(diagnostic.contains("Cargo reported success (exit 0)"));
                } else {
                    assert!(diagnostic.contains("Cargo exited with code 55"));
                    assert!(!diagnostic.contains("Cargo reported success"));
                }
                assert!(diagnostic.contains("PackageYanked event could not be confirmed"));
                assert!(diagnostic.contains("Remaining containment actions stopped"));
                assert!(diagnostic.contains("Reconcile registry truth"));
                assert!(diagnostic.contains("do not retry automatically"));
                assert_one_cargo(&fixture);
                for (path, bytes) in before {
                    let retained = if path == fixture.state.join("events.jsonl") {
                        fixture.state.join("events.before-conflict.jsonl")
                    } else {
                        path
                    };
                    assert_eq!(fs::read(retained).unwrap(), bytes);
                }
                assert!(fixture.state.join("events.jsonl").is_dir());
                assert!(!fixture.caller.join("state with spaces").exists());
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn requested_receipt_amendment_is_checked_before_cargo() {
    for fault in [
        "receipt-missing",
        "receipt-read",
        "receipt-malformed",
        "receipt-unmatched",
        "receipt-registry",
    ] {
        for quiet in [false, true] {
            let fixture = Fixture::new(Mode::Single, "private", "relative");
            change_evidence(&fixture.state, fault);
            let receipt_path = fixture.state.join("receipt.json");
            let receipt = path_state(&receipt_path);
            let prior_events = fs::read(fixture.state.join("events.jsonl")).unwrap();
            let diagnostic = failure(
                &mut fault_command(&fixture, "none", quiet, 0),
                "cannot prepare requested --mark-compromised receipt amendment",
            );
            assert!(diagnostic.contains("no Cargo yank was started"));
            if fault == "receipt-registry" {
                assert!(
                    diagnostic.contains("receipt registry does not match the selected registry")
                );
            }
            assert_no_cargo(&fixture);
            assert_eq!(path_state(&receipt_path), receipt);
            assert_eq!(
                fs::read(fixture.state.join("events.jsonl")).unwrap(),
                prior_events
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn requested_receipt_amendment_failure_after_yank_is_incomplete_evidence() {
    for fault in [
        "receipt-write",
        "receipt-missing",
        "receipt-read",
        "receipt-malformed",
        "receipt-unmatched",
        "receipt-registry",
    ] {
        for quiet in [false, true] {
            let fixture = Fixture::new(Mode::Single, "private", "relative");
            let original = fs::read(fixture.state.join("receipt.json")).unwrap();
            let diagnostic = failure(
                &mut fault_command(&fixture, fault, quiet, 0),
                "containment evidence incomplete",
            );
            assert!(diagnostic.contains("Cargo reported success (exit 0)"));
            assert!(diagnostic.contains("PackageYanked event was recorded"));
            assert!(diagnostic.contains("requested --mark-compromised receipt amendment failed"));
            assert!(diagnostic.contains("do not repeat the yank"));
            if fault == "receipt-registry" {
                assert!(
                    diagnostic.contains("receipt registry does not match the selected registry")
                );
            }
            assert_one_cargo(&fixture);
            let events = read_jsonl(&fixture.state.join("events.jsonl"));
            assert_eq!(events.len(), 2);
            assert_eq!(events[0], serde_json::json!({"prior": true}));
            assert_eq!(events[1]["event_type"]["type"], "package_yanked");
            assert_eq!(events[1]["event_type"]["exit_code"], 0);
            assert_eq!(events[1]["event_type"]["reason"], "context proof");
            let retained_receipt = fixture.state.join(match fault {
                "receipt-read" | "receipt-missing" => "receipt.before-conflict.json",
                _ => "receipt.json",
            });
            let expected = if matches!(
                fault,
                "receipt-unmatched" | "receipt-malformed" | "receipt-registry"
            ) {
                fs::read(fixture.state.join("receipt.after-conflict.json")).unwrap()
            } else {
                original
            };
            assert_eq!(fs::read(retained_receipt).unwrap(), expected);
            assert_eq!(
                fs::read(fixture.state.join("state.json")).unwrap(),
                b"retained state bytes\n"
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn successful_containment_preserves_events_markers_and_reason_contracts() {
    for mode in MODES {
        for quiet in [false, true] {
            let fixture = Fixture::new(mode, "private", "relative");
            fixture.assert_success(
                "private",
                fault_command(&fixture, "none", quiet, 0).args(["--format", "text"]),
            );
            let events = read_jsonl(&fixture.state.join("events.jsonl"));
            let expected = match mode {
                Mode::Single => "context proof",
                Mode::YankPlan => "reviewed containment",
                Mode::Remediate => "[OPERATOR_REASON_REDACTED]",
            };
            for event in &events[1..] {
                assert_eq!(event["event_type"]["reason"], expected);
                let serialized = serde_json::to_string(event).unwrap();
                assert!(!serialized.contains(SECRET));
                assert!(!serialized.contains(RESPONSE_BODY));
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn containment_rejects_json_before_discovery_or_effects() {
    for mode in MODES {
        for quiet in [false, true] {
            for missing_manifest in [false, true] {
                let fixture = Fixture::new(mode, "private", "relative");
                let before = fixture.evidence();
                if missing_manifest {
                    fs::rename(
                        fixture.workspace.join("Cargo.toml"),
                        fixture.workspace.join("Cargo.toml.off"),
                    )
                    .unwrap();
                }
                let output = fault_command(&fixture, "none", quiet, 0)
                    .args(["--format", "json"])
                    .output()
                    .expect("unsupported containment format");
                assert_eq!(output.status.code(), Some(2));
                assert!(output.stdout.is_empty());
                let diagnostic = String::from_utf8(output.stderr).unwrap();
                assert!(
                    diagnostic.contains("--format json is not supported for containment execution")
                );
                assert!(diagnostic.contains("use --format text"));
                assert!(!diagnostic.contains("cargo metadata"));
                assert_no_cargo(&fixture);
                assert_eq!(fixture.evidence(), before);
                assert!(!fixture.caller.join("state with spaces").exists());
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn remediation_dry_run_json_remains_machine_clean_without_yanks() {
    for quiet in [false, true] {
        let fixture = Fixture::new(Mode::Single, "private", "relative");
        let before = fixture.evidence();
        let mut command = shipper_cmd();
        command
            .timeout(Duration::from_secs(15))
            .current_dir(&fixture.caller)
            .arg("--manifest-path")
            .arg(fixture.workspace.join("Cargo.toml"))
            .arg("--state-dir")
            .arg(fixture.state_arg.as_ref().unwrap())
            .args(["--registry", "private", "--format", "json"])
            .env("CARGO_NET_OFFLINE", "true")
            .env("SHIPPER_CARGO_BIN", &fixture.cargo)
            .env("SHIPPER_FAKE_CARGO_LOG", &fixture.args_log)
            .env("SHIPPER_FAKE_CARGO_CWD_LOG", &fixture.cwd_log)
            .args(["remediate", "--dry-run", "--from-receipt"])
            .arg(fixture.state.join("receipt.json"))
            .args([
                "--crate",
                "core-lib",
                "--target-version",
                "0.2.0",
                "--reason",
            ])
            .arg(format!("Authorization: Bearer {SECRET}"));
        if quiet {
            command.arg("--quiet");
        }
        let output = command.output().expect("remediation dry-run JSON");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["schema_version"], "shipper.remediation_plan.v1");
        assert_eq!(plan["registry"], "private");
        assert_eq!(plan["target"]["reason"], "[OPERATOR_REASON_REDACTED]");
        assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET));
        let retained: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.state.join("remediation-plan.json")).unwrap())
                .unwrap();
        assert_eq!(plan, retained);
        assert_no_cargo(&fixture);
        assert_eq!(fixture.evidence(), before);
    }
}
