//! Isolated state-path observations through the real check, summary and renderers.
//! Other doctor checks are fixed passing fixtures; no registry or tool probe runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use serial_test::serial;
use shipper_core::types::{Registry, ReleasePlan};

use super::*;

fn workspace(root: &Path) -> plan::PlannedWorkspace {
    plan::PlannedWorkspace {
        workspace_root: root.to_path_buf(),
        plan: ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "doctor-state-path".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            registry: Registry::crates_io(),
            packages: vec![],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    }
}

fn options(state_dir: PathBuf) -> RuntimeOptions {
    let mut options = shipper_config::ShipperConfig::default()
        .build_runtime_options(shipper_config::CliOverrides::default());
    options.state_dir = state_dir;
    options
}

#[cfg(unix)]
struct RestorePermissions(PathBuf, fs::Permissions);

#[cfg(unix)]
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, self.1.clone());
    }
}

fn report_for_state(state_dir: checks::state_dir::StateDirCheck) -> DoctorReport {
    let auth = checks::auth::AuthCheck {
        auth_type: "fixture",
        findings: vec![],
    };
    let tools = ["cargo", "git"]
        .into_iter()
        .map(|command| checks::tools::ToolCheck {
            command,
            version: Some("fixture".to_string()),
            error: None,
        })
        .collect::<Vec<_>>();
    let connectivity = checks::connectivity::ConnectivityCheck {
        registry_reachable: true,
        registry_error: None,
        index_base: "fixture".to_string(),
        findings: vec![],
    };
    let git = checks::git::GitCheck {
        is_repository: true,
        commit: None,
        branch: None,
        dirty: Some(false),
        findings: vec![],
    };
    let encryption = checks::encryption::EncryptionCheck {
        enabled: false,
        key_source: None,
        env_var: None,
        key_present: None,
        findings: vec![],
    };
    let summary = summarize_report(
        &auth,
        &state_dir,
        &tools,
        &connectivity,
        &git,
        &encryption,
        None,
    );
    DoctorReport {
        workspace_root: "fixture".to_string(),
        registry: DoctorRegistryReport {
            name: "fixture".to_string(),
            api_base: "fixture".to_string(),
            index_base: None,
        },
        plan_unavailable: None,
        findings: state_dir.findings.clone(),
        auth,
        state_dir,
        tools,
        connectivity,
        git,
        encryption,
        summary,
    }
}

fn fixture(root: &Path) {
    for name in [
        "state.json",
        "events.jsonl",
        "receipt.json",
        "lock",
        ".shipper.toml",
        "config.toml",
        ".shipper-doctor-probe-existing",
    ] {
        fs::write(
            root.join(name),
            format!("retained fixture bytes for {name}\n"),
        )
        .unwrap();
    }
}

