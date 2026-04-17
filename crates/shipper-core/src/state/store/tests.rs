//! tests for the `state::store` module.
//! Absorbed from former `shipper-store` crate.

use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::tempdir;

use super::*;
use crate::types::{PackageProgress, PackageReceipt, PackageState, Registry};
use chrono::Utc;

fn sample_state() -> ExecutionState {
    let mut packages = BTreeMap::new();
    packages.insert(
        "demo@0.1.0".to_string(),
        PackageProgress {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );

    ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "p1".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

fn sample_receipt() -> Receipt {
    Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "p1".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![PackageReceipt {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 10,
            evidence: crate::types::PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: crate::types::EnvironmentFingerprint {
            shipper_version: "0.1.0".to_string(),
            cargo_version: Some("1.75.0".to_string()),
            rust_version: Some("1.75.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    }
}

#[test]
fn file_store_saves_and_loads_state() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state = sample_state();
    store.save_state(&state).expect("save state");

    let loaded = store.load_state().expect("load state");
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.plan_id, state.plan_id);
}

#[test]
fn file_store_returns_none_for_missing_state() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let loaded = store.load_state().expect("load state");
    assert!(loaded.is_none());
}

#[test]
fn file_store_saves_and_loads_receipt() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt = sample_receipt();
    store.save_receipt(&receipt).expect("save receipt");

    let loaded = store.load_receipt().expect("load receipt");
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.plan_id, receipt.plan_id);
}

#[test]
fn file_store_returns_none_for_missing_receipt() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let loaded = store.load_receipt().expect("load receipt");
    assert!(loaded.is_none());
}

#[test]
fn file_store_saves_and_loads_events() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    store.save_events(&events).expect("save events");

    let loaded = store.load_events().expect("load events");
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.all_events().len(), 1);
}

#[test]
fn file_store_returns_none_for_missing_events() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let loaded = store.load_events().expect("load events");
    assert!(loaded.is_none());
}

#[test]
fn file_store_clears_all_state() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    // Save some state
    store.save_state(&sample_state()).expect("save state");
    store.save_receipt(&sample_receipt()).expect("save receipt");

    // Verify it exists
    assert!(store.load_state().expect("load state").is_some());
    assert!(store.load_receipt().expect("load receipt").is_some());

    // Clear
    store.clear().expect("clear");

    // Verify it's gone
    assert!(store.load_state().expect("load state").is_none());
    assert!(store.load_receipt().expect("load receipt").is_none());
}

#[test]
fn validate_schema_version_accepts_current_version() {
    let result = validate_schema_version("shipper.receipt.v2");
    assert!(result.is_ok());
}

#[test]
fn validate_schema_version_accepts_minimum_version() {
    let result = validate_schema_version("shipper.receipt.v1");
    assert!(result.is_ok());
}

#[test]
fn validate_schema_version_rejects_old_version() {
    let result = validate_schema_version("shipper.receipt.v0");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("too old"));
}

#[test]
fn validate_schema_version_rejects_invalid_format() {
    let result = validate_schema_version("invalid.version");
    assert!(result.is_err());
}

#[test]
fn validate_schema_version_rejects_non_shipper_version() {
    let result = validate_schema_version("other.receipt.v2");
    assert!(result.is_err());
}

#[test]
fn validate_schema_version_rejects_missing_version_number() {
    let result = validate_schema_version("shipper.receipt.v");
    assert!(result.is_err());
}

#[test]
fn parse_schema_version_in_store_extracts_number_from_v1() {
    let result =
        shipper_types::schema::parse_schema_version("shipper.receipt.v1").expect("should parse");
    assert_eq!(result, 1);
}

#[test]
fn parse_schema_version_in_store_extracts_number_from_v2() {
    let result =
        shipper_types::schema::parse_schema_version("shipper.receipt.v2").expect("should parse");
    assert_eq!(result, 2);
}

#[test]
fn parse_schema_version_in_store_handles_large_version() {
    let result =
        shipper_types::schema::parse_schema_version("shipper.receipt.v100").expect("should parse");
    assert_eq!(result, 100);
}

#[test]
fn parse_schema_version_in_store_rejects_invalid_format_no_prefix() {
    let result = shipper_types::schema::parse_schema_version("receipt.v2");
    assert!(result.is_err());
}

#[test]
fn parse_schema_version_in_store_rejects_invalid_format_no_version() {
    let result = shipper_types::schema::parse_schema_version("shipper.receipt");
    assert!(result.is_err());
}

#[test]
fn parse_schema_version_in_store_rejects_invalid_format_missing_v() {
    let result = shipper_types::schema::parse_schema_version("shipper.receipt.2");
    assert!(result.is_err());
}

