use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use tempfile::tempdir;

use shipper_state::{
    CURRENT_RECEIPT_VERSION, CURRENT_STATE_VERSION, RECEIPT_FILE, STATE_FILE, clear_state,
    has_incomplete_state, load_receipt, load_state, receipt_path, save_state, state_path,
    write_receipt,
};
use shipper_types::{
    EnvironmentFingerprint, ErrorClass, ExecutionState, PackageEvidence, PackageProgress,
    PackageReceipt, PackageState, Receipt, Registry,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_registry() -> Registry {
    Registry::crates_io()
}

fn make_progress(name: &str, version: &str, state: PackageState) -> PackageProgress {
    PackageProgress {
        name: name.to_string(),
        version: version.to_string(),
        attempts: 1,
        state,
        last_updated_at: Utc::now(),
    }
}

fn make_state(plan_id: &str, packages: BTreeMap<String, PackageProgress>) -> ExecutionState {
    ExecutionState {
        state_version: CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: sample_registry(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

fn make_receipt(plan_id: &str, packages: Vec<PackageReceipt>) -> Receipt {
    Receipt {
        receipt_version: CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: sample_registry(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages,
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.1.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    }
}

fn make_package_receipt(name: &str, version: &str, state: PackageState) -> PackageReceipt {
    PackageReceipt {
        name: name.to_string(),
        version: version.to_string(),
        attempts: 1,
        state,
        started_at: Utc::now(),
        finished_at: Utc::now(),
        duration_ms: 42,
        evidence: PackageEvidence {
            attempts: vec![],
            readiness_checks: vec![],
        },
    }
}

// ---------------------------------------------------------------------------
// State persistence round-trip
// ---------------------------------------------------------------------------

#[test]
fn state_save_and_reload_preserves_all_fields() {
    let td = tempdir().unwrap();
    let dir = td.path().join("s");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "alpha@1.0.0".to_string(),
        make_progress("alpha", "1.0.0", PackageState::Pending),
    );
    pkgs.insert(
        "beta@2.0.0".to_string(),
        make_progress("beta", "2.0.0", PackageState::Published),
    );

    let state = make_state("plan-abc", pkgs);
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().expect("state must exist");
    assert_eq!(loaded.plan_id, "plan-abc");
    assert_eq!(loaded.state_version, CURRENT_STATE_VERSION);
    assert_eq!(loaded.registry.name, "crates-io");
    assert_eq!(loaded.packages.len(), 2);
    assert!(loaded.packages.contains_key("alpha@1.0.0"));
    assert!(loaded.packages.contains_key("beta@2.0.0"));
}

#[test]
fn state_reload_after_overwrite_returns_latest() {
    let td = tempdir().unwrap();
    let dir = td.path().join("s");

    let mut pkgs1 = BTreeMap::new();
    pkgs1.insert(
        "a@1.0.0".to_string(),
        make_progress("a", "1.0.0", PackageState::Pending),
    );
    save_state(&dir, &make_state("plan-1", pkgs1)).unwrap();

    let mut pkgs2 = BTreeMap::new();
    pkgs2.insert(
        "a@1.0.0".to_string(),
        make_progress("a", "1.0.0", PackageState::Published),
    );
    save_state(&dir, &make_state("plan-2", pkgs2)).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.plan_id, "plan-2");
    match &loaded.packages["a@1.0.0"].state {
        PackageState::Published => {}
        other => panic!("expected Published, got {other:?}"),
    }
}

#[test]
fn load_state_returns_none_for_empty_directory() {
    let td = tempdir().unwrap();
    assert!(load_state(td.path()).unwrap().is_none());
}

#[test]
fn load_state_returns_none_for_nonexistent_directory() {
    let td = tempdir().unwrap();
    let missing = td.path().join("does-not-exist");
    assert!(load_state(&missing).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Atomic write guarantees
// ---------------------------------------------------------------------------

#[test]
fn atomic_write_leaves_no_tmp_file_on_success() {
    let td = tempdir().unwrap();
    let dir = td.path().join("s");

    let state = make_state("p", BTreeMap::new());
    save_state(&dir, &state).unwrap();

    let tmp = state_path(&dir).with_extension("tmp");
    assert!(!tmp.exists(), "tmp file should be cleaned up after rename");
    assert!(state_path(&dir).exists(), "final state file must exist");
}

#[test]
fn atomic_write_produces_valid_json() {
    let td = tempdir().unwrap();
    let dir = td.path().join("s");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "x@0.1.0".to_string(),
        make_progress("x", "0.1.0", PackageState::Pending),
    );
    let state = make_state("p", pkgs);
    save_state(&dir, &state).unwrap();

    let raw = fs::read_to_string(state_path(&dir)).unwrap();
    let _: serde_json::Value = serde_json::from_str(&raw).expect("output must be valid JSON");
}

#[test]
fn atomic_write_receipt_produces_valid_json() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");

    let receipt = make_receipt("p1", vec![]);
    write_receipt(&dir, &receipt).unwrap();

    let raw = fs::read_to_string(receipt_path(&dir)).unwrap();
    let _: serde_json::Value = serde_json::from_str(&raw).expect("receipt must be valid JSON");
}

// ---------------------------------------------------------------------------
// All PackageState variants round-trip through state persistence
// ---------------------------------------------------------------------------

#[test]
fn all_package_state_variants_roundtrip() {
    let td = tempdir().unwrap();
    let dir = td.path().join("s");

    let variants: Vec<(&str, PackageState)> = vec![
        ("pending@1.0.0", PackageState::Pending),
        ("uploaded@1.0.0", PackageState::Uploaded),
        ("published@1.0.0", PackageState::Published),
        (
            "skipped@1.0.0",
            PackageState::Skipped {
                reason: "already published".to_string(),
            },
        ),
        (
            "failed-retryable@1.0.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "timeout".to_string(),
            },
        ),
        (
            "failed-permanent@1.0.0",
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "denied".to_string(),
            },
        ),
        (
            "failed-ambiguous@1.0.0",
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "unclear".to_string(),
            },
        ),
        (
            "ambiguous@1.0.0",
            PackageState::Ambiguous {
                message: "registry timeout".to_string(),
            },
        ),
    ];

    let mut pkgs = BTreeMap::new();
    for (key, state) in &variants {
        let name = key.split('@').next().unwrap();
        pkgs.insert(key.to_string(), make_progress(name, "1.0.0", state.clone()));
    }

    save_state(&dir, &make_state("all-variants", pkgs)).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), variants.len());

    // Spot-check a few variants
    match &loaded.packages["pending@1.0.0"].state {
        PackageState::Pending => {}
        other => panic!("expected Pending, got {other:?}"),
    }
    match &loaded.packages["skipped@1.0.0"].state {
        PackageState::Skipped { reason } => assert_eq!(reason, "already published"),
        other => panic!("expected Skipped, got {other:?}"),
    }
    match &loaded.packages["failed-retryable@1.0.0"].state {
        PackageState::Failed { class, message } => {
            assert!(matches!(class, ErrorClass::Retryable));
            assert_eq!(message, "timeout");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    match &loaded.packages["ambiguous@1.0.0"].state {
        PackageState::Ambiguous { message } => assert_eq!(message, "registry timeout"),
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Corruption recovery (invalid JSON)
// ---------------------------------------------------------------------------

#[test]
fn load_state_errors_on_corrupt_json() {
    let td = tempdir().unwrap();
    fs::write(state_path(td.path()), "{{{{not json!}").unwrap();

    let err = load_state(td.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse state JSON"),
        "unexpected error: {msg}"
    );
}

#[test]
fn load_state_errors_on_empty_file() {
    let td = tempdir().unwrap();
    fs::write(state_path(td.path()), "").unwrap();

    let err = load_state(td.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse state JSON"),
        "unexpected error: {msg}"
    );
}

#[test]
fn load_state_errors_on_valid_json_wrong_schema() {
    let td = tempdir().unwrap();
    // Valid JSON but not an ExecutionState
    fs::write(state_path(td.path()), r#"{"hello": "world"}"#).unwrap();

    let err = load_state(td.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse state JSON"),
        "unexpected error: {msg}"
    );
}

#[test]
fn load_receipt_errors_on_corrupt_json() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");
    fs::create_dir_all(&dir).unwrap();
    fs::write(receipt_path(&dir), "not json at all").unwrap();

    let err = load_receipt(&dir).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse receipt JSON"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// State migration / compatibility
// ---------------------------------------------------------------------------

#[test]
fn receipt_v1_migrated_to_v2_on_load() {
    let td = tempdir().unwrap();
    let dir = td.path().join("m");
    fs::create_dir_all(&dir).unwrap();

    let v1 = serde_json::json!({
        "receipt_version": "shipper.receipt.v1",
        "plan_id": "migration-test",
        "registry": {
            "name": "crates-io",
            "api_base": "https://crates.io",
            "index_base": "https://index.crates.io"
        },
        "started_at": "2024-06-01T00:00:00Z",
        "finished_at": "2024-06-01T00:05:00Z",
        "packages": [],
        "event_log_path": ".shipper/events.jsonl"
    });

    fs::write(
        receipt_path(&dir),
        serde_json::to_string_pretty(&v1).unwrap(),
    )
    .unwrap();

    let receipt = load_receipt(&dir).unwrap().expect("receipt must exist");
    assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
    assert!(receipt.git_context.is_none());
    assert!(!receipt.environment.shipper_version.is_empty());
}

#[test]
fn receipt_v2_loaded_as_is() {
    let td = tempdir().unwrap();
    let dir = td.path().join("m");

    let receipt = make_receipt("p-compat", vec![]);
    write_receipt(&dir, &receipt).unwrap();

    let loaded = load_receipt(&dir).unwrap().unwrap();
    assert_eq!(loaded.receipt_version, CURRENT_RECEIPT_VERSION);
    assert_eq!(loaded.plan_id, "p-compat");
}

#[test]
fn receipt_version_too_old_rejected() {
    let result = shipper_state::validate_receipt_version("shipper.receipt.v0");
    assert!(result.is_err());
    let msg = format!("{:#}", result.unwrap_err());
    assert!(msg.contains("too old"), "unexpected error: {msg}");
}

#[test]
fn receipt_version_invalid_format_rejected() {
    let result = shipper_state::validate_receipt_version("garbage");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Concurrent-style access patterns (sequential writes simulating contention)
// ---------------------------------------------------------------------------

#[test]
fn rapid_sequential_writes_keep_file_consistent() {
    let td = tempdir().unwrap();
    let dir = td.path().join("concurrent");

    for i in 0..20 {
        let mut pkgs = BTreeMap::new();
        pkgs.insert(
            format!("pkg@0.{i}.0"),
            make_progress("pkg", &format!("0.{i}.0"), PackageState::Pending),
        );
        let state = make_state(&format!("plan-{i}"), pkgs);
        save_state(&dir, &state).unwrap();

        // Immediately verify the file is readable
        let loaded = load_state(&dir).unwrap().unwrap();
        assert_eq!(loaded.plan_id, format!("plan-{i}"));
    }
}

#[test]
fn interleaved_state_and_receipt_writes() {
    let td = tempdir().unwrap();
    let dir = td.path().join("interleave");

    for i in 0..10 {
        let plan_id = format!("plan-{i}");

        let state = make_state(&plan_id, BTreeMap::new());
        save_state(&dir, &state).unwrap();
        assert!(has_incomplete_state(&dir));

        let receipt = make_receipt(&plan_id, vec![]);
        write_receipt(&dir, &receipt).unwrap();
        assert!(!has_incomplete_state(&dir));

        clear_state(&dir).unwrap();
        assert!(!state_path(&dir).exists());
        assert!(receipt_path(&dir).exists());

        // Clean up receipt for next iteration
        fs::remove_file(receipt_path(&dir)).unwrap();
    }
}

// ---------------------------------------------------------------------------
// plan_id validation on resume
// ---------------------------------------------------------------------------

#[test]
fn plan_id_mismatch_detectable_on_reload() {
    let td = tempdir().unwrap();
    let dir = td.path().join("resume");

    let state = make_state("original-plan", BTreeMap::new());
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    let expected_plan_id = "new-plan";

    // Simulate what a resume caller would do: check plan_id before proceeding
    assert_ne!(
        loaded.plan_id, expected_plan_id,
        "plan_id should differ, triggering re-plan"
    );
}

#[test]
fn plan_id_match_allows_resume() {
    let td = tempdir().unwrap();
    let dir = td.path().join("resume");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "crate-a@0.1.0".to_string(),
        make_progress("crate-a", "0.1.0", PackageState::Published),
    );
    pkgs.insert(
        "crate-b@0.2.0".to_string(),
        make_progress("crate-b", "0.2.0", PackageState::Pending),
    );

    let state = make_state("resume-plan", pkgs);
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.plan_id, "resume-plan");

    // A resume caller checks that the persisted plan_id matches
    let pending: Vec<_> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Pending))
        .collect();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "crate-b");
}

