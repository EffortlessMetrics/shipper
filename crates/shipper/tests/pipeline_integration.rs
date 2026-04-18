//! Pipeline integration tests for cross-module flows.
//!
//! Covers: config → plan → engine pipeline, state persistence → resume →
//! completion, error propagation across module boundaries, lock contention
//! scenarios, event logging through the full publish pipeline, and receipt
//! generation validation.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serial_test::serial;
use tempfile::tempdir;

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::plan;
use shipper::state::events::EventLog;
use shipper::state::execution_state as state;
use shipper::store::{FileStore, StateStore};
use shipper::types::{
    AttemptEvidence, EnvironmentFingerprint, ErrorClass, EventType, ExecutionResult,
    ExecutionState, Finishability, GitContext, PackageEvidence, PackageProgress, PackageReceipt,
    PackageState, PublishEvent, ReadinessEvidence, ReadinessMethod, Registry, ReleaseSpec,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn create_two_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "app"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("core/Cargo.toml"),
        r#"
[package]
name = "core"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("core/src/lib.rs"), "pub fn core_fn() {}\n");

    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core", version = "0.1.0" }
"#,
    );
    write_file(&root.join("app/src/lib.rs"), "pub fn app_fn() {}\n");
}

fn make_state(plan_id: &str, pkgs: &[(&str, &str, PackageState, u32)]) -> ExecutionState {
    let mut packages = BTreeMap::new();
    for &(name, version, ref st, attempts) in pkgs {
        packages.insert(
            format!("{name}@{version}"),
            PackageProgress {
                name: name.to_string(),
                version: version.to_string(),
                attempts,
                state: st.clone(),
                last_updated_at: Utc::now(),
            },
        );
    }
    ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

fn make_receipt(plan_id: &str, pkgs: &[(&str, &str, PackageState)]) -> shipper::types::Receipt {
    let packages = pkgs
        .iter()
        .map(|(name, version, st)| PackageReceipt {
            name: name.to_string(),
            version: version.to_string(),
            attempts: 1,
            state: st.clone(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 100,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        })
        .collect();

    shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages,
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    }
}

// ===========================================================================
// 1. Config → Plan → State → Receipt end-to-end pipeline
// ===========================================================================

#[test]
fn config_plan_state_receipt_end_to_end_pipeline() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    // Step 1: Write and load config
    write_file(
        &root.join(".shipper.toml"),
        &ShipperConfig::default_toml_template(),
    );
    let config = ShipperConfig::load_from_file(&root.join(".shipper.toml")).expect("load config");
    let opts = config.build_runtime_options(CliOverrides {
        max_attempts: Some(3),
        no_verify: true,
        ..Default::default()
    });
    assert_eq!(opts.max_attempts, 3);
    assert!(opts.no_verify);

    // Step 2: Build plan
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    assert_eq!(ws.plan.packages.len(), 2);

    // Step 3: Initialize state from plan
    let state_dir = root.join(".shipper");
    let mut packages = BTreeMap::new();
    for pkg in &ws.plan.packages {
        packages.insert(
            format!("{}@{}", pkg.name, pkg.version),
            PackageProgress {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let exec_state = ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    state::save_state(&state_dir, &exec_state).expect("save state");
    assert!(state::has_incomplete_state(&state_dir));

    // Step 4: Simulate publishing all packages
    let mut loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    for pkg in loaded.packages.values_mut() {
        pkg.state = PackageState::Published;
        pkg.attempts = 1;
        pkg.last_updated_at = Utc::now();
    }
    loaded.updated_at = Utc::now();
    state::save_state(&state_dir, &loaded).expect("save updated state");

    // Step 5: Write receipt
    let receipt = make_receipt(
        &ws.plan.plan_id,
        &[
            ("core", "0.1.0", PackageState::Published),
            ("app", "0.1.0", PackageState::Published),
        ],
    );
    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    assert!(!state::has_incomplete_state(&state_dir));

    // Step 6: Verify receipt
    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");
    assert_eq!(loaded_receipt.plan_id, ws.plan.plan_id);
    assert_eq!(loaded_receipt.packages.len(), 2);
    assert!(
        loaded_receipt
            .packages
            .iter()
            .all(|p| matches!(p.state, PackageState::Published))
    );
}

// ===========================================================================
// 2. State persistence → Resume with failed → Re-publish → Completion
// ===========================================================================

#[test]
fn state_resume_from_partial_failure_to_completion() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "resume-fail-complete";

    // Initial: core published, app failed
    let initial = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published, 1),
            (
                "app",
                "0.1.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "timeout".to_string(),
                },
                2,
            ),
        ],
    );
    state::save_state(&state_dir, &initial).expect("save initial");
    assert!(state::has_incomplete_state(&state_dir));

    // Resume: load and check which packages need work
    let mut loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.plan_id, plan_id);

    let failed: Vec<String> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Failed { .. }))
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(failed, vec!["app@0.1.0"]);

    // Retry the failed package
    if let Some(app) = loaded.packages.get_mut("app@0.1.0") {
        app.state = PackageState::Published;
        app.attempts = 3;
        app.last_updated_at = Utc::now();
    }
    loaded.updated_at = Utc::now();
    state::save_state(&state_dir, &loaded).expect("save resumed");

    // Write receipt and verify completion
    let receipt = make_receipt(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published),
            ("app", "0.1.0", PackageState::Published),
        ],
    );
    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    assert!(!state::has_incomplete_state(&state_dir));

    // Verify retry count was preserved
    let final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(final_state.packages["app@0.1.0"].attempts, 3);
}

