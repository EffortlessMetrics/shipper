//! snapshot_tests for the `state::store` module.
//! Absorbed from former `shipper-store` crate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};

use crate::state::events::EventLog;
use crate::types::{
    EnvironmentFingerprint, ErrorClass, EventType, ExecutionResult, ExecutionState, GitContext,
    PackageEvidence, PackageProgress, PackageReceipt, PackageState, PublishEvent, Receipt,
    Registry,
};

use super::*;

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap()
}

// ── ExecutionState snapshots ────────────────────────────────────

#[test]
fn snapshot_execution_state_single_pending() {
    let t = fixed_time();
    let mut packages = BTreeMap::new();
    packages.insert(
        "demo@0.1.0".to_string(),
        PackageProgress {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: t,
        },
    );

    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-abc".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages,
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    insta::assert_snapshot!("execution_state_single_pending", json);
}

#[test]
fn snapshot_execution_state_all_package_states() {
    let t = fixed_time();
    let mut packages = BTreeMap::new();

    packages.insert(
        "a@1.0.0".to_string(),
        PackageProgress {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: t,
        },
    );
    packages.insert(
        "b@1.0.0".to_string(),
        PackageProgress {
            name: "b".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Uploaded,
            last_updated_at: t,
        },
    );
    packages.insert(
        "c@1.0.0".to_string(),
        PackageProgress {
            name: "c".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: t,
        },
    );
    packages.insert(
        "d@1.0.0".to_string(),
        PackageProgress {
            name: "d".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".to_string(),
            },
            last_updated_at: t,
        },
    );
    packages.insert(
        "e@1.0.0".to_string(),
        PackageProgress {
            name: "e".to_string(),
            version: "1.0.0".to_string(),
            attempts: 3,
            state: PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth error".to_string(),
            },
            last_updated_at: t,
        },
    );
    packages.insert(
        "f@1.0.0".to_string(),
        PackageProgress {
            name: "f".to_string(),
            version: "1.0.0".to_string(),
            attempts: 2,
            state: PackageState::Ambiguous {
                message: "timeout during upload".to_string(),
            },
            last_updated_at: t,
        },
    );

    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-multi".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages,
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    insta::assert_snapshot!("execution_state_all_package_states", json);
}

#[test]
fn snapshot_execution_state_empty_packages() {
    let t = fixed_time();
    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-empty".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages: BTreeMap::new(),
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    insta::assert_snapshot!("execution_state_empty_packages", json);
}

// ── Receipt snapshots ───────────────────────────────────────────

#[test]
fn snapshot_receipt_minimal() {
    let t = fixed_time();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-min".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![PackageReceipt {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: t,
            finished_at: t,
            duration_ms: 1500,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    let json = serde_json::to_string_pretty(&receipt).expect("serialize");
    insta::assert_snapshot!("receipt_minimal", json);
}

#[test]
fn snapshot_receipt_with_git_context() {
    let t = fixed_time();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-git".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![PackageReceipt {
            name: "my-lib".to_string(),
            version: "2.0.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: t,
            finished_at: t,
            duration_ms: 3200,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: Some(GitContext {
            commit: Some("abc123def456".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v2.0.0".to_string()),
            dirty: Some(false),
        }),
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    let json = serde_json::to_string_pretty(&receipt).expect("serialize");
    insta::assert_snapshot!("receipt_with_git_context", json);
}

#[test]
fn snapshot_receipt_mixed_outcomes() {
    let t = fixed_time();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-mixed".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![
            PackageReceipt {
                name: "core".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 2000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "utils".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Skipped {
                    reason: "already published".to_string(),
                },
                started_at: t,
                finished_at: t,
                duration_ms: 0,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "cli".to_string(),
                version: "1.0.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "registry timeout after 3 attempts".to_string(),
                },
                started_at: t,
                finished_at: t,
                duration_ms: 45000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
        },
    };

    let json = serde_json::to_string_pretty(&receipt).expect("serialize");
    insta::assert_snapshot!("receipt_mixed_outcomes", json);
}

// ── FileStore persistence format snapshots ──────────────────────

