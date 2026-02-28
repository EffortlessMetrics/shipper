//! Cross-crate integration tests verifying that shipper's public modules
//! compose correctly: config → plan, plan → state, auth → registry,
//! state → store → events, and full preflight flows with a mocked registry.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tempfile::tempdir;

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::events::EventLog;
use shipper::plan;
use shipper::state;
use shipper::store::{FileStore, StateStore};
use shipper::types::{
    EnvironmentFingerprint, ErrorClass, EventType, ExecutionResult, ExecutionState,
    PackageEvidence, PackageProgress, PackageReceipt, PackageState, PublishEvent, ReadinessMethod,
    Registry, ReleaseSpec,
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

/// Create a minimal Cargo workspace with two crates (`core` depends on nothing,
/// `app` depends on `core`).
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

fn sample_state(plan_id: &str) -> ExecutionState {
    let mut packages = BTreeMap::new();
    packages.insert(
        "core@0.1.0".to_string(),
        PackageProgress {
            name: "core".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "app@0.1.0".to_string(),
        PackageProgress {
            name: "app".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );

    ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

fn sample_receipt(plan_id: &str) -> shipper::types::Receipt {
    shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![PackageReceipt {
            name: "core".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 42,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }],
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

// ===========================================================================
// 1. Config loading → plan building flow
// ===========================================================================

#[test]
fn config_load_then_build_plan() {
    let td = tempdir().expect("tempdir");
    let root = td.path();

    // Write a .shipper.toml with non-default retry
    let template = ShipperConfig::default_toml_template();
    write_file(&root.join(".shipper.toml"), &template);

    // Create a workspace
    create_two_crate_workspace(root);

    // Load config and merge with CLI overrides
    let config = ShipperConfig::load_from_file(&root.join(".shipper.toml")).expect("load config");
    let opts = config.build_runtime_options(CliOverrides {
        output_lines: Some(256),
        ..Default::default()
    });

    // Build plan from the same workspace
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Plan should list the two packages in dependency order
    assert_eq!(ws.plan.packages.len(), 2);
    assert_eq!(ws.plan.packages[0].name, "core");
    assert_eq!(ws.plan.packages[1].name, "app");

    // CLI override should have taken effect
    assert_eq!(opts.output_lines, 256);
}

// ===========================================================================
// 2. Plan building → state persistence flow
// ===========================================================================

#[test]
fn plan_build_then_persist_state() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Construct execution state from the plan
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

    // Persist to disk
    let state_dir = root.join(".shipper");
    state::save_state(&state_dir, &exec_state).expect("save state");

    // Reload and verify
    let loaded = state::load_state(&state_dir)
        .expect("load state")
        .expect("state exists");

    assert_eq!(loaded.plan_id, ws.plan.plan_id);
    assert_eq!(loaded.packages.len(), 2);
    assert!(loaded.packages.contains_key("core@0.1.0"));
    assert!(loaded.packages.contains_key("app@0.1.0"));
}

// ===========================================================================
// 3. Auth resolution → registry checking flow (mocked HTTP)
// ===========================================================================

#[test]
fn auth_resolve_then_registry_version_check() {
    // Spin up a tiny HTTP server that pretends to be a registry
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start mock server");
    let addr = server.server_addr().to_ip().expect("server addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    // Spawn a handler that responds to /api/v1/crates/core/0.1.0
    let api_base_clone = api_base.clone();
    let handler = std::thread::spawn(move || {
        let _base = api_base_clone;
        if let Ok(req) = server.recv() {
            let url = req.url().to_string();
            if url.contains("/api/v1/crates/core/0.1.0") {
                let response = tiny_http::Response::from_string(r#"{"version":{"num":"0.1.0"}}"#)
                    .with_status_code(200);
                let _ = req.respond(response);
            } else {
                let response = tiny_http::Response::from_string("not found").with_status_code(404);
                let _ = req.respond(response);
            }
        }
    });

    // Build a RegistryClient pointing at the mock
    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };

    let client = shipper::registry::RegistryClient::new(reg).expect("build registry client");

    // Check version exists
    let exists = client
        .version_exists("core", "0.1.0")
        .expect("version check");
    assert!(exists);

    handler.join().expect("handler thread");
}

#[test]
fn registry_reports_missing_version() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start mock server");
    let addr = server.server_addr().to_ip().expect("server addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let response = tiny_http::Response::from_string("not found").with_status_code(404);
            let _ = req.respond(response);
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("build registry client");

    let exists = client
        .version_exists("nonexistent", "9.9.9")
        .expect("version check");
    assert!(!exists);

    handler.join().expect("handler thread");
}

// ===========================================================================
// 4. Full flow: config → plan → preflight (mocked registry)
// ===========================================================================

#[test]
fn config_to_plan_to_registry_version_check() {
    let td = tempdir().expect("tempdir");
    let root = td.path();

    // Write config
    write_file(
        &root.join(".shipper.toml"),
        &ShipperConfig::default_toml_template(),
    );

    // Create workspace
    create_two_crate_workspace(root);

    // Load config
    let config = ShipperConfig::load_from_file(&root.join(".shipper.toml")).expect("load config");
    let _opts = config.build_runtime_options(CliOverrides::default());

    // Build plan
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Mock registry: respond 404 for each package (not yet published)
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start mock server");
    let addr = server.server_addr().to_ip().expect("server addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let expected_count = ws.plan.packages.len();
    let handler = std::thread::spawn(move || {
        for _ in 0..expected_count {
            if let Ok(req) = server.recv() {
                let response = tiny_http::Response::from_string("not found").with_status_code(404);
                let _ = req.respond(response);
            }
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("build registry client");

    // Verify none of the planned packages are published yet
    for pkg in &ws.plan.packages {
        let exists = client
            .version_exists(&pkg.name, &pkg.version)
            .expect("version check");
        assert!(!exists, "{} should not be published yet", pkg.name);
    }

    handler.join().expect("handler thread");
}

// ===========================================================================
// 5. State save → reload → resume verification
// ===========================================================================

#[test]
fn state_save_reload_resume_verification() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "test-plan-abc";

    // Save state with one published and one pending package
    let exec_state = sample_state(plan_id);
    state::save_state(&state_dir, &exec_state).expect("save state");

    // There should be incomplete state (no receipt yet)
    assert!(state::has_incomplete_state(&state_dir));

    // Reload state
    let loaded = state::load_state(&state_dir)
        .expect("load state")
        .expect("state exists");

    assert_eq!(loaded.plan_id, plan_id);
    assert_eq!(loaded.packages.len(), 2);

    // Verify package states roundtrip correctly
    let core_progress = loaded.packages.get("core@0.1.0").expect("core exists");
    assert!(matches!(core_progress.state, PackageState::Published));
    assert_eq!(core_progress.attempts, 1);

    let app_progress = loaded.packages.get("app@0.1.0").expect("app exists");
    assert!(matches!(app_progress.state, PackageState::Pending));
    assert_eq!(app_progress.attempts, 0);

    // Simulate completing the resume: mark app as published
    let mut updated = loaded;
    if let Some(app) = updated.packages.get_mut("app@0.1.0") {
        app.state = PackageState::Published;
        app.attempts = 1;
        app.last_updated_at = Utc::now();
    }
    updated.updated_at = Utc::now();

    state::save_state(&state_dir, &updated).expect("save updated state");

    // Write receipt
    let receipt = sample_receipt(plan_id);
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Incomplete state should now be false
    assert!(!state::has_incomplete_state(&state_dir));

    // Reload receipt and verify
    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");
    assert_eq!(loaded_receipt.plan_id, plan_id);
    assert_eq!(
        loaded_receipt.receipt_version,
        state::CURRENT_RECEIPT_VERSION
    );
}

// ===========================================================================
// 6. Event logging throughout a simulated publish
// ===========================================================================

#[test]
fn event_log_simulated_publish_lifecycle() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::events::events_path(&state_dir);
    let plan_id = "sim-plan-001";

    // Phase 1: Plan creation
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

    // Phase 2: Publishing "core"
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
        event_type: EventType::PackagePublished { duration_ms: 1500 },
        package: "core@0.1.0".to_string(),
    });

    // Phase 3: Readiness check
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
            visible: false,
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll {
            attempt: 2,
            visible: true,
        },
        package: "core@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessComplete {
            duration_ms: 3200,
            attempts: 2,
        },
        package: "core@0.1.0".to_string(),
    });

    // Phase 4: Publishing "app" (fails then succeeds)
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
            class: ErrorClass::Retryable,
            message: "rate limited".to_string(),
        },
        package: "app@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 2,
            command: "cargo publish -p app".to_string(),
        },
        package: "app@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 800 },
        package: "app@0.1.0".to_string(),
    });

    // Phase 5: Execution complete
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    // Write to file
    log.write_to_file(&events_path).expect("write events");

    // Reload from file
    let loaded = EventLog::read_from_file(&events_path).expect("read events");
    let all = loaded.all_events();
    assert_eq!(all.len(), 14);

    // Verify per-package filtering
    let core_events = loaded.events_for_package("core@0.1.0");
    assert_eq!(core_events.len(), 7); // started, attempted, published, readiness×4

    let app_events = loaded.events_for_package("app@0.1.0");
    assert_eq!(app_events.len(), 4); // started, failed, attempted, published

    let global_events = loaded.events_for_package("all");
    assert_eq!(global_events.len(), 3); // plan_created, execution_started, execution_finished
}