// ===========================================================================
// 3. Error propagation: registry timeout → state reflects failure
// ===========================================================================

#[test]
fn registry_timeout_error_captured_in_state_and_events() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    // Spin up a mock server that returns 504 (gateway timeout)
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ = req
                .respond(tiny_http::Response::from_string("gateway timeout").with_status_code(504));
        }
    });

    let reg = Registry {
        name: "timeout-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper_core::registry::RegistryClient::new(reg).expect("client");

    // Registry check fails
    let err = client
        .version_exists("some-crate", "1.0.0")
        .expect_err("504 should error");
    let err_msg = format!("{err:#}");

    // Record this failure in state
    let exec_state = make_state(
        "timeout-plan",
        &[(
            "some-crate",
            "1.0.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: err_msg.clone(),
            },
            1,
        )],
    );
    state::save_state(&state_dir, &exec_state).expect("save state");

    // Record the failure as an event
    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: err_msg.clone(),
        },
        package: "some-crate@1.0.0".to_string(),
    });
    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write events");

    // Verify state persisted the error
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    if let PackageState::Failed { class, message } = &loaded.packages["some-crate@1.0.0"].state {
        assert_eq!(*class, ErrorClass::Retryable);
        assert!(message.contains("unexpected status") || message.contains("504"));
    } else {
        panic!("expected Failed state");
    }

    // Verify event was recorded
    let loaded_events = EventLog::read_from_file(&events_path).expect("read events");
    let pkg_events = loaded_events.events_for_package("some-crate@1.0.0");
    assert_eq!(pkg_events.len(), 1);

    handler.join().expect("handler thread");
}

// ===========================================================================
// 4. Lock contention: second acquire fails while first holds lock
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_contention_second_acquire_fails() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    // First process acquires the lock
    let mut lock1 =
        shipper_core::lock::LockFile::acquire(&state_dir, None).expect("acquire lock 1");
    assert!(shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("check locked"));

    // Second attempt should fail
    let err = shipper_core::lock::LockFile::acquire(&state_dir, None)
        .expect_err("second acquire should fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("lock already held"),
        "error should mention lock contention: {msg}"
    );

    // Release and re-acquire should succeed
    lock1.release().expect("release");
    assert!(!shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("check unlocked"));

    let mut lock2 =
        shipper_core::lock::LockFile::acquire(&state_dir, None).expect("acquire lock 2");
    assert!(shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("check locked again"));
    lock2.release().expect("release lock 2");
}

// ===========================================================================
// 5. Lock with stale timeout auto-cleanup
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_stale_timeout_allows_reacquire() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    // Create a lock then release the handle (drop releases it)
    {
        let mut lock = shipper_core::lock::LockFile::acquire(&state_dir, None).expect("acquire");
        lock.set_plan_id("stale-plan").expect("set plan_id");
        // Intentionally don't release — lock file remains but process holds it
        // For testing, we manually write a stale lock file
        lock.release().expect("release");
    }

    // Write a fake stale lock with a very old timestamp
    let lock_path = shipper_core::lock::lock_path(&state_dir, None);
    let stale_info = serde_json::json!({
        "pid": 99999,
        "hostname": "old-host",
        "acquired_at": "2020-01-01T00:00:00Z",
        "plan_id": "ancient-plan"
    });
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&stale_info).unwrap(),
    )
    .expect("write stale");

    // acquire_with_timeout should remove the stale lock and succeed
    let mut lock = shipper_core::lock::LockFile::acquire_with_timeout(
        &state_dir,
        None,
        Duration::from_secs(60),
    )
    .expect("acquire with timeout");
    assert!(shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("locked"));
    lock.release().expect("release");
}

// ===========================================================================
// 6. Event logging through full publish pipeline with all event types
// ===========================================================================

#[test]
fn event_log_complete_pipeline_with_all_phases() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::state::events::events_path(&state_dir);
    let plan_id = "pipeline-events-001";

    let mut log = EventLog::new();

    // Preflight phase
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightWorkspaceVerify {
            passed: true,
            output: "2 publishable crates".to_string(),
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightOwnershipCheck {
            crate_name: "core".to_string(),
            verified: true,
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: Finishability::Proven,
        },
        package: "all".to_string(),
    });

    // Plan + execution
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 2,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // Publish core: success with readiness
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "core".to_string(),
            version: "0.1.0".to_string(),
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p core".to_string(),
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 1000 },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll {
            attempt: 1,
            visible: true,
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessComplete {
            duration_ms: 200,
            attempts: 1,
        },
        package: "core@0.1.0".to_string(),
    });

    // Publish app: fail then skip
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "app".to_string(),
            version: "0.1.0".to_string(),
        },
        package: "app@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "auth failure".to_string(),
        },
        package: "app@0.1.0".to_string(),
    });

    // Execution finished with partial failure
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::PartialFailure,
        },
        package: "all".to_string(),
    });

    log.write_to_file(&events_path).expect("write events");
    let loaded = EventLog::read_from_file(&events_path).expect("read events");

    assert_eq!(loaded.all_events().len(), 15);

    // Verify per-package filtering
    let core_events = loaded.events_for_package("core@0.1.0");
    assert_eq!(core_events.len(), 7); // ownership + started + attempted + published + readiness×3

    let app_events = loaded.events_for_package("app@0.1.0");
    assert_eq!(app_events.len(), 2); // started + failed

    let global = loaded.events_for_package("all");
    assert_eq!(global.len(), 6); // preflight×3 + plan + exec_start + exec_finish
}

// ===========================================================================
// 7. Receipt with evidence (attempts + readiness checks) roundtrip
// ===========================================================================