fn retained_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_path_buf(), vec![]);
                walk(root, &path, result);
            } else if metadata.file_type().is_symlink() {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn render_child(root: &Path, path: &Path, format: &str, drop_root: bool) -> String {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "doctor::state_dir_tests::state_dir_report_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(root)
        .env("SHIPPER_TEST_DOCTOR_STATE_DIR", path)
        .env("SHIPPER_TEST_DOCTOR_ROOT", root)
        .env("SHIPPER_TEST_DOCTOR_FORMAT", format);
    #[cfg(unix)]
    if drop_root {
        use std::os::unix::process::CommandExt;
        // Test subprocess only: production never changes identity.
        command.gid(65534).uid(65534);
    }
    #[cfg(not(unix))]
    assert!(!drop_root);
    let output = command
        .output()
        .expect("run state-directory renderer helper");
    assert!(
        output.status.success(),
        "helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_rendered_case(
    root: &Path,
    path: &Path,
    kind: &str,
    readiness: &str,
    finding_id: Option<&str>,
    drop_root: bool,
) -> Value {
    let text = render_child(root, path, "text", drop_root);
    let output = render_child(root, path, "json", drop_root);
    let json = output
        .split_once("DOCTOR_JSON_BEGIN\n")
        .unwrap()
        .1
        .split_once("\nDOCTOR_JSON_END")
        .unwrap()
        .0;
    let report: Value = serde_json::from_str(json).unwrap();
    let state = &report["reports"][0]["state_dir"];
    let summary = &report["summary"];

    // Classifications are the primary oracle, before asserting the additive
    // observation vocabulary. The old file-is-writable bug fails here.
    assert_eq!(summary["readiness"], readiness, "{report:#}");
    assert_eq!(report["reports"][0]["summary"]["readiness"], readiness);
    assert!(text.contains(&format!("Doctor: {readiness}")), "{text}");
    let (passed, blockers, unknown) = match readiness {
        "ready" => (8, 0, 0),
        "blocked" => (7, 1, 0),
        "incomplete" => (7, 0, 1),
        other => panic!("unexpected fixture expectation: {other}"),
    };
    assert_eq!(summary["checks_evaluated"], 8);
    assert_eq!(summary["checks_passed"], passed);
    assert_eq!(summary["blocker_count"], blockers);
    assert_eq!(summary["warning_count"], 0);
    assert_eq!(summary["skipped_or_unknown_count"], unknown);
    let blocker_label = if blockers == 1 {
        "1 blocker"
    } else {
        "0 blockers"
    };
    assert!(
        text.contains(&format!(
            "Checks: 8 evaluated, {passed} passed, 0 warnings, {blocker_label}, {unknown} unknown"
        )),
        "{text}"
    );
    match finding_id {
        Some(id) => {
            let status = if readiness == "blocked" {
                "blocked"
            } else {
                "unknown"
            };
            assert_eq!(state["findings"].as_array().unwrap().len(), 1);
            assert_eq!(state["findings"][0]["id"], id);
            assert_eq!(state["findings"][0]["status"], status);
            assert_eq!(report["reports"][0]["findings"], state["findings"]);
            assert!(
                text.contains(&format!("({id})")) && text.contains(&format!("status: {status}")),
                "{text}"
            );
            assert!(
                !state["findings"][0]["evidence"]
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
        }
        None => assert!(state["findings"].as_array().unwrap().is_empty()),
    }
    if unknown == 1 {
        assert_eq!(
            summary["unknown_checks"],
            serde_json::json!(["state directory"])
        );
        assert!(text.contains("Unknown: state directory"));
    }
    assert_eq!(state["observation"]["kind"], kind);
    if readiness != "ready" {
        assert_ne!(state.get("writable"), Some(&Value::Bool(true)));
        assert!(!text.contains("state_dir_writable: true"));
    }
    state.clone()
}

#[test]
#[serial]
fn state_dir_supported_directory_passes_and_preserves_all_evidence() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let before = retained_bytes(root.path());
    let state = assert_rendered_case(
        root.path(),
        root.path(),
        "writable_directory",
        "ready",
        None,
        false,
    );
    assert_eq!(state["exists"], true);
    assert_eq!(state["writable"], true);
    assert_eq!(retained_bytes(root.path()), before);
}

#[test]
#[serial]
fn state_dir_regular_file_is_blocked_in_both_renderers() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let path = root.path().join("state.json");
    let before = retained_bytes(root.path());
    let state = assert_rendered_case(
        root.path(),
        &path,
        "not_directory",
        "blocked",
        Some("state-dir-not-directory"),
        false,
    );
    assert_eq!(state["exists"], true);
    assert_eq!(state["observation"]["file_type"], "regular_file");
    assert_eq!(retained_bytes(root.path()), before);
}

#[test]
#[serial]
fn state_dir_parent_file_is_blocked_without_creating_descendants() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let path = root.path().join("state.json").join("nested").join("state");
    let before = retained_bytes(root.path());
    let state = assert_rendered_case(
        root.path(),
        &path,
        "parent_not_directory",
        "blocked",
        Some("state-dir-parent-not-directory"),
        false,
    );
    assert_eq!(state["exists"], false);
    assert_eq!(
        state["observation"]["parent"],
        root.path().join("state.json").display().to_string()
    );
    assert_eq!(retained_bytes(root.path()), before);
}

#[test]
#[serial]
fn state_dir_absent_path_is_unknown_and_remains_prospective() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let path = root.path().join("not-created").join("state");
    let before = retained_bytes(root.path());
    let state = assert_rendered_case(
        root.path(),
        &path,
        "missing",
        "incomplete",
        Some("state-dir-not-created"),
        false,
    );
    assert_eq!(state["exists"], false);
    assert!(state.get("writable").is_none());
    assert_eq!(retained_bytes(root.path()), before);
}

#[test]
#[serial]
fn state_dir_relative_path_uses_the_workspace_root() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("state")).unwrap();
    fixture(&root.path().join("state"));
    let before = retained_bytes(root.path());
    let ws = workspace(root.path());
    let opts = options(PathBuf::from("state"));
    let state = checks::state_dir::inspect(&ws, &opts);
    assert_eq!(state.path, root.path().join("state").display().to_string());
    assert_eq!(
        serde_json::to_value(report_for_state(state)).unwrap()["summary"]["readiness"],
        "ready"
    );
    assert_eq!(retained_bytes(root.path()), before);
}

#[cfg(unix)]
#[test]
#[serial]
fn state_dir_unresolved_symlink_is_blocked_and_preserved() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let path = root.path().join("dangling");
    symlink(root.path().join("absent-target"), &path).unwrap();
    let before = retained_bytes(root.path());
    assert_rendered_case(
        root.path(),
        &path,
        "unresolved_symlink",
        "blocked",
        Some("state-dir-unresolved-symlink"),
        false,
    );
    assert_eq!(retained_bytes(root.path()), before);
}