// ---------------------------------------------------------------------------
// has_incomplete_state and clear_state
// ---------------------------------------------------------------------------

#[test]
fn has_incomplete_state_lifecycle() {
    let td = tempdir().unwrap();
    let dir = td.path().join("lifecycle");
    fs::create_dir_all(&dir).unwrap();

    // Empty dir — no incomplete state
    assert!(!has_incomplete_state(&dir));

    // State only — incomplete
    save_state(&dir, &make_state("lc", BTreeMap::new())).unwrap();
    assert!(has_incomplete_state(&dir));

    // Add receipt — no longer incomplete
    write_receipt(&dir, &make_receipt("lc", vec![])).unwrap();
    assert!(!has_incomplete_state(&dir));

    // Clear state — receipt still present, not incomplete
    clear_state(&dir).unwrap();
    assert!(!has_incomplete_state(&dir));
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

#[test]
fn path_helpers_use_expected_filenames() {
    let base = PathBuf::from("some_dir");
    assert_eq!(state_path(&base).file_name().unwrap(), STATE_FILE);
    assert_eq!(receipt_path(&base).file_name().unwrap(), RECEIPT_FILE);
}

// ---------------------------------------------------------------------------
// Large state round-trip (many packages)
// ---------------------------------------------------------------------------

#[test]
fn large_state_roundtrip() {
    let td = tempdir().unwrap();
    let dir = td.path().join("large");

    let mut pkgs = BTreeMap::new();
    for i in 0..100 {
        let key = format!("crate-{i}@1.0.0");
        let state = if i % 3 == 0 {
            PackageState::Published
        } else if i % 3 == 1 {
            PackageState::Pending
        } else {
            PackageState::Skipped {
                reason: format!("reason {i}"),
            }
        };
        pkgs.insert(key, make_progress(&format!("crate-{i}"), "1.0.0", state));
    }

    let state = make_state("big-plan", pkgs);
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), 100);
    assert_eq!(loaded.plan_id, "big-plan");
}