#[test]
fn receipt_with_full_evidence_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "evidence-plan".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![PackageReceipt {
            name: "core".to_string(),
            version: "0.1.0".to_string(),
            attempts: 2,
            state: PackageState::Published,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 5000,
            evidence: PackageEvidence {
                attempts: vec![
                    AttemptEvidence {
                        attempt_number: 1,
                        command: "cargo publish -p core".to_string(),
                        exit_code: 1,
                        stdout_tail: "".to_string(),
                        stderr_tail: "error: rate limited".to_string(),
                        timestamp: Utc::now(),
                        duration: Duration::from_secs(3),
                    },
                    AttemptEvidence {
                        attempt_number: 2,
                        command: "cargo publish -p core".to_string(),
                        exit_code: 0,
                        stdout_tail: "Uploading core v0.1.0".to_string(),
                        stderr_tail: "".to_string(),
                        timestamp: Utc::now(),
                        duration: Duration::from_secs(2),
                    },
                ],
                readiness_checks: vec![
                    ReadinessEvidence {
                        attempt: 1,
                        visible: false,
                        timestamp: Utc::now(),
                        delay_before: Duration::from_secs(1),
                    },
                    ReadinessEvidence {
                        attempt: 2,
                        visible: true,
                        timestamp: Utc::now(),
                        delay_before: Duration::from_secs(2),
                    },
                ],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        }],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");

    // Verify evidence roundtrip
    assert_eq!(loaded.packages[0].evidence.attempts.len(), 2);
    assert_eq!(loaded.packages[0].evidence.attempts[0].exit_code, 1);
    assert_eq!(loaded.packages[0].evidence.attempts[1].exit_code, 0);
    assert_eq!(
        loaded.packages[0].evidence.attempts[1].stdout_tail,
        "Uploading core v0.1.0"
    );

    assert_eq!(loaded.packages[0].evidence.readiness_checks.len(), 2);
    assert!(!loaded.packages[0].evidence.readiness_checks[0].visible);
    assert!(loaded.packages[0].evidence.readiness_checks[1].visible);
}

// ===========================================================================
// 8. Receipt with git context roundtrip
// ===========================================================================

#[test]
fn receipt_with_git_context_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "git-ctx-plan".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: Some(GitContext {
            commit: Some("abc123def456".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v0.1.0".to_string()),
            dirty: Some(false),
        }),
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");

    let ctx = loaded.git_context.expect("git context should exist");
    assert_eq!(ctx.commit.as_deref(), Some("abc123def456"));
    assert_eq!(ctx.branch.as_deref(), Some("main"));
    assert_eq!(ctx.tag.as_deref(), Some("v0.1.0"));
    assert_eq!(ctx.dirty, Some(false));
}

// ===========================================================================
// 9. Lock → state → events → receipt: full publish simulation
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_state_events_receipt_full_simulation() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let plan_id = "full-sim-001";

    // Step 1: Acquire lock
    let mut lock = shipper_core::lock::LockFile::acquire(&state_dir, None).expect("acquire lock");
    lock.set_plan_id(plan_id).expect("set plan_id");

    // Step 2: Initialize state
    let initial_state = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Pending, 0),
            ("app", "0.1.0", PackageState::Pending, 0),
        ],
    );
    state::save_state(&state_dir, &initial_state).expect("save initial state");

    // Step 3: Record events during publish
    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 2,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // Simulate publishing both packages
    for name in &["core", "app"] {
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: name.to_string(),
                version: "0.1.0".to_string(),
            },
            package: format!("{name}@0.1.0"),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 500 },
            package: format!("{name}@0.1.0"),
        });
    }

    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write events");

    // Step 4: Update state to all published
    let mut final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    for pkg in final_state.packages.values_mut() {
        pkg.state = PackageState::Published;
        pkg.attempts = 1;
        pkg.last_updated_at = Utc::now();
    }
    final_state.updated_at = Utc::now();
    state::save_state(&state_dir, &final_state).expect("save final state");

    // Step 5: Write receipt
    let receipt = make_receipt(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published),
            ("app", "0.1.0", PackageState::Published),
        ],
    );
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Step 6: Release lock
    lock.release().expect("release lock");

    // Verify everything is consistent
    assert!(!state::has_incomplete_state(&state_dir));
    assert!(!shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("check unlock"));

    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");
    assert_eq!(loaded_receipt.plan_id, plan_id);
    assert_eq!(loaded_receipt.packages.len(), 2);

    let loaded_events = EventLog::read_from_file(&events_path).expect("read events");
    assert_eq!(loaded_events.all_events().len(), 7); // plan + exec_start + 2*(start+publish) + exec_finish
}

// ===========================================================================
// 10. Config → Plan → Registry check pipeline with mock
// ===========================================================================

#[test]
fn config_plan_registry_check_pipeline() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    // Load config
    write_file(
        &root.join(".shipper.toml"),
        &ShipperConfig::default_toml_template(),
    );
    let config = ShipperConfig::load_from_file(&root.join(".shipper.toml")).expect("load config");
    config.validate().expect("config valid");

    // Build plan
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Mock registry: core already published, app not
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        for _ in 0..ws.plan.packages.len() {
            if let Ok(req) = server.recv() {
                let url = req.url().to_string();
                if url.contains("/core/") {
                    let body = r#"{"version":{"num":"0.1.0"}}"#;
                    let resp = tiny_http::Response::from_string(body).with_status_code(200);
                    let _ = req.respond(resp);
                } else {
                    let _ = req.respond(
                        tiny_http::Response::from_string("not found").with_status_code(404),
                    );
                }
            }
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper_core::registry::RegistryClient::new(reg).expect("client");

    // Re-build plan for iteration (handler consumed the ws)
    let ws2 = plan::build_plan(&spec).expect("build plan 2");

    let mut already_published = vec![];
    let mut needs_publish = vec![];
    for pkg in &ws2.plan.packages {
        let exists = client
            .version_exists(&pkg.name, &pkg.version)
            .expect("check");
        if exists {
            already_published.push(pkg.name.as_str());
        } else {
            needs_publish.push(pkg.name.as_str());
        }
    }

    assert_eq!(already_published, vec!["core"]);
    assert_eq!(needs_publish, vec!["app"]);

    handler.join().expect("handler thread");
}

