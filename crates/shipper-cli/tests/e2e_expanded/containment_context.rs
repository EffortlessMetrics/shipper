//! Production CLI containment must preserve registry, cwd and evidence identity.

use super::*;
use std::time::Duration;

#[path = "containment_evidence.rs"]
mod containment_evidence;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Single,
    YankPlan,
    Remediate,
}

const MODES: [Mode; 3] = [Mode::Single, Mode::YankPlan, Mode::Remediate];

struct Fixture {
    _temp: tempfile::TempDir,
    mode: Mode,
    workspace: PathBuf,
    caller: PathBuf,
    state: PathBuf,
    state_arg: Option<PathBuf>,
    plan: Option<PathBuf>,
    cargo: PathBuf,
    args_log: PathBuf,
    cwd_log: PathBuf,
}

impl Fixture {
    fn new(mode: Mode, registry: &str, state_kind: &str) -> Self {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path().join("selected workspace");
        let caller = temp.path().join("unrelated caller");
        create_multi_crate_workspace(&workspace);
        fs::create_dir_all(&caller).expect("caller");
        // Real metadata stays offline; the mutation shim must still receive an
        // explicit target when Cargo has a different configured default.
        write_file(
            &workspace.join(".cargo/config.toml"),
            "[registry]\ndefault = \"other-default\"\n[registries.other-default]\nindex = \"sparse+https://other.example/index/\"\n",
        );
        let state_arg = match state_kind {
            "default" => None,
            "relative" => Some(PathBuf::from("state with spaces")),
            "absolute" => Some(temp.path().join("absolute evidence")),
            _ => panic!("unknown state kind"),
        };
        let state = match &state_arg {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => workspace.join(path),
            None => workspace.join(".shipper"),
        };
        fs::create_dir_all(&state).expect("state");
        write_file(&state.join("state.json"), "retained state bytes\n");
        write_file(&state.join("events.jsonl"), "{\"prior\":true}\n");
        let receipt_path = state.join("receipt.json");
        write_remediation_receipt(&receipt_path);
        rewrite_registry(&receipt_path, registry, true);
        let plan = match mode {
            Mode::Single => None,
            Mode::YankPlan => {
                let path = write_yank_plan_artifact(&workspace);
                rewrite_registry(&path, registry, false);
                Some(path)
            }
            Mode::Remediate => Some(write_remediation_dry_run_artifact(
                &workspace,
                &state,
                &receipt_path,
            )),
        };
        let cargo = write_fake_yank_cargo(temp.path(), false);
        let args_log = temp.path().join("cargo args.log");
        let cwd_log = temp.path().join("cargo cwd.log");
        Self {
            _temp: temp,
            mode,
            workspace,
            caller,
            state,
            state_arg,
            plan,
            cargo,
            args_log,
            cwd_log,
        }
    }

    fn legacy_registry(&self, name: &str) {
        write_file(
            &self.workspace.join(".shipper.toml"),
            &format!(
                "[registry]\nname = \"{name}\"\napi_base = \"https://registry.example\"\nindex_base = \"https://registry.example/index\"\n"
            ),
        );
    }