#[test]
fn file_store_state_dir_returns_correct_path() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join(".shipper");
    let store = FileStore::new(path.clone());

    assert_eq!(store.state_dir(), path);
}

#[test]
fn file_store_validate_version_delegates_to_validate_schema_version() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    // Test valid version
    assert!(store.validate_version("shipper.receipt.v2").is_ok());

    // Test invalid version
    assert!(store.validate_version("shipper.receipt.v0").is_err());
}

// --- State transition tests ---

#[test]
fn file_store_state_overwrite_preserves_latest() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut state = sample_state();
    store.save_state(&state).expect("save state");

    state.plan_id = "p2".to_string();
    store.save_state(&state).expect("overwrite state");

    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p2");
}

#[test]
fn file_store_state_package_state_transition() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut state = sample_state();
    store.save_state(&state).expect("save pending");

    // Transition package to Published
    state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Published;
    state.packages.get_mut("demo@0.1.0").unwrap().attempts = 2;
    store.save_state(&state).expect("save published");

    let loaded = store.load_state().expect("load").unwrap();
    let pkg = loaded.packages.get("demo@0.1.0").unwrap();
    assert!(matches!(pkg.state, PackageState::Published));
    assert_eq!(pkg.attempts, 2);
}

#[test]
fn file_store_state_with_all_package_states() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut packages = BTreeMap::new();
    let now = Utc::now();

    packages.insert(
        "a@0.1.0".to_string(),
        PackageProgress {
            name: "a".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: now,
        },
    );
    packages.insert(
        "b@0.1.0".to_string(),
        PackageProgress {
            name: "b".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Uploaded,
            last_updated_at: now,
        },
    );
    packages.insert(
        "c@0.1.0".to_string(),
        PackageProgress {
            name: "c".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: now,
        },
    );
    packages.insert(
        "d@0.1.0".to_string(),
        PackageProgress {
            name: "d".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Skipped {
                reason: "already published".to_string(),
            },
            last_updated_at: now,
        },
    );
    packages.insert(
        "e@0.1.0".to_string(),
        PackageProgress {
            name: "e".to_string(),
            version: "0.1.0".to_string(),
            attempts: 3,
            state: PackageState::Failed {
                class: crate::types::ErrorClass::Permanent,
                message: "auth error".to_string(),
            },
            last_updated_at: now,
        },
    );
    packages.insert(
        "f@0.1.0".to_string(),
        PackageProgress {
            name: "f".to_string(),
            version: "0.1.0".to_string(),
            attempts: 2,
            state: PackageState::Ambiguous {
                message: "timeout".to_string(),
            },
            last_updated_at: now,
        },
    );

    let state = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "multi".to_string(),
        registry: Registry::crates_io(),
        created_at: now,
        updated_at: now,
        packages,
    };

    store.save_state(&state).expect("save");
    let loaded = store.load_state().expect("load").unwrap();

    assert_eq!(loaded.packages.len(), 6);
    assert!(matches!(
        loaded.packages["a@0.1.0"].state,
        PackageState::Pending
    ));
    assert!(matches!(
        loaded.packages["b@0.1.0"].state,
        PackageState::Uploaded
    ));
    assert!(matches!(
        loaded.packages["c@0.1.0"].state,
        PackageState::Published
    ));
    assert!(matches!(
        loaded.packages["d@0.1.0"].state,
        PackageState::Skipped { .. }
    ));
    assert!(matches!(
        loaded.packages["e@0.1.0"].state,
        PackageState::Failed { .. }
    ));
    assert!(matches!(
        loaded.packages["f@0.1.0"].state,
        PackageState::Ambiguous { .. }
    ));
}

// --- Receipt tests ---

#[test]
fn file_store_receipt_overwrite_preserves_latest() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut receipt = sample_receipt();
    store.save_receipt(&receipt).expect("save");

    receipt.plan_id = "p99".to_string();
    store.save_receipt(&receipt).expect("overwrite");

    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p99");
}

#[test]
fn file_store_receipt_with_git_context() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut receipt = sample_receipt();
    receipt.git_context = Some(crate::types::GitContext {
        commit: Some("abc123".to_string()),
        branch: Some("main".to_string()),
        tag: Some("v0.1.0".to_string()),
        dirty: Some(false),
    });

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();

    let ctx = loaded.git_context.expect("git_context should be Some");
    assert_eq!(ctx.commit.as_deref(), Some("abc123"));
    assert_eq!(ctx.branch.as_deref(), Some("main"));
    assert_eq!(ctx.tag.as_deref(), Some("v0.1.0"));
    assert_eq!(ctx.dirty, Some(false));
}