// ---------------------------------------------------------------------------
// Receipt round-trip with all PackageState variants
// ---------------------------------------------------------------------------

#[test]
fn receipt_all_package_state_variants_roundtrip() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");

    let packages = vec![
        make_package_receipt("a", "1.0.0", PackageState::Published),
        make_package_receipt(
            "b",
            "1.0.0",
            PackageState::Skipped {
                reason: "exists".to_string(),
            },
        ),
        make_package_receipt(
            "c",
            "1.0.0",
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "denied".to_string(),
            },
        ),
        make_package_receipt(
            "d",
            "1.0.0",
            PackageState::Ambiguous {
                message: "unknown".to_string(),
            },
        ),
    ];

    let receipt = make_receipt("receipt-plan", packages);
    write_receipt(&dir, &receipt).unwrap();

    let loaded = load_receipt(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), 4);

    match &loaded.packages[2].state {
        PackageState::Failed { class, message } => {
            assert!(matches!(class, ErrorClass::Permanent));
            assert_eq!(message, "denied");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// save_state creates nested directories
// ---------------------------------------------------------------------------

#[test]
fn save_state_creates_deep_nested_directories() {
    let td = tempdir().unwrap();
    let dir = td.path().join("a").join("b").join("c").join("d");

    save_state(&dir, &make_state("deep", BTreeMap::new())).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.plan_id, "deep");
}

// ---------------------------------------------------------------------------
// Corrupt state recovery: partial/truncated write
// ---------------------------------------------------------------------------

#[test]
fn load_state_errors_on_truncated_json() {
    let td = tempdir().unwrap();
    let truncated = r#"{"state_version":"shipper.state.v1","plan_id":"tr"#;
    fs::write(state_path(td.path()), truncated).unwrap();

    let err = load_state(td.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse state JSON"),
        "unexpected error: {msg}"
    );
}

#[test]
fn load_receipt_errors_on_truncated_json() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");
    fs::create_dir_all(&dir).unwrap();
    let truncated = r#"{"receipt_version":"shipper.receipt.v2","plan_id":"#;
    fs::write(receipt_path(&dir), truncated).unwrap();

    let err = load_receipt(&dir).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to parse receipt JSON"),
        "unexpected error: {msg}"
    );
}