    fn command(&self) -> Command {
        let mut command = shipper_cmd();
        command
            .timeout(Duration::from_secs(15))
            .current_dir(&self.caller)
            .arg("--manifest-path")
            .arg(self.workspace.join("Cargo.toml"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_REGISTRY_DEFAULT", "other-default")
            .env("SHIPPER_CARGO_BIN", &self.cargo)
            .env("SHIPPER_FAKE_CARGO_LOG", &self.args_log)
            .env("SHIPPER_FAKE_CARGO_CWD_LOG", &self.cwd_log);
        if let Some(path) = &self.state_arg {
            command.arg("--state-dir").arg(path);
        }
        match self.mode {
            Mode::Single => {
                command.args([
                    "yank",
                    "--crate",
                    "core-lib",
                    "--version",
                    "0.2.0",
                    "--reason",
                    "context proof",
                    "--mark-compromised",
                ]);
            }
            Mode::YankPlan => {
                command
                    .args(["yank", "--plan"])
                    .arg(self.plan.as_ref().unwrap());
            }
            Mode::Remediate => {
                command
                    .args(["remediate", "--execute-plan"])
                    .arg(self.plan.as_ref().unwrap());
            }
        }
        command
    }

    fn evidence(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut paths = vec![
            self.state.join("state.json"),
            self.state.join("events.jsonl"),
            self.state.join("receipt.json"),
        ];
        if let Some(plan) = &self.plan {
            paths.push(plan.clone());
        }
        paths
            .into_iter()
            .map(|path| {
                let bytes = fs::read(&path).expect("evidence bytes");
                (path, bytes)
            })
            .collect()
    }

    fn assert_success(&self, registry: &str, command: &mut Command) {
        let output = command.output().expect("run containment");
        assert!(
            output.status.success(),
            "{:?}: {}",
            self.mode,
            String::from_utf8_lossy(&output.stderr)
        );
        let packages = match self.mode {
            Mode::Single => vec![("core-lib", "0.2.0")],
            Mode::YankPlan | Mode::Remediate => vec![("top-app", "0.4.0"), ("core-lib", "0.2.0")],
        };
        let expected = packages
            .iter()
            .map(|(name, version)| format!("yank {name} --version={version} --registry {registry}"))
            .collect::<Vec<_>>();
        let actual = fs::read_to_string(&self.args_log).expect("Cargo args");
        assert_eq!(
            actual.lines().collect::<Vec<_>>(),
            expected,
            "{:?}",
            self.mode
        );
        let cwd = fs::read_to_string(&self.cwd_log).expect("Cargo cwd");
        assert_eq!(cwd.lines().count(), packages.len());
        for directory in cwd.lines() {
            assert_eq!(
                fs::canonicalize(directory).unwrap(),
                fs::canonicalize(&self.workspace).unwrap()
            );
        }
        let events = read_jsonl(&self.state.join("events.jsonl"));
        assert_eq!(events.len(), packages.len() + 1);
        assert_eq!(events[0], serde_json::json!({"prior": true}));
        for (event, (name, version)) in events[1..].iter().zip(packages) {
            assert_eq!(event["event_type"]["type"], "package_yanked");
            assert_eq!(event["event_type"]["crate_name"], name);
            assert_eq!(event["event_type"]["version"], version);
            assert_eq!(event["event_type"]["exit_code"], 0);
        }
        if matches!(self.mode, Mode::Single) {
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(self.state.join("receipt.json")).unwrap())
                    .unwrap();
            assert_eq!(receipt["packages"][0]["compromised_by"], "context proof");
        }
        assert_eq!(
            fs::read(self.state.join("state.json")).unwrap(),
            b"retained state bytes\n"
        );
        assert!(!self.caller.join(".shipper").exists());
        assert!(!self.caller.join("state with spaces").exists());
    }

    fn assert_refused(&self, command: &mut Command, diagnostic: &str) {
        let before = self.evidence();
        let output = command.output().expect("run refusal");
        assert!(!output.status.success(), "{:?}", self.mode);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!self.args_log.exists(), "no Cargo mutation may run");
        assert!(!self.cwd_log.exists());
        assert_eq!(
            self.evidence(),
            before,
            "refusal must preserve evidence bytes"
        );
        assert!(!self.caller.join(".shipper").exists());
        assert!(!self.caller.join("state with spaces").exists());
    }
}