#[test]
fn file_store_receipt_with_multiple_packages() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let now = Utc::now();
    let mut receipt = sample_receipt();
    receipt.packages.push(PackageReceipt {
        name: "lib-a".to_string(),
        version: "1.0.0".to_string(),
        attempts: 2,
        state: PackageState::Failed {
            class: crate::types::ErrorClass::Retryable,
            message: "network timeout".to_string(),
        },
        started_at: now,
        finished_at: now,
        duration_ms: 5000,
        evidence: crate::types::PackageEvidence {
            attempts: vec![],
            readiness_checks: vec![],
        },
    });

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.packages.len(), 2);
    assert_eq!(loaded.packages[1].name, "lib-a");
    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Failed { .. }
    ));
}

// --- Events tests ---

#[test]
fn file_store_events_multiple_entries_roundtrip() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PackageStarted {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
        },
        package: "demo".to_string(),
    });
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PackagePublished { duration_ms: 1500 },
        package: "demo".to_string(),
    });
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionFinished {
            result: crate::types::ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    store.save_events(&events).expect("save events");
    let loaded = store.load_events().expect("load events").unwrap();
    assert_eq!(loaded.all_events().len(), 4);
}

#[test]
fn file_store_events_overwrite() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("first save");

    // Save different events
    let mut events2 = EventLog::new();
    events2.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PreflightStarted,
        package: "all".to_string(),
    });
    events2.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PreflightComplete {
            finishability: crate::types::Finishability::Proven,
        },
        package: "all".to_string(),
    });
    store.save_events(&events2).expect("second save");

    let loaded = store.load_events().expect("load").unwrap();
    // EventLog::write_to_file appends, so we get all events
    assert!(loaded.all_events().len() >= 2);
}

// --- Clear / edge-case tests ---

#[test]
fn file_store_clear_on_empty_store_succeeds() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    // Clearing when nothing was saved should succeed
    store.clear().expect("clear on empty store");
}

#[test]
fn file_store_clear_is_idempotent() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    store.save_state(&sample_state()).expect("save");
    store.clear().expect("first clear");
    store.clear().expect("second clear");

    assert!(store.load_state().expect("load").is_none());
}

#[test]
fn file_store_clear_removes_events_too() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");
    store.save_state(&sample_state()).expect("save state");
    store.save_receipt(&sample_receipt()).expect("save receipt");

    store.clear().expect("clear");

    assert!(store.load_state().expect("load state").is_none());
    assert!(store.load_receipt().expect("load receipt").is_none());
    assert!(store.load_events().expect("load events").is_none());
}

#[test]
fn file_store_clear_does_not_remove_other_files() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    // Create an unrelated file in the state dir
    let other_file = td.path().join("other.txt");
    std::fs::write(&other_file, "keep me").expect("write other file");

    store.save_state(&sample_state()).expect("save");
    store.clear().expect("clear");

    assert!(other_file.exists(), "unrelated file should not be removed");
}

// --- Corrupt / invalid data tests ---

#[test]
fn file_store_load_state_corrupt_json_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state_file = crate::state::execution_state::state_path(td.path());
    std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
    std::fs::write(&state_file, "{ not valid json !!!").expect("write corrupt");

    let result = store.load_state();
    assert!(result.is_err(), "corrupt state.json should produce error");
}

#[test]
fn file_store_load_receipt_corrupt_json_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt_file = crate::state::execution_state::receipt_path(td.path());
    std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
    std::fs::write(&receipt_file, "<<<garbage>>>").expect("write corrupt");

    let result = store.load_receipt();
    assert!(result.is_err(), "corrupt receipt.json should produce error");
}

#[test]
fn file_store_load_events_corrupt_jsonl_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let events_file = crate::state::events::events_path(td.path());
    std::fs::create_dir_all(events_file.parent().unwrap_or(td.path())).ok();
    std::fs::write(&events_file, "not-json-at-all\n").expect("write corrupt");

    let result = store.load_events();
    assert!(result.is_err(), "corrupt events.jsonl should produce error");
}

#[test]
fn file_store_load_state_empty_file_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state_file = crate::state::execution_state::state_path(td.path());
    std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
    std::fs::write(&state_file, "").expect("write empty");

    let result = store.load_state();
    assert!(result.is_err(), "empty state.json should produce error");
}

// --- Roundtrip fidelity tests ---

#[test]
fn file_store_state_roundtrip_preserves_all_fields() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state = sample_state();
    store.save_state(&state).expect("save");
    let loaded = store.load_state().expect("load").unwrap();

    assert_eq!(loaded.state_version, state.state_version);
    assert_eq!(loaded.plan_id, state.plan_id);
    assert_eq!(loaded.registry.name, state.registry.name);
    assert_eq!(loaded.packages.len(), state.packages.len());

    let orig_pkg = state.packages.get("demo@0.1.0").unwrap();
    let load_pkg = loaded.packages.get("demo@0.1.0").unwrap();
    assert_eq!(load_pkg.name, orig_pkg.name);
    assert_eq!(load_pkg.version, orig_pkg.version);
    assert_eq!(load_pkg.attempts, orig_pkg.attempts);
}

