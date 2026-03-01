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
use tempfile::tempdir;

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::events::EventLog;
use shipper::plan;
use shipper::state;
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
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

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
    let events_path = shipper::events::events_path(&state_dir);
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
    let mut lock1 = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire lock 1");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("check locked"));

    // Second attempt should fail
    let err =
        shipper::lock::LockFile::acquire(&state_dir, None).expect_err("second acquire should fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("lock already held"),
        "error should mention lock contention: {msg}"
    );

    // Release and re-acquire should succeed
    lock1.release().expect("release");
    assert!(!shipper::lock::LockFile::is_locked(&state_dir, None).expect("check unlocked"));

    let mut lock2 = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire lock 2");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("check locked again"));
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
        let mut lock = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire");
        lock.set_plan_id("stale-plan").expect("set plan_id");
        // Intentionally don't release — lock file remains but process holds it
        // For testing, we manually write a stale lock file
        lock.release().expect("release");
    }

    // Write a fake stale lock with a very old timestamp
    let lock_path = shipper::lock::lock_path(&state_dir, None);
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
    let mut lock =
        shipper::lock::LockFile::acquire_with_timeout(&state_dir, None, Duration::from_secs(60))
            .expect("acquire with timeout");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("locked"));
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

    let events_path = shipper::events::events_path(&state_dir);
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
    let mut lock = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire lock");
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

    let events_path = shipper::events::events_path(&state_dir);
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
    assert!(!shipper::lock::LockFile::is_locked(&state_dir, None).expect("check unlock"));

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
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

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
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

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