fn rewrite_registry(path: &Path, registry: &str, receipt: bool) {
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    if receipt {
        value["registry"]["name"] = registry.into();
    } else {
        value["registry"] = registry.into();
    }
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
#[serial_test::serial]
fn containment_uses_cli_legacy_and_override_registry() {
    for mode in MODES {
        for selection in [
            "cli-private",
            "legacy-private",
            "override-crates-io",
            "default-crates-io",
        ] {
            let registry = if selection.ends_with("crates-io") {
                "crates-io"
            } else {
                "private"
            };
            let fixture = Fixture::new(mode, registry, "default");
            if matches!(selection, "legacy-private" | "override-crates-io") {
                fixture.legacy_registry("private");
            }
            let mut command = fixture.command();
            if matches!(selection, "cli-private" | "override-crates-io") {
                command.args(["--registry", registry]);
            }
            fixture.assert_success(registry, &mut command);
        }
    }
}

#[test]
#[serial_test::serial]
fn containment_resolves_workspace_and_all_state_path_forms() {
    for mode in MODES {
        for state_kind in ["default", "relative", "absolute"] {
            let fixture = Fixture::new(mode, "private", state_kind);
            fixture.assert_success("private", fixture.command().args(["--registry", "private"]));
        }
    }
}

#[test]
#[serial_test::serial]
fn containment_rejects_plan_registry_conflicts_without_effects() {
    for mode in [Mode::YankPlan, Mode::Remediate] {
        for override_name in [None, Some("other")] {
            let fixture = Fixture::new(mode, "private", "relative");
            let mut command = fixture.command();
            if let Some(name) = override_name {
                command.args(["--registry", name]);
            }
            fixture.assert_refused(&mut command, "does not match configured registry");
        }
    }
}

#[test]
#[serial_test::serial]
fn containment_rejects_multi_registry_selectors_without_effects() {
    for mode in MODES {
        for selection in [
            vec!["--registries", "private"],
            vec!["--registries", "private,other"],
            vec!["--all-registries"],
        ] {
            let fixture = Fixture::new(mode, "private", "relative");
            fixture.assert_refused(fixture.command().args(selection), "support one registry");
        }
    }
}

fn receipt_planning_command(fixture: &Fixture, name: &str) -> Command {
    let mut command = shipper_cmd();
    command
        .timeout(Duration::from_secs(15))
        .current_dir(&fixture.caller)
        .arg("--manifest-path")
        .arg(fixture.workspace.join("Cargo.toml"))
        .args(["--format", "json"])
        .env("CARGO_NET_OFFLINE", "true")
        .env("SHIPPER_CARGO_BIN", &fixture.cargo)
        .env("SHIPPER_FAKE_CARGO_LOG", &fixture.args_log);
    if let Some(path) = &fixture.state_arg {
        command.arg("--state-dir").arg(path);
    }
    command.arg(name);
    command
}

fn receipt_with_plan_id(path: &Path, plan_id: &str) {
    write_remediation_receipt(path);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["plan_id"] = plan_id.into();
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
#[serial_test::serial]
fn receipt_planning_defaults_use_workspace_state_paths() {
    for name in ["plan-yank", "fix-forward"] {
        for state_kind in ["default", "relative", "absolute"] {
            let fixture = Fixture::new(Mode::Single, "crates-io", state_kind);
            let decoys = [
                fixture.caller.join(".shipper/receipt.json"),
                fixture.caller.join("state with spaces/receipt.json"),
            ];
            for path in &decoys {
                receipt_with_plan_id(path, "wrong-caller-receipt");
            }
            let before = fixture.evidence();
            let decoy_bytes = decoys.map(|path| fs::read(path).unwrap());
            let output = receipt_planning_command(&fixture, name)
                .output()
                .expect("run receipt planning");
            assert!(
                output.status.success(),
                "{name}/{state_kind}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(plan["command"], name);
            assert_eq!(
                plan["plan_id"], "plan-remediate-json",
                "{name}/{state_kind}"
            );
            assert_eq!(fixture.evidence(), before);
            assert_eq!(
                [".shipper/receipt.json", "state with spaces/receipt.json"]
                    .map(|path| fs::read(fixture.caller.join(path)).unwrap()),
                decoy_bytes
            );
            assert!(!fixture.args_log.exists(), "planning cannot invoke a yank");
        }
    }
}

#[test]
#[serial_test::serial]
fn receipt_planning_preserves_explicit_caller_relative_and_absolute_paths() {
    for name in ["plan-yank", "fix-forward"] {
        for absolute in [false, true] {
            let fixture = Fixture::new(Mode::Single, "crates-io", "relative");
            let relative = PathBuf::from("explicit receipt.json");
            let path = fixture.caller.join(&relative);
            receipt_with_plan_id(&path, "explicit-selected-receipt");
            let before = fixture.evidence();
            let explicit_bytes = fs::read(&path).unwrap();
            let output = receipt_planning_command(&fixture, name)
                .arg("--from-receipt")
                .arg(if absolute { &path } else { &relative })
                .output()
                .expect("run explicit receipt planning");
            assert!(
                output.status.success(),
                "{name}/absolute={absolute}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(plan["command"], name);
            assert_eq!(plan["plan_id"], "explicit-selected-receipt");
            assert_eq!(fixture.evidence(), before);
            assert_eq!(fs::read(path).unwrap(), explicit_bytes);
            assert!(!fixture.args_log.exists(), "planning cannot invoke a yank");
        }
    }
}