// ---------------------------------------------------------------------------
// State transitions: Pending → Failed → Pending (retry cycle)
// ---------------------------------------------------------------------------

#[test]
fn state_transition_pending_failed_pending_retry_cycle() {
    let td = tempdir().unwrap();
    let dir = td.path().join("retry");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "retry-crate@1.0.0".to_string(),
        make_progress("retry-crate", "1.0.0", PackageState::Pending),
    );
    let mut state = make_state("retry-plan", pkgs);
    save_state(&dir, &state).unwrap();

    // Transition to Failed
    let pkg = state.packages.get_mut("retry-crate@1.0.0").unwrap();
    pkg.state = PackageState::Failed {
        class: ErrorClass::Retryable,
        message: "network timeout".to_string(),
    };
    pkg.attempts = 1;
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert!(matches!(
        loaded.packages["retry-crate@1.0.0"].state,
        PackageState::Failed { .. }
    ));

    // Transition back to Pending (retry)
    state.packages.get_mut("retry-crate@1.0.0").unwrap().state = PackageState::Pending;
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert!(matches!(
        loaded.packages["retry-crate@1.0.0"].state,
        PackageState::Pending
    ));
    assert_eq!(loaded.packages["retry-crate@1.0.0"].attempts, 1);
}

// ---------------------------------------------------------------------------
// State transitions: Published is idempotent
// ---------------------------------------------------------------------------