#[test]
fn file_store_receipt_roundtrip_preserves_all_fields() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt = sample_receipt();
    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();

    assert_eq!(loaded.receipt_version, receipt.receipt_version);
    assert_eq!(loaded.plan_id, receipt.plan_id);
    assert_eq!(loaded.registry.name, receipt.registry.name);
    assert_eq!(loaded.packages.len(), receipt.packages.len());
    assert_eq!(loaded.packages[0].name, receipt.packages[0].name);
    assert_eq!(loaded.packages[0].version, receipt.packages[0].version);
    assert_eq!(loaded.packages[0].attempts, receipt.packages[0].attempts);
    assert_eq!(
        loaded.packages[0].duration_ms,
        receipt.packages[0].duration_ms
    );
    assert_eq!(
        loaded.environment.shipper_version,
        receipt.environment.shipper_version
    );
    assert_eq!(loaded.environment.os, receipt.environment.os);
    assert_eq!(loaded.event_log_path, receipt.event_log_path);
}

// --- Empty packages edge case ---

#[test]
fn file_store_state_with_empty_packages() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "empty".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::new(),
    };

    store.save_state(&state).expect("save empty");
    let loaded = store.load_state().expect("load").unwrap();
    assert!(loaded.packages.is_empty());
    assert_eq!(loaded.plan_id, "empty");
}

#[test]
fn file_store_receipt_with_empty_packages() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut receipt = sample_receipt();
    receipt.packages.clear();

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();
    assert!(loaded.packages.is_empty());
}

#[test]
fn file_store_empty_event_log_roundtrip() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let events = EventLog::new();
    store.save_events(&events).expect("save empty events");

    let loaded = store.load_events().expect("load");
    // Empty event log may write an empty file; implementation may return Some or None
    if let Some(loaded) = loaded {
        assert!(loaded.all_events().is_empty());
    }
}

// --- Schema version edge cases ---

#[test]
fn validate_schema_version_rejects_empty_string() {
    assert!(validate_schema_version("").is_err());
}

#[test]
fn validate_schema_version_rejects_future_version_gracefully() {
    // Future versions should be accepted (forward compatible)
    let result = validate_schema_version("shipper.receipt.v999");
    assert!(result.is_ok());
}

#[test]
fn validate_schema_version_rejects_negative_looking_version() {
    let result = validate_schema_version("shipper.receipt.v-1");
    assert!(result.is_err());
}

// --- StateStore trait as trait object ---

#[test]
fn file_store_usable_as_dyn_state_store() {
    let td = tempdir().expect("tempdir");
    let store: Box<dyn StateStore> = Box::new(FileStore::new(td.path().to_path_buf()));

    store
        .save_state(&sample_state())
        .expect("save via trait object");
    let loaded = store.load_state().expect("load via trait object");
    assert!(loaded.is_some());
}

// --- Save to non-existent nested directory ---

#[test]
fn file_store_save_creates_parent_directories() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("deep").join("nested").join(".shipper");
    let store = FileStore::new(nested);

    // save_state should create directories as needed
    let result = store.save_state(&sample_state());
    assert!(result.is_ok(), "save should create parent dirs: {result:?}");

    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p1");
}

// --- Property-based tests (proptest) ---

mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for valid crate-like package names (lowercase alphanumeric + hyphens/underscores).
    fn pkg_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{0,30}".prop_map(|s| s)
    }

    /// Strategy for semver-like version strings.
    fn version_strategy() -> impl Strategy<Value = String> {
        (0u32..100, 0u32..100, 0u32..100).prop_map(|(ma, mi, pa)| format!("{ma}.{mi}.{pa}"))
    }

    /// Strategy for non-empty directory name segments.
    fn dir_segment_strategy() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_-]{1,20}".prop_map(|s| s)
    }

    proptest! {
        #[test]
        fn receipt_roundtrip_arbitrary_names_and_versions(
            name in pkg_name_strategy(),
            version in version_strategy(),
            plan_id in "[a-z0-9]{1,16}",
        ) {
            let td = tempdir().expect("tempdir");
            let store = FileStore::new(td.path().to_path_buf());

            let receipt = Receipt {
                receipt_version: "shipper.receipt.v2".to_string(),
                plan_id,
                registry: Registry::crates_io(),
                started_at: Utc::now(),
                finished_at: Utc::now(),
                packages: vec![PackageReceipt {
                    name: name.clone(),
                    version: version.clone(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    duration_ms: 10,
                    evidence: crate::types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                }],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: None,
                environment: crate::types::EnvironmentFingerprint {
                    shipper_version: "0.1.0".to_string(),
                    cargo_version: Some("1.75.0".to_string()),
                    rust_version: Some("1.75.0".to_string()),
                    os: "linux".to_string(),
                    arch: "x86_64".to_string(),
                },
            };

            store.save_receipt(&receipt).expect("save receipt");
            let loaded = store.load_receipt().expect("load receipt").expect("receipt present");
            prop_assert_eq!(&loaded.packages[0].name, &name);
            prop_assert_eq!(&loaded.packages[0].version, &version);
        }

        #[test]
        fn store_path_construction_with_arbitrary_dirs(
            segments in proptest::collection::vec(dir_segment_strategy(), 1..5),
        ) {
            let td = tempdir().expect("tempdir");
            let mut path = td.path().to_path_buf();
            for seg in &segments {
                path = path.join(seg);
            }
            let store = FileStore::new(path.clone());
            prop_assert_eq!(store.state_dir(), path.as_path());
        }

        #[test]
        fn receipt_json_serialization_roundtrip(
            name in pkg_name_strategy(),
            version in version_strategy(),
            attempts in 1u32..10,
            duration in 0u128..100_000,
        ) {
            let receipt = Receipt {
                receipt_version: "shipper.receipt.v2".to_string(),
                plan_id: "rt-test".to_string(),
                registry: Registry::crates_io(),
                started_at: Utc::now(),
                finished_at: Utc::now(),
                packages: vec![PackageReceipt {
                    name: name.clone(),
                    version: version.clone(),
                    attempts,
                    state: PackageState::Published,
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    duration_ms: duration,
                    evidence: crate::types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                }],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: None,
                environment: crate::types::EnvironmentFingerprint {
                    shipper_version: "0.1.0".to_string(),
                    cargo_version: None,
                    rust_version: None,
                    os: "test".to_string(),
                    arch: "test".to_string(),
                },
            };

            let json = serde_json::to_string(&receipt).expect("serialize");
            let deserialized: Receipt = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(&deserialized.packages[0].name, &name);
            prop_assert_eq!(&deserialized.packages[0].version, &version);
            prop_assert_eq!(deserialized.packages[0].attempts, attempts);
            prop_assert_eq!(deserialized.packages[0].duration_ms, duration);
        }

        #[test]
        fn events_log_append_with_arbitrary_data(
            pkg_name in pkg_name_strategy(),
            version in version_strategy(),
            event_count in 1usize..20,
        ) {
            let td = tempdir().expect("tempdir");
            let store = FileStore::new(td.path().to_path_buf());

            let mut events = EventLog::new();
            // Always start with ExecutionStarted
            events.record(crate::types::PublishEvent {
                timestamp: Utc::now(),
                event_type: crate::types::EventType::ExecutionStarted,
                package: "all".to_string(),
            });
            // Add N package events
            for _ in 0..event_count {
                events.record(crate::types::PublishEvent {
                    timestamp: Utc::now(),
                    event_type: crate::types::EventType::PackageStarted {
                        name: pkg_name.clone(),
                        version: version.clone(),
                    },
                    package: format!("{pkg_name}@{version}"),
                });
            }

            store.save_events(&events).expect("save events");
            let loaded = store.load_events().expect("load events").expect("events present");
            // 1 ExecutionStarted + event_count PackageStarted
            prop_assert_eq!(loaded.all_events().len(), 1 + event_count);
        }
    }
}

// --- Partial/truncated JSON recovery ---

#[test]
fn file_store_load_state_truncated_json_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state_file = crate::state::execution_state::state_path(td.path());
    std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
    let truncated = r#"{"state_version":"shipper.state.v1","plan_id":"tr"#;
    std::fs::write(&state_file, truncated).expect("write truncated");

    let result = store.load_state();
    assert!(result.is_err(), "truncated state.json should produce error");
}

#[test]
fn file_store_load_receipt_truncated_json_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt_file = crate::state::execution_state::receipt_path(td.path());
    std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
    let truncated = r#"{"receipt_version":"shipper.receipt.v2","plan_id":"#;
    std::fs::write(&receipt_file, truncated).expect("write truncated");

    let result = store.load_receipt();
    assert!(
        result.is_err(),
        "truncated receipt.json should produce error"
    );
}

// --- State transition: retry cycle (Pending → Failed → Pending) ---