// ===========================================================================
// 11. FileStore: state + events + receipt lifecycle through store trait
// ===========================================================================

#[test]
fn file_store_state_events_receipt_lifecycle() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());
    let plan_id = "store-lifecycle-001";

    // Save state
    let exec_state = make_state(
        plan_id,
        &[
            ("alpha", "1.0.0", PackageState::Published, 1),
            ("beta", "1.0.0", PackageState::Pending, 0),
        ],
    );
    store.save_state(&exec_state).expect("save state");

    // Save events
    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 2,
        },
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "alpha@1.0.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 300 },
        package: "alpha@1.0.0".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Save receipt
    let receipt = make_receipt(plan_id, &[("alpha", "1.0.0", PackageState::Published)]);
    store.save_receipt(&receipt).expect("save receipt");

    // Load everything and cross-validate
    let loaded_state = store.load_state().expect("load state").expect("exists");
    let loaded_events = store.load_events().expect("load events").expect("exists");
    let loaded_receipt = store.load_receipt().expect("load receipt").expect("exists");

    // Plan IDs should match across all artifacts
    assert_eq!(loaded_state.plan_id, plan_id);
    assert_eq!(loaded_receipt.plan_id, plan_id);

    // Events should reference the same plan
    let plan_event = loaded_events
        .all_events()
        .iter()
        .find(|e| matches!(e.event_type, EventType::PlanCreated { .. }))
        .expect("plan event exists");
    if let EventType::PlanCreated {
        plan_id: ref pid, ..
    } = plan_event.event_type
    {
        assert_eq!(pid, plan_id);
    }

    // State package count should be consistent with plan
    assert_eq!(loaded_state.packages.len(), 2);
    assert_eq!(loaded_events.all_events().len(), 3);
}

// ===========================================================================
// 12. Ambiguous package state → Resume → Resolution
// ===========================================================================

#[test]
fn ambiguous_state_resume_and_resolution() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "ambiguous-resume";

    // State has an ambiguous package (publish may or may not have succeeded)
    let initial = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published, 1),
            (
                "app",
                "0.1.0",
                PackageState::Ambiguous {
                    message: "timeout during cargo publish".to_string(),
                },
                1,
            ),
        ],
    );
    state::save_state(&state_dir, &initial).expect("save initial");
    assert!(state::has_incomplete_state(&state_dir));

    // Load and verify the ambiguous state is preserved
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    let app = &loaded.packages["app@0.1.0"];
    assert!(
        matches!(app.state, PackageState::Ambiguous { .. }),
        "app should be Ambiguous"
    );

    // Mock registry says it's actually published → resolve as Published
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let body = r#"{"version":{"num":"0.1.0"}}"#;
            let resp = tiny_http::Response::from_string(body).with_status_code(200);
            let _ = req.respond(resp);
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper_core::registry::RegistryClient::new(reg).expect("client");

    let exists = client.version_exists("app", "0.1.0").expect("check");
    assert!(exists, "registry says app is published");

    // Resolve ambiguous → published in state
    let mut resolved = loaded;
    if let Some(app) = resolved.packages.get_mut("app@0.1.0") {
        app.state = PackageState::Published;
        app.last_updated_at = Utc::now();
    }
    resolved.updated_at = Utc::now();
    state::save_state(&state_dir, &resolved).expect("save resolved");

    // Write receipt
    let receipt = make_receipt(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published),
            ("app", "0.1.0", PackageState::Published),
        ],
    );
    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    assert!(!state::has_incomplete_state(&state_dir));

    handler.join().expect("handler thread");
}

// ===========================================================================
// 13. Config validation rejects contradictory settings
// ===========================================================================

#[test]
fn config_validation_rejects_bad_retry_settings() {
    let td = tempdir().expect("tempdir");

    // max_delay < base_delay should fail validation
    let bad_retry = r#"
[retry]
base_delay = "30s"
max_delay = "1s"
"#;
    let path = td.path().join("bad_retry.toml");
    fs::write(&path, bad_retry).expect("write config");

    let result = ShipperConfig::load_from_file(&path);
    if let Ok(cfg) = result {
        assert!(
            cfg.validate().is_err(),
            "max_delay < base_delay should fail validation"
        );
    }

    // Negative max_attempts should fail at parse or validation
    let bad_attempts = "[retry]\nmax_attempts = -1\n";
    let path2 = td.path().join("bad_attempts.toml");
    fs::write(&path2, bad_attempts).expect("write config");
    assert!(
        ShipperConfig::load_from_file(&path2).is_err(),
        "negative max_attempts should fail"
    );
}

// ===========================================================================
// 14. Plan id changes when package versions differ
// ===========================================================================