#[test]
fn state_transition_published_idempotent() {
    let td = tempdir().unwrap();
    let dir = td.path().join("idempotent");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "stable@1.0.0".to_string(),
        make_progress("stable", "1.0.0", PackageState::Published),
    );
    let state = make_state("idem-plan", pkgs);

    save_state(&dir, &state).unwrap();
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert!(matches!(
        loaded.packages["stable@1.0.0"].state,
        PackageState::Published
    ));
}

// ---------------------------------------------------------------------------
// Receipt generation: all packages published
// ---------------------------------------------------------------------------

#[test]
fn receipt_all_packages_published() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");

    let packages = vec![
        make_package_receipt("core", "1.0.0", PackageState::Published),
        make_package_receipt("utils", "0.5.0", PackageState::Published),
        make_package_receipt("cli", "2.0.0", PackageState::Published),
    ];

    let receipt = make_receipt("all-published", packages);
    write_receipt(&dir, &receipt).unwrap();

    let loaded = load_receipt(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), 3);
    assert!(
        loaded
            .packages
            .iter()
            .all(|p| matches!(p.state, PackageState::Published))
    );
}

// ---------------------------------------------------------------------------
// Receipt generation: some packages failed
// ---------------------------------------------------------------------------

#[test]
fn receipt_some_packages_failed() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");

    let packages = vec![
        make_package_receipt("core", "1.0.0", PackageState::Published),
        make_package_receipt(
            "utils",
            "0.5.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "registry timeout".to_string(),
            },
        ),
        make_package_receipt(
            "cli",
            "2.0.0",
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "crate name reserved".to_string(),
            },
        ),
    ];

    let receipt = make_receipt("some-failed", packages);
    write_receipt(&dir, &receipt).unwrap();

    let loaded = load_receipt(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), 3);
    assert!(matches!(loaded.packages[0].state, PackageState::Published));
    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Failed { .. }
    ));
    assert!(matches!(
        loaded.packages[2].state,
        PackageState::Failed { .. }
    ));
}