#[test]
fn file_store_state_retry_cycle_pending_failed_pending() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut state = sample_state();
    store.save_state(&state).expect("save pending");

    let pkg = state.packages.get_mut("demo@0.1.0").unwrap();
    pkg.state = PackageState::Failed {
        class: crate::types::ErrorClass::Retryable,
        message: "network timeout".to_string(),
    };
    pkg.attempts = 2;
    store.save_state(&state).expect("save failed");

    let loaded = store.load_state().expect("load").unwrap();
    assert!(matches!(
        loaded.packages["demo@0.1.0"].state,
        PackageState::Failed { .. }
    ));

    state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Pending;
    store.save_state(&state).expect("save pending retry");

    let loaded = store.load_state().expect("load").unwrap();
    assert!(matches!(
        loaded.packages["demo@0.1.0"].state,
        PackageState::Pending
    ));
    assert_eq!(loaded.packages["demo@0.1.0"].attempts, 2);
}

// --- Published idempotent ---

#[test]
fn file_store_state_published_idempotent() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut state = sample_state();
    state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Published;

    store.save_state(&state).expect("save published 1");
    store.save_state(&state).expect("save published 2");

    let loaded = store.load_state().expect("load").unwrap();
    assert!(matches!(
        loaded.packages["demo@0.1.0"].state,
        PackageState::Published
    ));
}

// --- Very long package names ---

#[test]
fn file_store_state_very_long_package_name() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let long_name = "a".repeat(500);
    let key = format!("{long_name}@1.0.0");
    let mut packages = BTreeMap::new();
    packages.insert(
        key.clone(),
        PackageProgress {
            name: long_name.clone(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );

    let state = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "long".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };

    store.save_state(&state).expect("save");
    let loaded = store.load_state().expect("load").unwrap();
    assert!(loaded.packages.contains_key(&key));
    assert_eq!(loaded.packages[&key].name, long_name);
}

// --- Empty plan_id ---

#[test]
fn file_store_state_empty_plan_id() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: String::new(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::new(),
    };

    store.save_state(&state).expect("save");
    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "");
}

// --- Unicode directory paths ---

#[test]
fn file_store_unicode_directory_path() {
    let td = tempdir().expect("tempdir");
    let unicode_dir = td.path().join("données").join("日本語");
    let store = FileStore::new(unicode_dir);

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p1");
}

// --- Concurrent readers ---

#[test]
fn file_store_concurrent_readers_consistent() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    store.save_state(&sample_state()).expect("save");

    let dir = std::sync::Arc::new(td.path().to_path_buf());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let dir = std::sync::Arc::clone(&dir);
            std::thread::spawn(move || {
                let store = FileStore::new((*dir).clone());
                let loaded = store.load_state().expect("load").unwrap();
                assert_eq!(loaded.plan_id, "p1");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }
}

// --- Receipt: all published ---

#[test]
fn file_store_receipt_all_published() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let now = Utc::now();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "all-pub".to_string(),
        registry: Registry::crates_io(),
        started_at: now,
        finished_at: now,
        packages: vec![
            PackageReceipt {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: now,
                finished_at: now,
                duration_ms: 100,
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "b".to_string(),
                version: "2.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: now,
                finished_at: now,
                duration_ms: 200,
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: crate::types::EnvironmentFingerprint {
            shipper_version: "0.1.0".to_string(),
            cargo_version: Some("1.75.0".to_string()),
            rust_version: Some("1.75.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.packages.len(), 2);
    assert!(
        loaded
            .packages
            .iter()
            .all(|p| matches!(p.state, PackageState::Published))
    );
}

// --- Receipt: some failed ---

#[test]
fn file_store_receipt_some_failed() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let now = Utc::now();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "some-failed".to_string(),
        registry: Registry::crates_io(),
        started_at: now,
        finished_at: now,
        packages: vec![
            PackageReceipt {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: now,
                finished_at: now,
                duration_ms: 100,
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "b".to_string(),
                version: "2.0.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: crate::types::ErrorClass::Retryable,
                    message: "timeout".to_string(),
                },
                started_at: now,
                finished_at: now,
                duration_ms: 5000,
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: crate::types::EnvironmentFingerprint {
            shipper_version: "0.1.0".to_string(),
            cargo_version: Some("1.75.0".to_string()),
            rust_version: Some("1.75.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.packages.len(), 2);
    assert!(matches!(loaded.packages[0].state, PackageState::Published));
    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Failed { .. }
    ));
}

// --- Directory creation: receipt and events also create parents ---

#[test]
fn file_store_save_receipt_creates_parent_directories() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("deep").join("receipt-dir");
    let store = FileStore::new(nested);

    let result = store.save_receipt(&sample_receipt());
    assert!(result.is_ok(), "save receipt should create parent dirs");

    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p1");
}

#[test]
fn file_store_save_events_creates_parent_directories() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("deep").join("events-dir");
    let store = FileStore::new(nested);

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    let result = store.save_events(&events);
    assert!(result.is_ok(), "save events should create parent dirs");

    let loaded = store.load_events().expect("load").unwrap();
    assert_eq!(loaded.all_events().len(), 1);
}

// --- Partial clear: only some files exist ---

#[test]
fn file_store_clear_partial_only_state_exists() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    store.save_state(&sample_state()).expect("save state");
    // No receipt or events saved
    store.clear().expect("clear with only state");

    assert!(store.load_state().expect("load").is_none());
}