#[cfg(unix)]
#[test]
#[serial]
fn state_dir_permission_denial_uses_effective_identity_and_preserves_evidence() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fixture(root.path());
    let denied = root.path().join("denied");
    fs::create_dir(&denied).unwrap();
    fixture(&denied);
    let before = retained_bytes(root.path());
    let is_root = fs::metadata(root.path()).unwrap().uid() == 0;
    let restore = RestorePermissions(denied.clone(), fs::metadata(&denied).unwrap().permissions());
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o555)).unwrap();
    let state = assert_rendered_case(
        root.path(),
        &denied,
        "probe_failed",
        "blocked",
        Some("state-dir-write-probe-failed"),
        is_root,
    );
    assert_eq!(state["observation"]["operation"], "create");
    assert_eq!(state["observation"]["error_kind"], "PermissionDenied");
    assert_eq!(state["exists"], true);
    assert_eq!(state["writable"], false);
    if is_root {
        // Under root the same readonly bits do not deny effective writes.
        let opts = options(denied.clone());
        let state = checks::state_dir::inspect(&workspace(root.path()), &opts);
        assert_eq!(
            serde_json::to_value(report_for_state(state)).unwrap()["summary"]["readiness"],
            "ready"
        );
    }
    drop(restore);
    assert_eq!(retained_bytes(root.path()), before);
}

#[cfg(unix)]
#[test]
#[serial]
fn state_dir_metadata_denial_is_unknown_with_a_reason() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fixture(root.path());
    let hidden = root.path().join("hidden");
    fs::create_dir(&hidden).unwrap();
    let path = hidden.join("state");
    fs::create_dir(&path).unwrap();
    fixture(&path);
    let before = retained_bytes(root.path());
    let is_root = fs::metadata(root.path()).unwrap().uid() == 0;
    let restore = RestorePermissions(hidden.clone(), fs::metadata(&hidden).unwrap().permissions());
    fs::set_permissions(&hidden, fs::Permissions::from_mode(0o000)).unwrap();
    let state = assert_rendered_case(
        root.path(),
        &path,
        "inspection_failed",
        "incomplete",
        Some("state-dir-inspection-failed"),
        is_root,
    );
    drop(restore);
    assert_eq!(state["observation"]["error_kind"], "PermissionDenied");
    assert!(state.get("exists").is_none());
    assert!(state.get("writable").is_none());
    assert_eq!(retained_bytes(root.path()), before);
}

#[test]
fn shared_finding_status_preserves_existing_checks_and_prioritizes_unknowns() {
    use findings::FindingLevel::{Blocked, Unknown, Warning};
    use summary::DoctorCheckStatus;
    let finding = |status| findings::Finding {
        id: "fixture",
        severity: Warning,
        status,
        title: "fixture",
        why_it_matters: "fixture",
        evidence: "fixture".to_string(),
        try_next: vec![],
        docs: None,
    };
    for (statuses, expected) in [
        (vec![], DoctorCheckStatus::Passed),
        (vec![Warning], DoctorCheckStatus::Warning),
        (vec![Blocked], DoctorCheckStatus::Blocked),
        (vec![Warning, Blocked], DoctorCheckStatus::Blocked),
        (vec![Unknown], DoctorCheckStatus::Unknown),
        (vec![Warning, Unknown], DoctorCheckStatus::Unknown),
        (vec![Unknown, Blocked], DoctorCheckStatus::Blocked),
    ] {
        assert_eq!(
            summary::status_from_findings(&statuses.into_iter().map(finding).collect::<Vec<_>>()),
            expected
        );
    }
}

#[test]
#[ignore = "subprocess helper for actual state check and text/JSON renderer parity"]
#[serial]
fn state_dir_report_helper() {
    let path = PathBuf::from(
        std::env::var_os("SHIPPER_TEST_DOCTOR_STATE_DIR").expect("helper state path"),
    );
    let root = PathBuf::from(std::env::var_os("SHIPPER_TEST_DOCTOR_ROOT").expect("helper root"));
    let format = std::env::var("SHIPPER_TEST_DOCTOR_FORMAT").expect("helper format");
    let ws = workspace(&root);
    let opts = options(path);
    let state = if format == "text" {
        checks::state_dir::check(&ws, &opts)
    } else {
        checks::state_dir::inspect(&ws, &opts)
    };
    let report = report_for_state(state);
    if format == "json" {
        println!("\nDOCTOR_JSON_BEGIN");
        print_json(vec![report], &WorkspaceStatus::Planned).unwrap();
        println!("DOCTOR_JSON_END");
    } else {
        findings::print_findings(&report.findings);
        summary::DoctorSummary::combine(WorkspaceStatus::Planned.check_status(), [report.summary])
            .print_human();
    }
}