// ---------------------------------------------------------------------------
// Receipt generation: mixed states (published, failed, skipped, ambiguous)
// ---------------------------------------------------------------------------

#[test]
fn receipt_mixed_states_all_outcomes() {
    let td = tempdir().unwrap();
    let dir = td.path().join("r");

    let packages = vec![
        make_package_receipt("core", "1.0.0", PackageState::Published),
        make_package_receipt(
            "utils",
            "0.5.0",
            PackageState::Skipped {
                reason: "already published".to_string(),
            },
        ),
        make_package_receipt(
            "cli",
            "2.0.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "network error".to_string(),
            },
        ),
        make_package_receipt(
            "extras",
            "0.1.0",
            PackageState::Ambiguous {
                message: "upload timeout".to_string(),
            },
        ),
    ];

    let receipt = make_receipt("mixed-states", packages);
    write_receipt(&dir, &receipt).unwrap();

    let loaded = load_receipt(&dir).unwrap().unwrap();
    assert_eq!(loaded.packages.len(), 4);
    assert!(matches!(loaded.packages[0].state, PackageState::Published));
    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Skipped { .. }
    ));
    assert!(matches!(
        loaded.packages[2].state,
        PackageState::Failed { .. }
    ));
    assert!(matches!(
        loaded.packages[3].state,
        PackageState::Ambiguous { .. }
    ));
}

// ---------------------------------------------------------------------------
// Edge case: very long package names
// ---------------------------------------------------------------------------

#[test]
fn state_with_very_long_package_name() {
    let td = tempdir().unwrap();
    let dir = td.path().join("long");

    let long_name = "a".repeat(500);
    let key = format!("{long_name}@1.0.0");
    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        key.clone(),
        make_progress(&long_name, "1.0.0", PackageState::Pending),
    );

    let state = make_state("long-plan", pkgs);
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert!(loaded.packages.contains_key(&key));
    assert_eq!(loaded.packages[&key].name, long_name);
}

// ---------------------------------------------------------------------------
// Edge case: empty plan_id
// ---------------------------------------------------------------------------

#[test]
fn state_with_empty_plan_id() {
    let td = tempdir().unwrap();
    let dir = td.path().join("empty-plan");

    let state = make_state("", BTreeMap::new());
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.plan_id, "");
}

// ---------------------------------------------------------------------------
// Edge case: unicode in directory paths
// ---------------------------------------------------------------------------

#[test]
fn state_save_load_unicode_directory() {
    let td = tempdir().unwrap();
    let dir = td.path().join("données").join("日本語");

    let state = make_state("unicode-plan", BTreeMap::new());
    save_state(&dir, &state).unwrap();

    let loaded = load_state(&dir).unwrap().unwrap();
    assert_eq!(loaded.plan_id, "unicode-plan");
}