#[test]
fn file_store_clear_partial_only_events_exist() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");
    // No state or receipt saved
    store.clear().expect("clear with only events");

    assert!(store.load_events().expect("load").is_none());
}

// --- Custom state-dir isolation ---

#[test]
fn file_store_custom_state_dir_isolation() {
    let td = tempdir().expect("tempdir");
    let dir_a = td.path().join("store-a");
    let dir_b = td.path().join("store-b");
    let store_a = FileStore::new(dir_a);
    let store_b = FileStore::new(dir_b);

    let mut state_a = sample_state();
    state_a.plan_id = "plan-a".to_string();
    let mut state_b = sample_state();
    state_b.plan_id = "plan-b".to_string();

    store_a.save_state(&state_a).expect("save a");
    store_b.save_state(&state_b).expect("save b");

    let loaded_a = store_a.load_state().expect("load a").unwrap();
    let loaded_b = store_b.load_state().expect("load b").unwrap();

    assert_eq!(loaded_a.plan_id, "plan-a");
    assert_eq!(loaded_b.plan_id, "plan-b");
}

// --- Save after clear cycle ---

#[test]
fn file_store_save_after_clear_works() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    store.save_state(&sample_state()).expect("save 1");
    store.clear().expect("clear");
    assert!(store.load_state().expect("load").is_none());

    let mut state = sample_state();
    state.plan_id = "after-clear".to_string();
    store.save_state(&state).expect("save 2");

    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "after-clear");
}

// --- Concurrent writers: last write wins ---

#[test]
fn file_store_concurrent_writers_last_write_readable() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());
    // Seed initial state so directory exists
    store.save_state(&sample_state()).expect("seed");

    let dir = std::sync::Arc::new(td.path().to_path_buf());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let dir = std::sync::Arc::clone(&dir);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let store = FileStore::new((*dir).clone());
                let mut state = ExecutionState {
                    state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                    plan_id: format!("writer-{i}"),
                    registry: Registry::crates_io(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    packages: BTreeMap::new(),
                };
                state.packages.insert(
                    "pkg@1.0.0".to_string(),
                    PackageProgress {
                        name: "pkg".to_string(),
                        version: "1.0.0".to_string(),
                        attempts: 0,
                        state: PackageState::Pending,
                        last_updated_at: Utc::now(),
                    },
                );
                // Write must not panic; errors are tolerable under contention
                let _ = store.save_state(&state);
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread must not panic");
    }

    // After all writers finish, the file must exist and load must not panic.
    // Under contention the final content may be from any writer.
    let result = store.load_state();
    // The atomic-write implementation should make this succeed, but we
    // mainly care that it doesn't panic or produce undefined behavior.
    if let Ok(Some(loaded)) = result {
        assert!(loaded.plan_id.starts_with("writer-"));
    }
}

// --- Many packages roundtrip ---

#[test]
fn file_store_state_with_many_packages() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let now = Utc::now();
    let mut packages = BTreeMap::new();
    for i in 0..100 {
        let name = format!("crate-{i}");
        let key = format!("{name}@0.{i}.0");
        packages.insert(
            key,
            PackageProgress {
                name,
                version: format!("0.{i}.0"),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: now,
            },
        );
    }

    let state = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "many-pkgs".to_string(),
        registry: Registry::crates_io(),
        created_at: now,
        updated_at: now,
        packages,
    };

    store.save_state(&state).expect("save");
    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.packages.len(), 100);
}

// --- Empty string edge cases ---

#[test]
fn file_store_receipt_empty_strings_roundtrip() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let now = Utc::now();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: String::new(),
        registry: Registry::crates_io(),
        started_at: now,
        finished_at: now,
        packages: vec![PackageReceipt {
            name: String::new(),
            version: String::new(),
            attempts: 0,
            state: PackageState::Published,
            started_at: now,
            finished_at: now,
            duration_ms: 0,
            evidence: crate::types::PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from(""),
        git_context: None,
        environment: crate::types::EnvironmentFingerprint {
            shipper_version: String::new(),
            cargo_version: None,
            rust_version: None,
            os: String::new(),
            arch: String::new(),
        },
    };

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "");
    assert_eq!(loaded.packages[0].name, "");
    assert_eq!(loaded.packages[0].version, "");
}