#[test]
fn plan_id_changes_when_version_differs() {
    let td = tempdir().expect("tempdir");
    let root = td.path();

    // Create workspace with version 0.1.0
    create_two_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws1 = plan::build_plan(&spec).expect("plan v1");

    // Bump the version
    write_file(
        &root.join("core/Cargo.toml"),
        r#"
[package]
name = "core"
version = "0.2.0"
edition = "2021"
"#,
    );
    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.2.0"
edition = "2021"

[dependencies]
core = { path = "../core", version = "0.2.0" }
"#,
    );

    let ws2 = plan::build_plan(&spec).expect("plan v2");

    assert_ne!(
        ws1.plan.plan_id, ws2.plan.plan_id,
        "plan_id should change when versions change"
    );
}

// ===========================================================================
// 15. Full pipeline: plan → preflight events → publish → verify → receipt
// ===========================================================================

#[test]
fn full_pipeline_plan_preflight_publish_verify_receipt() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    // Plan
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let plan_id = &ws.plan.plan_id;
    assert!(!plan_id.is_empty());

    let state_dir = root.join(".shipper");

    // Init state
    let mut packages = BTreeMap::new();
    for pkg in &ws.plan.packages {
        packages.insert(
            format!("{}@{}", pkg.name, pkg.version),
            PackageProgress {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let exec_state = ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    state::save_state(&state_dir, &exec_state).expect("save state");

    // Record preflight + publish + readiness events
    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: Finishability::Proven,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // Simulate publish + readiness for each package
    for pkg in &ws.plan.packages {
        let key = format!("{}@{}", pkg.name, pkg.version);
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
            },
            package: key.clone(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 200 },
            package: key.clone(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessStarted {
                method: ReadinessMethod::Api,
            },
            package: key.clone(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessComplete {
                duration_ms: 100,
                attempts: 1,
            },
            package: key.clone(),
        });
    }

    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write events");

    // Update state → all published
    let mut loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    for pkg in loaded.packages.values_mut() {
        pkg.state = PackageState::Published;
        pkg.attempts = 1;
    }
    state::save_state(&state_dir, &loaded).expect("save updated");

    // Write receipt
    let pkg_receipts: Vec<PackageReceipt> = ws
        .plan
        .packages
        .iter()
        .map(|p| PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: 1,
            state: PackageState::Published,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 200,
            evidence: PackageEvidence {
                attempts: vec![AttemptEvidence {
                    attempt_number: 1,
                    command: format!("cargo publish -p {}", p.name),
                    exit_code: 0,
                    stdout_tail: format!("Uploading {} v{}", p.name, p.version),
                    stderr_tail: String::new(),
                    timestamp: Utc::now(),
                    duration: Duration::from_millis(200),
                }],
                readiness_checks: vec![ReadinessEvidence {
                    attempt: 1,
                    visible: true,
                    timestamp: Utc::now(),
                    delay_before: Duration::from_secs(1),
                }],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        })
        .collect();

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.clone(),
        registry: ws.plan.registry.clone(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: pkg_receipts,
        event_log_path: events_path.clone(),
        git_context: Some(GitContext {
            commit: Some("deadbeef".to_string()),
            branch: Some("main".to_string()),
            tag: None,
            dirty: Some(false),
        }),
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    };
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Verify completeness
    assert!(!state::has_incomplete_state(&state_dir));

    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("exists");
    assert_eq!(loaded_receipt.plan_id, *plan_id);
    assert_eq!(loaded_receipt.packages.len(), 2);
    for pr in &loaded_receipt.packages {
        assert!(matches!(pr.state, PackageState::Published));
        assert_eq!(pr.evidence.attempts.len(), 1);
        assert_eq!(pr.evidence.readiness_checks.len(), 1);
        assert!(pr.evidence.readiness_checks[0].visible);
    }

    let loaded_events = EventLog::read_from_file(&events_path).expect("read events");
    // 2 preflight + plan + exec_start + 2*(start+publish+readiness_start+readiness_complete) + exec_finish = 13
    assert_eq!(loaded_events.all_events().len(), 13);
}

// ===========================================================================
// 16. Error propagation: permanent failure halts pipeline
// ===========================================================================

#[test]
fn permanent_failure_recorded_in_state_and_events() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let plan_id = "perm-fail-plan";

    // core publishes, app hits permanent error (auth)
    let exec_state = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published, 1),
            (
                "app",
                "0.1.0",
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "403 forbidden: invalid token".to_string(),
                },
                1,
            ),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save state");

    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 500 },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "403 forbidden: invalid token".to_string(),
        },
        package: "app@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::PartialFailure,
        },
        package: "all".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write events");

    // Verify state
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    let app = &loaded.packages["app@0.1.0"];
    assert!(matches!(
        app.state,
        PackageState::Failed {
            class: ErrorClass::Permanent,
            ..
        }
    ));
    assert_eq!(app.attempts, 1);

    // Verify events
    let loaded_events = EventLog::read_from_file(&events_path).expect("read events");
    let fail_events: Vec<_> = loaded_events
        .events_for_package("app@0.1.0")
        .into_iter()
        .filter(|e| matches!(e.event_type, EventType::PackageFailed { .. }))
        .collect();
    assert_eq!(fail_events.len(), 1);

    // Verify execution result is partial failure
    let finish = loaded_events
        .events_for_package("all")
        .into_iter()
        .find(|e| matches!(e.event_type, EventType::ExecutionFinished { .. }))
        .expect("finish event");
    if let EventType::ExecutionFinished { ref result } = finish.event_type {
        assert_eq!(*result, ExecutionResult::PartialFailure);
    }
}

// ===========================================================================
// 17. State persistence and resume across simulated interruptions
// ===========================================================================