// ---------------------------------------------------------------------------
// Concurrent access: multiple threads loading the same state file
// ---------------------------------------------------------------------------

#[test]
fn concurrent_readers_see_consistent_state() {
    let td = tempdir().unwrap();
    let dir = td.path().join("concurrent");

    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "pkg@1.0.0".to_string(),
        make_progress("pkg", "1.0.0", PackageState::Published),
    );
    let state = make_state("concurrent-plan", pkgs);
    save_state(&dir, &state).unwrap();

    let dir = std::sync::Arc::new(dir);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let dir = std::sync::Arc::clone(&dir);
            std::thread::spawn(move || {
                let loaded = load_state(&dir).unwrap().unwrap();
                assert_eq!(loaded.plan_id, "concurrent-plan");
                assert!(loaded.packages.contains_key("pkg@1.0.0"));
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }
}

// ---------------------------------------------------------------------------
// Snapshot tests for edge cases (deterministic timestamps)
// ---------------------------------------------------------------------------

fn fixed_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap()
}

fn make_deterministic_progress(
    name: &str,
    version: &str,
    state: PackageState,
    attempts: u32,
) -> PackageProgress {
    PackageProgress {
        name: name.to_string(),
        version: version.to_string(),
        attempts,
        state,
        last_updated_at: fixed_time(),
    }
}

fn make_deterministic_state(
    plan_id: &str,
    packages: BTreeMap<String, PackageProgress>,
) -> ExecutionState {
    ExecutionState {
        state_version: CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: sample_registry(),
        created_at: fixed_time(),
        updated_at: fixed_time(),
        packages,
    }
}

fn make_deterministic_package_receipt(
    name: &str,
    version: &str,
    state: PackageState,
) -> PackageReceipt {
    let t = fixed_time();
    PackageReceipt {
        name: name.to_string(),
        version: version.to_string(),
        attempts: 1,
        state,
        started_at: t,
        finished_at: t,
        duration_ms: 1500,
        evidence: PackageEvidence {
            attempts: vec![],
            readiness_checks: vec![],
        },
    }
}

fn make_deterministic_receipt(plan_id: &str, packages: Vec<PackageReceipt>) -> Receipt {
    let t = fixed_time();
    Receipt {
        receipt_version: CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: sample_registry(),
        started_at: t,
        finished_at: t,
        packages,
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    }
}

#[test]
fn snapshot_state_retry_cycle() {
    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        "retried@1.0.0".to_string(),
        make_deterministic_progress("retried", "1.0.0", PackageState::Pending, 2),
    );

    let state = make_deterministic_state("retry-plan", pkgs);
    let json = serde_json::to_string_pretty(&state).unwrap();
    insta::assert_snapshot!("state_retry_cycle", json);
}

#[test]
fn snapshot_receipt_all_published() {
    let receipt = make_deterministic_receipt(
        "all-pub",
        vec![
            make_deterministic_package_receipt("core", "1.0.0", PackageState::Published),
            make_deterministic_package_receipt("utils", "0.5.0", PackageState::Published),
        ],
    );

    let json = serde_json::to_string_pretty(&receipt).unwrap();
    insta::assert_snapshot!("receipt_all_published", json);
}

#[test]
fn snapshot_receipt_mixed_outcomes() {
    let receipt = make_deterministic_receipt(
        "mixed",
        vec![
            make_deterministic_package_receipt("core", "1.0.0", PackageState::Published),
            make_deterministic_package_receipt(
                "utils",
                "0.5.0",
                PackageState::Skipped {
                    reason: "already published".to_string(),
                },
            ),
            make_deterministic_package_receipt(
                "cli",
                "2.0.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "registry timeout".to_string(),
                },
            ),
        ],
    );

    let json = serde_json::to_string_pretty(&receipt).unwrap();
    insta::assert_snapshot!("receipt_mixed_outcomes", json);
}