#[test]
fn snapshot_state_persisted_json() {
    let t = fixed_time();
    let td = tempfile::tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut packages = BTreeMap::new();
    packages.insert(
        "alpha@0.1.0".to_string(),
        PackageProgress {
            name: "alpha".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: t,
        },
    );
    packages.insert(
        "beta@0.2.0".to_string(),
        PackageProgress {
            name: "beta".to_string(),
            version: "0.2.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: t,
        },
    );

    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-persist".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages,
    };

    store.save_state(&state).expect("save");
    let raw = std::fs::read_to_string(crate::state::execution_state::state_path(td.path())).expect("read");
    let roundtrip: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    let pretty = serde_json::to_string_pretty(&roundtrip).expect("pretty");
    insta::assert_snapshot!("state_persisted_json", pretty);
}

#[test]
fn snapshot_receipt_persisted_json() {
    let t = fixed_time();
    let td = tempfile::tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-persist".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![PackageReceipt {
            name: "alpha".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: t,
            finished_at: t,
            duration_ms: 5000,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    store.save_receipt(&receipt).expect("save");
    let raw = std::fs::read_to_string(crate::state::execution_state::receipt_path(td.path())).expect("read");
    let roundtrip: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    let pretty = serde_json::to_string_pretty(&roundtrip).expect("pretty");
    insta::assert_snapshot!("receipt_persisted_json", pretty);
}

// ── Event serialization snapshots ───────────────────────────────

#[test]
fn snapshot_event_execution_started() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_execution_started", json);
}

#[test]
fn snapshot_event_package_started() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::PackageStarted {
            name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "my-crate@1.0.0".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_package_started", json);
}

#[test]
fn snapshot_event_package_published() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::PackagePublished { duration_ms: 4200 },
        package: "my-crate@1.0.0".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_package_published", json);
}

#[test]
fn snapshot_event_package_failed() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "connection reset by peer".to_string(),
        },
        package: "my-crate@1.0.0".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_package_failed", json);
}

#[test]
fn snapshot_event_package_skipped() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::PackageSkipped {
            reason: "version already exists on registry".to_string(),
        },
        package: "my-crate@1.0.0".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_package_skipped", json);
}

#[test]
fn snapshot_event_execution_finished_success() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_execution_finished_success", json);
}

#[test]
fn snapshot_event_execution_finished_partial_failure() {
    let t = fixed_time();
    let event = PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::PartialFailure,
        },
        package: "all".to_string(),
    };
    let json = serde_json::to_string_pretty(&event).expect("serialize");
    insta::assert_snapshot!("event_execution_finished_partial_failure", json);
}

// ── Schema version error message snapshots ──────────────────────

#[test]
fn snapshot_error_version_too_old() {
    let err = validate_schema_version("shipper.receipt.v0")
        .unwrap_err()
        .to_string();
    insta::assert_snapshot!("error_version_too_old", err);
}

#[test]
fn snapshot_error_invalid_version_format() {
    let err = validate_schema_version("invalid.version")
        .unwrap_err()
        .to_string();
    insta::assert_snapshot!("error_invalid_version_format", err);
}

#[test]
fn snapshot_error_empty_version() {
    let err = validate_schema_version("").unwrap_err().to_string();
    insta::assert_snapshot!("error_empty_version", err);
}

#[test]
fn snapshot_error_missing_v_prefix() {
    let err = validate_schema_version("shipper.receipt.2")
        .unwrap_err()
        .to_string();
    insta::assert_snapshot!("error_missing_v_prefix", err);
}

#[test]
fn snapshot_error_non_numeric_version() {
    let err = validate_schema_version("shipper.receipt.vx")
        .unwrap_err()
        .to_string();
    insta::assert_snapshot!("error_non_numeric_version", err);
}

// ── Events JSONL persisted format ───────────────────────────────

#[test]
fn snapshot_events_persisted_jsonl() {
    let t = fixed_time();
    let td = tempfile::tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: t,
        event_type: EventType::PackageStarted {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
        },
        package: "demo@0.1.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: t,
        event_type: EventType::PackagePublished { duration_ms: 2500 },
        package: "demo@0.1.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    store.save_events(&events).expect("save");
    let raw = std::fs::read_to_string(crate::state::events::events_path(td.path())).expect("read");
    // Normalize each line to pretty JSON for readable snapshot
    let pretty_lines: Vec<String> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("parse line");
            serde_json::to_string_pretty(&v).expect("pretty")
        })
        .collect();
    let snapshot = pretty_lines.join("\n---\n");
    insta::assert_snapshot!("events_persisted_jsonl", snapshot);
}