#[test]
fn resume_from_uploaded_state_after_interruption() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "interrupt-resume";

    // Simulate: core fully published, app uploaded (cargo publish succeeded
    // but readiness not yet verified — simulates interruption after upload)
    let initial = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published, 1),
            ("app", "0.1.0", PackageState::Uploaded, 1),
        ],
    );
    state::save_state(&state_dir, &initial).expect("save initial");
    assert!(state::has_incomplete_state(&state_dir));

    // Resume: load and identify packages needing work
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    let needs_work: Vec<String> = loaded
        .packages
        .values()
        .filter(|p| !matches!(p.state, PackageState::Published))
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(needs_work, vec!["app@0.1.0"]);

    // Uploaded should transition to Published after readiness check
    let app = &loaded.packages["app@0.1.0"];
    assert!(matches!(app.state, PackageState::Uploaded));

    // Complete the resume
    let mut resumed = loaded;
    if let Some(app) = resumed.packages.get_mut("app@0.1.0") {
        app.state = PackageState::Published;
        app.last_updated_at = Utc::now();
    }
    resumed.updated_at = Utc::now();
    state::save_state(&state_dir, &resumed).expect("save resumed");

    // Write receipt and verify
    let receipt = make_receipt(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published),
            ("app", "0.1.0", PackageState::Published),
        ],
    );
    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    assert!(!state::has_incomplete_state(&state_dir));
}

// ===========================================================================
// 18. Multiple resumptions with incrementing attempt counts
// ===========================================================================

#[test]
fn multiple_resume_cycles_preserve_attempt_counts() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    let plan_id = "multi-resume";

    // Round 1: app fails on first attempt
    let s1 = make_state(
        plan_id,
        &[
            ("core", "0.1.0", PackageState::Published, 1),
            (
                "app",
                "0.1.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "rate limited".to_string(),
                },
                1,
            ),
        ],
    );
    state::save_state(&state_dir, &s1).expect("save r1");

    // Round 2: load, bump attempts, still fails
    let mut s2 = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    if let Some(app) = s2.packages.get_mut("app@0.1.0") {
        app.attempts = 2;
        app.state = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "rate limited again".to_string(),
        };
        app.last_updated_at = Utc::now();
    }
    state::save_state(&state_dir, &s2).expect("save r2");

    // Round 3: finally succeeds
    let mut s3 = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(s3.packages["app@0.1.0"].attempts, 2);
    if let Some(app) = s3.packages.get_mut("app@0.1.0") {
        app.attempts = 3;
        app.state = PackageState::Published;
        app.last_updated_at = Utc::now();
    }
    state::save_state(&state_dir, &s3).expect("save r3");

    let final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(final_state.packages["app@0.1.0"].attempts, 3);
    assert!(matches!(
        final_state.packages["app@0.1.0"].state,
        PackageState::Published
    ));
}

// ===========================================================================
// 19. Event log ordering: timestamps are non-decreasing
// ===========================================================================

#[test]
fn event_log_timestamps_are_non_decreasing() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let mut log = EventLog::new();

    let event_types = [
        EventType::PreflightStarted,
        EventType::PreflightComplete {
            finishability: Finishability::Proven,
        },
        EventType::PlanCreated {
            plan_id: "ts-plan".to_string(),
            package_count: 1,
        },
        EventType::ExecutionStarted,
        EventType::PackageStarted {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
        },
        EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p alpha".to_string(),
        },
        EventType::PackagePublished { duration_ms: 100 },
        EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        EventType::ReadinessComplete {
            duration_ms: 50,
            attempts: 1,
        },
        EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
    ];

    for et in event_types {
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: et,
            package: "alpha@1.0.0".to_string(),
        });
    }

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write");

    let loaded = EventLog::read_from_file(&events_path).expect("read");
    let events = loaded.all_events();
    for window in events.windows(2) {
        assert!(
            window[1].timestamp >= window[0].timestamp,
            "events should be in non-decreasing timestamp order"
        );
    }
}

// ===========================================================================
// 20. Receipt fields populated correctly for mixed success/failure
// ===========================================================================