// ===========================================================================
// 7. FileStore end-to-end: state + receipt + events through the store trait
// ===========================================================================

#[test]
fn file_store_full_lifecycle() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let plan_id = "store-plan-xyz";

    // Save state via store
    let exec_state = sample_state(plan_id);
    store.save_state(&exec_state).expect("save state");

    // Save events via store
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
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Save receipt via store
    let receipt = sample_receipt(plan_id);
    store.save_receipt(&receipt).expect("save receipt");

    // Load everything back via store
    let loaded_state = store
        .load_state()
        .expect("load state")
        .expect("state exists");
    assert_eq!(loaded_state.plan_id, plan_id);
    assert_eq!(loaded_state.packages.len(), 2);

    let loaded_receipt = store
        .load_receipt()
        .expect("load receipt")
        .expect("receipt exists");
    assert_eq!(loaded_receipt.plan_id, plan_id);

    let loaded_events = store
        .load_events()
        .expect("load events")
        .expect("events exist");
    assert_eq!(loaded_events.all_events().len(), 2);

    // Schema validation through store trait
    store
        .validate_version(state::CURRENT_RECEIPT_VERSION)
        .expect("current version valid");
    store
        .validate_version(state::MINIMUM_SUPPORTED_VERSION)
        .expect("minimum version valid");
    assert!(store.validate_version("shipper.receipt.v0").is_err());

    // Clear and verify
    store.clear().expect("clear store");
    assert!(store.load_state().expect("load state").is_none());
    assert!(store.load_receipt().expect("load receipt").is_none());
    assert!(store.load_events().expect("load events").is_none());
}