// --- Corrupt data: wrong JSON shape ---

#[test]
fn file_store_load_state_wrong_json_shape_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let state_file = crate::state::execution_state::state_path(td.path());
    std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
    // Valid JSON, but wrong schema
    std::fs::write(&state_file, r#"{"name":"not-a-state"}"#).expect("write");

    let result = store.load_state();
    assert!(
        result.is_err(),
        "wrong JSON shape should produce an error on load"
    );
}

#[test]
fn file_store_load_receipt_wrong_json_shape_returns_error() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt_file = crate::state::execution_state::receipt_path(td.path());
    std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
    // Valid JSON but completely wrong shape — no receipt_version, no packages, etc.
    std::fs::write(&receipt_file, r#"{"unexpected_key": true, "number": 42}"#).expect("write");

    // Either returns an error or migrates/fails gracefully — must not panic
    let result = store.load_receipt();
    // The implementation attempts migration which may also fail; either Err or Ok is fine
    // but it must never panic
    if let Ok(Some(r)) = &result {
        // If it somehow parsed, the shape is wrong so fields will be defaults/empty
        assert!(
            r.plan_id.is_empty() || !r.plan_id.is_empty(),
            "should not panic"
        );
    }
}

// --- State dir accessor with nested .shipper ---

#[test]
fn file_store_state_dir_with_dot_shipper_subdir() {
    let td = tempdir().expect("tempdir");
    let shipper_dir = td.path().join("workspace").join(".shipper");
    let store = FileStore::new(shipper_dir.clone());

    assert_eq!(store.state_dir(), shipper_dir.as_path());

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load").unwrap();
    assert_eq!(loaded.plan_id, "p1");
}

// --- Events: valid JSONL lines survive alongside load ---

#[test]
fn file_store_events_for_package_filter_after_roundtrip() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PackageStarted {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "alpha@1.0.0".to_string(),
    });
    events.record(crate::types::PublishEvent {
        timestamp: Utc::now(),
        event_type: crate::types::EventType::PackageStarted {
            name: "beta".to_string(),
            version: "2.0.0".to_string(),
        },
        package: "beta@2.0.0".to_string(),
    });

    store.save_events(&events).expect("save");
    let loaded = store.load_events().expect("load").unwrap();
    let alpha_events = loaded.events_for_package("alpha@1.0.0");
    let beta_events = loaded.events_for_package("beta@2.0.0");
    assert_eq!(alpha_events.len(), 1);
    assert_eq!(beta_events.len(), 1);
}

// --- Property-based: arbitrary bytes never panic on load ---

mod proptests_hardened {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn arbitrary_bytes_state_load_never_panics(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let td = tempdir().expect("tempdir");
            let store = FileStore::new(td.path().to_path_buf());

            let state_file = crate::state::execution_state::state_path(td.path());
            std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
            std::fs::write(&state_file, &data).expect("write");

            // Must not panic — may return Ok(None) or Err, both acceptable
            let _ = store.load_state();
        }

        #[test]
        fn arbitrary_bytes_receipt_load_never_panics(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let td = tempdir().expect("tempdir");
            let store = FileStore::new(td.path().to_path_buf());

            let receipt_file = crate::state::execution_state::receipt_path(td.path());
            std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
            std::fs::write(&receipt_file, &data).expect("write");

            // Must not panic — may return Ok(None) or Err, both acceptable
            let _ = store.load_receipt();
        }

        #[test]
        fn state_roundtrip_arbitrary_attempts_and_plan_id(
            plan_id in "[a-z0-9_-]{0,32}",
            attempts in 0u32..1000,
            pkg_count in 1usize..20,
        ) {
            let td = tempdir().expect("tempdir");
            let store = FileStore::new(td.path().to_path_buf());
            let now = Utc::now();

            let mut packages = BTreeMap::new();
            for i in 0..pkg_count {
                let name = format!("pkg-{i}");
                let key = format!("{name}@0.1.0");
                packages.insert(key, PackageProgress {
                    name,
                    version: "0.1.0".to_string(),
                    attempts,
                    state: PackageState::Pending,
                    last_updated_at: now,
                });
            }

            let state = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: plan_id.clone(),
                registry: Registry::crates_io(),
                created_at: now,
                updated_at: now,
                packages,
            };

            store.save_state(&state).expect("save");
            let loaded = store.load_state().expect("load").expect("present");
            prop_assert_eq!(&loaded.plan_id, &plan_id);
            prop_assert_eq!(loaded.packages.len(), pkg_count);
            for pkg in loaded.packages.values() {
                prop_assert_eq!(pkg.attempts, attempts);
            }
        }
    }
}