#[test]
fn receipt_fields_populated_for_mixed_outcomes() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    let plan_id = "mixed-outcome-receipt";

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![
            PackageReceipt {
                name: "core".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 300,
                evidence: PackageEvidence {
                    attempts: vec![AttemptEvidence {
                        attempt_number: 1,
                        command: "cargo publish -p core".to_string(),
                        exit_code: 0,
                        stdout_tail: "Uploading core v0.1.0".to_string(),
                        stderr_tail: "".to_string(),
                        timestamp: Utc::now(),
                        duration: Duration::from_millis(300),
                    }],
                    readiness_checks: vec![ReadinessEvidence {
                        attempt: 1,
                        visible: true,
                        timestamp: Utc::now(),
                        delay_before: Duration::from_secs(1),
                    }],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
            PackageReceipt {
                name: "app".to_string(),
                version: "0.1.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "version already exists".to_string(),
                },
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 9000,
                evidence: PackageEvidence {
                    attempts: vec![
                        AttemptEvidence {
                            attempt_number: 1,
                            command: "cargo publish -p app".to_string(),
                            exit_code: 1,
                            stdout_tail: "".to_string(),
                            stderr_tail: "version already exists".to_string(),
                            timestamp: Utc::now(),
                            duration: Duration::from_secs(3),
                        },
                        AttemptEvidence {
                            attempt_number: 2,
                            command: "cargo publish -p app".to_string(),
                            exit_code: 1,
                            stdout_tail: "".to_string(),
                            stderr_tail: "version already exists".to_string(),
                            timestamp: Utc::now(),
                            duration: Duration::from_secs(3),
                        },
                        AttemptEvidence {
                            attempt_number: 3,
                            command: "cargo publish -p app".to_string(),
                            exit_code: 1,
                            stdout_tail: "".to_string(),
                            stderr_tail: "version already exists".to_string(),
                            timestamp: Utc::now(),
                            duration: Duration::from_secs(3),
                        },
                    ],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
        ],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "test".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("exists");

    // Verify published package
    let core = &loaded.packages[0];
    assert_eq!(core.name, "core");
    assert!(matches!(core.state, PackageState::Published));
    assert_eq!(core.attempts, 1);
    assert_eq!(core.evidence.attempts[0].exit_code, 0);

    // Verify failed package
    let app = &loaded.packages[1];
    assert_eq!(app.name, "app");
    assert!(matches!(
        app.state,
        PackageState::Failed {
            class: ErrorClass::Permanent,
            ..
        }
    ));
    assert_eq!(app.attempts, 3);
    assert_eq!(app.evidence.attempts.len(), 3);
    assert!(app.evidence.readiness_checks.is_empty());
    assert_eq!(app.duration_ms, 9000);
}

// ===========================================================================
// 21. Lock acquire → publish state → release lifecycle
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_lifecycle_around_state_operations() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let plan_id = "lock-lifecycle-test";

    // Acquire lock
    let mut lock = shipper_core::lock::LockFile::acquire(&state_dir, None).expect("acquire");
    lock.set_plan_id(plan_id).expect("set plan_id");

    // Verify lock info
    let info =
        shipper_core::lock::LockFile::read_lock_info(&state_dir, None).expect("read lock info");
    assert_eq!(info.plan_id.as_deref(), Some(plan_id));
    assert!(info.pid > 0);
    assert!(!info.hostname.is_empty());

    // Perform state operations while locked
    let exec_state = make_state(plan_id, &[("alpha", "1.0.0", PackageState::Pending, 0)]);
    state::save_state(&state_dir, &exec_state).expect("save state while locked");

    // Release lock
    lock.release().expect("release");
    assert!(!shipper_core::lock::LockFile::is_locked(&state_dir, None).expect("check unlocked"));

    // State should persist after lock release
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.plan_id, plan_id);
}

// ===========================================================================
// 22. Event log: clear and re-record
// ===========================================================================

#[test]
fn event_log_clear_and_rerecord() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    let events_path = shipper::state::events::events_path(&state_dir);

    // First batch
    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    log.write_to_file(&events_path).expect("write first");
    assert_eq!(log.all_events().len(), 1);

    // Clear in-memory log
    log.clear();
    assert!(log.all_events().is_empty());

    // Remove file to start fresh (write_to_file appends)
    fs::remove_file(&events_path).expect("remove events file");

    // Second batch
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: "new-plan".to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    log.write_to_file(&events_path).expect("write second");

    // File has only the second batch
    let loaded = EventLog::read_from_file(&events_path).expect("read");
    assert_eq!(loaded.all_events().len(), 2);
    assert!(matches!(
        loaded.all_events()[0].event_type,
        EventType::PlanCreated { .. }
    ));
}

// ===========================================================================
// 23. State clear removes state but not receipt
// ===========================================================================

#[test]
fn clear_state_removes_state_file_only() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "clear-test";

    // Save both state and receipt
    let exec_state = make_state(plan_id, &[("a", "1.0.0", PackageState::Published, 1)]);
    state::save_state(&state_dir, &exec_state).expect("save state");
    let receipt = make_receipt(plan_id, &[("a", "1.0.0", PackageState::Published)]);
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Both exist
    assert!(state::state_path(&state_dir).exists());
    assert!(state::receipt_path(&state_dir).exists());

    // Clear state
    state::clear_state(&state_dir).expect("clear state");

    // State gone, receipt remains
    assert!(!state::state_path(&state_dir).exists());
    assert!(state::receipt_path(&state_dir).exists());

    // load_state returns None
    let loaded = state::load_state(&state_dir).expect("load state");
    assert!(loaded.is_none());
}

// ===========================================================================
// 24. FileStore roundtrip with clear
// ===========================================================================

#[test]
fn file_store_clear_removes_all_artifacts() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());
    let plan_id = "store-clear-test";

    // Save all three artifacts
    let exec_state = make_state(plan_id, &[("x", "1.0.0", PackageState::Published, 1)]);
    store.save_state(&exec_state).expect("save state");

    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    let receipt = make_receipt(plan_id, &[("x", "1.0.0", PackageState::Published)]);
    store.save_receipt(&receipt).expect("save receipt");

    // All exist
    assert!(store.load_state().expect("load").is_some());
    assert!(store.load_events().expect("load").is_some());
    assert!(store.load_receipt().expect("load").is_some());

    // Clear
    store.clear().expect("clear");

    // All gone
    assert!(store.load_state().expect("load").is_none());
    assert!(store.load_events().expect("load").is_none());
    assert!(store.load_receipt().expect("load").is_none());
}

// ===========================================================================
// 25. Plan → levels grouping for parallel engine
// ===========================================================================

#[test]
fn plan_levels_grouping_reflects_dependencies() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    let levels = ws.plan.group_by_levels();
    assert!(
        !levels.is_empty(),
        "should have at least one level for parallel publishing"
    );

    // core should be at a lower level than app (since app depends on core)
    let core_level = levels
        .iter()
        .find(|l| l.packages.iter().any(|p| p.name == "core"))
        .expect("core level");
    let app_level = levels
        .iter()
        .find(|l| l.packages.iter().any(|p| p.name == "app"))
        .expect("app level");
    assert!(
        core_level.level <= app_level.level,
        "core should be at same or lower level than app"
    );
}