// ===========================================================================
// 8. Plan determinism: same input produces same plan_id
// ===========================================================================

#[test]
fn plan_is_deterministic_across_builds() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_two_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };

    let ws1 = plan::build_plan(&spec).expect("build plan 1");
    let ws2 = plan::build_plan(&spec).expect("build plan 2");

    assert_eq!(ws1.plan.plan_id, ws2.plan.plan_id);
    assert_eq!(ws1.plan.packages.len(), ws2.plan.packages.len());
    for (a, b) in ws1.plan.packages.iter().zip(ws2.plan.packages.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.version, b.version);
    }
}

// ===========================================================================
// 9. Config validation rejects invalid then accepts valid
// ===========================================================================

#[test]
fn config_validate_rejects_bad_then_accepts_good() {
    let td = tempdir().expect("tempdir");

    // Write invalid config
    let bad_path = td.path().join("bad.toml");
    fs::write(&bad_path, "[retry]\nmax_attempts = -5\n").expect("write bad config");
    assert!(ShipperConfig::load_from_file(&bad_path).is_err());

    // Write valid config
    let good_path = td.path().join("good.toml");
    fs::write(&good_path, ShipperConfig::default_toml_template()).expect("write good config");
    let config = ShipperConfig::load_from_file(&good_path).expect("load good config");
    config.validate().expect("validate good config");
}

// ===========================================================================
// 10. Event log persistence through FileStore then direct state reload
// ===========================================================================

#[test]
fn events_persisted_via_store_readable_via_state_module() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().to_path_buf();
    let store = FileStore::new(state_dir.clone());

    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: shipper::types::Finishability::Proven,
        },
        package: "all".to_string(),
    });

    store.save_events(&events).expect("save events via store");

    // Read back through the events module directly
    let events_file = shipper::events::events_path(&state_dir);
    let loaded = EventLog::read_from_file(&events_file).expect("read events directly");
    assert_eq!(loaded.all_events().len(), 2);
}