// ── Registry serialization snapshot ─────────────────────────────

#[test]
fn snapshot_registry_crates_io() {
    let registry = Registry::crates_io();
    let json = serde_json::to_string_pretty(&registry).expect("serialize");
    insta::assert_snapshot!("registry_crates_io", json);
}

#[test]
fn snapshot_registry_custom() {
    let registry = Registry {
        name: "my-registry".to_string(),
        api_base: "https://my-registry.example.com".to_string(),
        index_base: Some("https://index.my-registry.example.com".to_string()),
    };
    let json = serde_json::to_string_pretty(&registry).expect("serialize");
    insta::assert_snapshot!("registry_custom", json);
}

// ── Edge case snapshot: retry cycle state ───────────────────────

#[test]
fn snapshot_state_retry_cycle() {
    let t = fixed_time();
    let mut packages = BTreeMap::new();
    packages.insert(
        "retried@1.0.0".to_string(),
        PackageProgress {
            name: "retried".to_string(),
            version: "1.0.0".to_string(),
            attempts: 2,
            state: PackageState::Pending,
            last_updated_at: t,
        },
    );

    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-retry".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages,
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    insta::assert_snapshot!("state_retry_cycle", json);
}

// ── Edge case snapshot: receipt all published ────────────────────

#[test]
fn snapshot_receipt_all_published() {
    let t = fixed_time();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-all-pub".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![
            PackageReceipt {
                name: "core".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 2000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "utils".to_string(),
                version: "0.5.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 1500,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    let json = serde_json::to_string_pretty(&receipt).expect("serialize");
    insta::assert_snapshot!("receipt_all_published", json);
}

// ── Edge case snapshot: receipt some failed ──────────────────────

#[test]
fn snapshot_receipt_some_failed() {
    let t = fixed_time();
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-some-fail".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![
            PackageReceipt {
                name: "core".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 2000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "cli".to_string(),
                version: "2.0.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "authorization denied".to_string(),
                },
                started_at: t,
                finished_at: t,
                duration_ms: 30000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    let json = serde_json::to_string_pretty(&receipt).expect("serialize");
    insta::assert_snapshot!("receipt_some_failed", json);
}

// ── Directory layout snapshot ───────────────────────────────────

#[test]
fn snapshot_directory_layout_after_full_save() {
    let t = fixed_time();
    let td = tempfile::tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut packages = BTreeMap::new();
    packages.insert(
        "demo@0.1.0".to_string(),
        PackageProgress {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: t,
        },
    );
    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-layout".to_string(),
        registry: Registry::crates_io(),
        created_at: t,
        updated_at: t,
        packages,
    };
    store.save_state(&state).expect("save state");

    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: "plan-layout".to_string(),
        registry: Registry::crates_io(),
        started_at: t,
        finished_at: t,
        packages: vec![PackageReceipt {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: t,
            finished_at: t,
            duration_ms: 1000,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
        event_log_path: PathBuf::from("events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.82.0".to_string()),
            rust_version: Some("1.82.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };
    store.save_receipt(&receipt).expect("save receipt");

    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: t,
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Collect file listing relative to state_dir
    let base = td.path();
    let mut files: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(base).expect("read_dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().expect("metadata");
        let size_hint = if meta.len() > 0 { ">0" } else { "0" };
        files.push(format!("{name} (size: {size_hint})"));
    }
    files.sort();
    let layout = files.join("\n");
    insta::assert_snapshot!("directory_layout_after_full_save", layout);
}

// ── Custom registry state snapshot ──────────────────────────────

#[test]
fn snapshot_state_with_custom_registry() {
    let t = fixed_time();
    let mut packages = BTreeMap::new();
    packages.insert(
        "my-lib@0.1.0".to_string(),
        PackageProgress {
            name: "my-lib".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: t,
        },
    );

    let state = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "plan-custom-reg".to_string(),
        registry: Registry {
            name: "my-private-registry".to_string(),
            api_base: "https://registry.internal.example.com/api/v1".to_string(),
            index_base: Some("https://index.internal.example.com/git/index".to_string()),
        },
        created_at: t,
        updated_at: t,
        packages,
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    insta::assert_snapshot!("state_with_custom_registry", json);
}