// ===========================================================================
// 26. Skipped package state roundtrips through state/receipt
// ===========================================================================

#[test]
fn skipped_package_state_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    let plan_id = "skip-roundtrip";

    let exec_state = make_state(
        plan_id,
        &[(
            "old-crate",
            "1.0.0",
            PackageState::Skipped {
                reason: "already published".to_string(),
            },
            0,
        )],
    );
    state::save_state(&state_dir, &exec_state).expect("save");

    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    if let PackageState::Skipped { ref reason } = loaded.packages["old-crate@1.0.0"].state {
        assert_eq!(reason, "already published");
    } else {
        panic!("expected Skipped state");
    }
}

// ===========================================================================
// 27. Event log: package-scoped filtering correctness
// ===========================================================================

#[test]
fn event_log_package_filtering_is_exact() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let mut log = EventLog::new();

    // Events for similar-named packages
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "core".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "core@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "core-utils".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "core-utils@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 100 },
        package: "core@1.0.0".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write");

    let loaded = EventLog::read_from_file(&events_path).expect("read");

    // "core@1.0.0" should NOT match "core-utils@1.0.0"
    let core_events = loaded.events_for_package("core@1.0.0");
    assert_eq!(core_events.len(), 2);
    assert!(core_events.iter().all(|e| e.package == "core@1.0.0"));

    let utils_events = loaded.events_for_package("core-utils@1.0.0");
    assert_eq!(utils_events.len(), 1);
}

// ===========================================================================
// 28. Registry 404 → mark as new crate (not published)
// ===========================================================================

#[test]
fn registry_404_means_not_published() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ =
                req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    });

    let reg = Registry {
        name: "test-404".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper_core::registry::RegistryClient::new(reg).expect("client");
    let exists = client
        .version_exists("brand-new-crate", "0.1.0")
        .expect("check");
    assert!(!exists, "404 should mean crate is not published");

    handler.join().expect("handler thread");
}

// ===========================================================================
// 29. State version string preserved through roundtrip
// ===========================================================================

#[test]
fn state_version_preserved_through_roundtrip() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let exec_state = make_state("version-test", &[("a", "1.0.0", PackageState::Pending, 0)]);
    state::save_state(&state_dir, &exec_state).expect("save");

    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.state_version, state::CURRENT_STATE_VERSION);
}

// ===========================================================================
// 30. Config overrides applied correctly to runtime options
// ===========================================================================

#[test]
#[serial]
fn config_with_cli_overrides_produces_correct_options() {
    let td = tempdir().expect("tempdir");
    let root = td.path();

    write_file(
        &root.join(".shipper.toml"),
        &ShipperConfig::default_toml_template(),
    );
    let config = ShipperConfig::load_from_file(&root.join(".shipper.toml")).expect("load config");

    let opts = config.build_runtime_options(CliOverrides {
        max_attempts: Some(5),
        no_verify: true,
        allow_dirty: true,
        skip_ownership_check: true,
        ..Default::default()
    });

    assert_eq!(opts.max_attempts, 5);
    assert!(opts.no_verify);
    assert!(opts.allow_dirty);
    assert!(opts.skip_ownership_check);
}

// ===========================================================================
// 31. Receipt version is current
// ===========================================================================

#[test]
fn receipt_version_is_current_version() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    let plan_id = "version-receipt";

    let receipt = make_receipt(plan_id, &[("a", "1.0.0", PackageState::Published)]);
    state::write_receipt(&state_dir, &receipt).expect("write");

    let loaded = state::load_receipt(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.receipt_version, state::CURRENT_RECEIPT_VERSION);
}

// ===========================================================================
// 32. Complete failure result in events
// ===========================================================================

#[test]
fn complete_failure_recorded_in_events() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "auth expired".to_string(),
        },
        package: "only-crate@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::CompleteFailure,
        },
        package: "all".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write");

    let loaded = EventLog::read_from_file(&events_path).expect("read");
    let finish = loaded
        .all_events()
        .iter()
        .find(|e| matches!(e.event_type, EventType::ExecutionFinished { .. }))
        .expect("finish event");
    if let EventType::ExecutionFinished { ref result } = finish.event_type {
        assert_eq!(*result, ExecutionResult::CompleteFailure);
    }
}

// ===========================================================================
// 33. Readiness timeout event recorded
// ===========================================================================

#[test]
fn readiness_timeout_event_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let mut log = EventLog::new();
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        package: "slow-crate@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll {
            attempt: 1,
            visible: false,
        },
        package: "slow-crate@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll {
            attempt: 2,
            visible: false,
        },
        package: "slow-crate@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessTimeout { max_wait_ms: 60000 },
        package: "slow-crate@1.0.0".to_string(),
    });

    let events_path = shipper::state::events::events_path(&state_dir);
    log.write_to_file(&events_path).expect("write");

    let loaded = EventLog::read_from_file(&events_path).expect("read");
    let pkg_events = loaded.events_for_package("slow-crate@1.0.0");
    assert_eq!(pkg_events.len(), 4);

    let timeout = pkg_events
        .iter()
        .find(|e| matches!(e.event_type, EventType::ReadinessTimeout { .. }))
        .expect("timeout event");
    if let EventType::ReadinessTimeout { max_wait_ms } = &timeout.event_type {
        assert_eq!(*max_wait_ms, 60000);
    }
}

// ===========================================================================
// 34. Plan with package selection filters correctly
// ===========================================================================

#[test]
fn plan_with_package_selection_filters_to_selected() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: Some(vec!["core".to_string()]),
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    assert_eq!(ws.plan.packages.len(), 1);
    assert_eq!(ws.plan.packages[0].name, "core");
}
