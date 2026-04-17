//! Cross-crate integration tests verifying that shipper's public modules
//! compose correctly: config → plan, plan → state, auth → registry,
//! state → store → events, and full preflight flows with a mocked registry.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tempfile::tempdir;

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::plan;
use shipper::state::events::EventLog;
use shipper::state::execution_state as state;
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
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
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

    let events_path = shipper::state::events::events_path(&state_dir);
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
    let events_file = shipper::state::events::events_path(&state_dir);
    let loaded = EventLog::read_from_file(&events_file).expect("read events directly");
    assert_eq!(loaded.all_events().len(), 2);
}

// ===========================================================================
// 11. Plan creation for multi-level dependency trees (deep 5-crate chain)
// ===========================================================================

/// Five-crate linear chain: a → b → c → d → e
fn create_deep_chain_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["a", "b", "c", "d", "e"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("a/Cargo.toml"),
        r#"
[package]
name = "a"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("a/src/lib.rs"), "pub fn a() {}\n");

    write_file(
        &root.join("b/Cargo.toml"),
        r#"
[package]
name = "b"
version = "0.1.0"
edition = "2021"

[dependencies]
a = { path = "../a", version = "0.1.0" }
"#,
    );
    write_file(&root.join("b/src/lib.rs"), "pub fn b() {}\n");

    write_file(
        &root.join("c/Cargo.toml"),
        r#"
[package]
name = "c"
version = "0.1.0"
edition = "2021"

[dependencies]
b = { path = "../b", version = "0.1.0" }
"#,
    );
    write_file(&root.join("c/src/lib.rs"), "pub fn c() {}\n");

    write_file(
        &root.join("d/Cargo.toml"),
        r#"
[package]
name = "d"
version = "0.1.0"
edition = "2021"

[dependencies]
c = { path = "../c", version = "0.1.0" }
"#,
    );
    write_file(&root.join("d/src/lib.rs"), "pub fn d() {}\n");

    write_file(
        &root.join("e/Cargo.toml"),
        r#"
[package]
name = "e"
version = "0.1.0"
edition = "2021"

[dependencies]
d = { path = "../d", version = "0.1.0" }
"#,
    );
    write_file(&root.join("e/src/lib.rs"), "pub fn e() {}\n");
}

/// Wide workspace: four independent crates with no inter-dependencies.
fn create_wide_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["alpha", "beta", "gamma", "delta"]
resolver = "2"
"#,
    );

    for name in &["alpha", "beta", "gamma", "delta"] {
        write_file(
            &root.join(format!("{name}/Cargo.toml")),
            &format!(
                r#"
[package]
name = "{name}"
version = "1.0.0"
edition = "2021"
"#
            ),
        );
        write_file(&root.join(format!("{name}/src/lib.rs")), "");
    }
}

#[test]
fn deep_chain_plan_respects_linear_dependency_order() {
    let td = tempdir().expect("tempdir");
    create_deep_chain_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 5);
    assert_eq!(names, vec!["a", "b", "c", "d", "e"]);

    // Verify dependency map
    let a_deps = ws.plan.dependencies.get("a").expect("a deps");
    assert!(a_deps.is_empty());
    let e_deps = ws.plan.dependencies.get("e").expect("e deps");
    assert!(e_deps.contains(&"d".to_string()));
}

#[test]
fn deep_chain_levels_are_all_singletons() {
    let td = tempdir().expect("tempdir");
    create_deep_chain_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let levels = ws.plan.group_by_levels();

    // A linear chain produces one package per level
    assert_eq!(levels.len(), 5);
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(level.packages.len(), 1, "level {i} should have 1 package");
    }
}

// ===========================================================================
// 12. Preflight mock: registry returns various HTTP error statuses
// ===========================================================================

#[test]
fn registry_version_check_errors_on_500() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ = req
                .respond(tiny_http::Response::from_string("internal error").with_status_code(500));
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let err = client
        .version_exists("some-crate", "1.0.0")
        .expect_err("500 should produce error");
    assert!(
        format!("{err:#}").contains("unexpected status"),
        "error should mention unexpected status"
    );

    handler.join().expect("handler thread");
}

#[test]
fn registry_crate_exists_errors_on_503() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ = req.respond(
                tiny_http::Response::from_string("service unavailable").with_status_code(503),
            );
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let err = client
        .crate_exists("some-crate")
        .expect_err("503 should produce error");
    assert!(
        format!("{err:#}").contains("unexpected status"),
        "error should mention unexpected status"
    );

    handler.join().expect("handler thread");
}

#[test]
fn registry_list_owners_errors_on_429() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ = req.respond(
                tiny_http::Response::from_string("too many requests").with_status_code(429),
            );
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let err = client
        .list_owners("some-crate", "token")
        .expect_err("429 should produce error");
    assert!(
        format!("{err:#}").contains("unexpected status"),
        "error should mention unexpected status"
    );

    handler.join().expect("handler thread");
}

// ===========================================================================
// 13. State persistence roundtrip with all PackageState variants
// ===========================================================================

#[test]
fn state_roundtrip_preserves_all_package_state_variants() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let mut packages = BTreeMap::new();
    packages.insert(
        "pending-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "pending-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "uploaded-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "uploaded-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Uploaded,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "published-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "published-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "skipped-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "skipped-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".to_string(),
            },
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "failed-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "failed-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 3,
            state: PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "rate limited".to_string(),
            },
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "ambiguous-pkg@1.0.0".to_string(),
        PackageProgress {
            name: "ambiguous-pkg".to_string(),
            version: "1.0.0".to_string(),
            attempts: 2,
            state: PackageState::Ambiguous {
                message: "timeout during publish".to_string(),
            },
            last_updated_at: Utc::now(),
        },
    );

    let exec_state = ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "all-variants-test".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };

    state::save_state(&state_dir, &exec_state).expect("save state");
    let loaded = state::load_state(&state_dir)
        .expect("load state")
        .expect("state exists");

    assert_eq!(loaded.packages.len(), 6);

    assert!(matches!(
        loaded.packages["pending-pkg@1.0.0"].state,
        PackageState::Pending
    ));
    assert!(matches!(
        loaded.packages["uploaded-pkg@1.0.0"].state,
        PackageState::Uploaded
    ));
    assert!(matches!(
        loaded.packages["published-pkg@1.0.0"].state,
        PackageState::Published
    ));
    assert!(matches!(
        loaded.packages["skipped-pkg@1.0.0"].state,
        PackageState::Skipped { .. }
    ));
    assert!(matches!(
        loaded.packages["failed-pkg@1.0.0"].state,
        PackageState::Failed { .. }
    ));
    assert!(matches!(
        loaded.packages["ambiguous-pkg@1.0.0"].state,
        PackageState::Ambiguous { .. }
    ));

    // Verify Failed details preserved
    if let PackageState::Failed { class, message } = &loaded.packages["failed-pkg@1.0.0"].state {
        assert_eq!(*class, ErrorClass::Retryable);
        assert_eq!(message, "rate limited");
    }

    // Verify Skipped reason preserved
    if let PackageState::Skipped { reason } = &loaded.packages["skipped-pkg@1.0.0"].state {
        assert_eq!(reason, "already published");
    }
}

// ===========================================================================
// 14. Resume from saved state: skip published, retry failed
// ===========================================================================

#[test]
fn resume_skips_published_and_retries_failed() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    // Initial state: a published, b failed (retryable), c+d+e pending
    let mut packages = BTreeMap::new();
    packages.insert(
        "a@0.1.0".to_string(),
        PackageProgress {
            name: "a".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "b@0.1.0".to_string(),
        PackageProgress {
            name: "b".to_string(),
            version: "0.1.0".to_string(),
            attempts: 2,
            state: PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "connection reset".to_string(),
            },
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "c@0.1.0".to_string(),
        PackageProgress {
            name: "c".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "d@0.1.0".to_string(),
        PackageProgress {
            name: "d".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        "e@0.1.0".to_string(),
        PackageProgress {
            name: "e".to_string(),
            version: "0.1.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );

    let exec_state = ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "resume-test-chain".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };

    state::save_state(&state_dir, &exec_state).expect("save initial state");
    assert!(state::has_incomplete_state(&state_dir));

    // Load and simulate resume: published packages should be skipped
    let mut loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");

    // Identify packages needing work
    let published: Vec<String> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Published))
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(published, vec!["a@0.1.0"]);

    let needs_retry: Vec<String> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Failed { .. }))
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(needs_retry, vec!["b@0.1.0"]);

    let pending: Vec<String> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Pending))
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(pending.len(), 3);

    // Simulate: retry b successfully, then publish c, d, e
    for key in &["b@0.1.0", "c@0.1.0", "d@0.1.0", "e@0.1.0"] {
        if let Some(pkg) = loaded.packages.get_mut(*key) {
            pkg.state = PackageState::Published;
            pkg.attempts += 1;
            pkg.last_updated_at = Utc::now();
        }
    }
    loaded.updated_at = Utc::now();
    state::save_state(&state_dir, &loaded).expect("save resumed state");

    // Verify all published
    let final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert!(
        final_state
            .packages
            .values()
            .all(|p| matches!(p.state, PackageState::Published))
    );
    // b should have 3 total attempts (2 original + 1 retry)
    assert_eq!(final_state.packages["b@0.1.0"].attempts, 3);
}

// ===========================================================================
// 15. Event log generation for full publish flow with 5-crate chain
// ===========================================================================

#[test]
fn event_log_full_publish_flow_deep_chain() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::state::events::events_path(&state_dir);
    let plan_id = "chain-publish-001";

    let mut log = EventLog::new();

    // Plan + execution start
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 5,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // Publish all 5 packages: a succeeds, b succeeds, c fails then retries, d+e succeed
    for (name, attempt_count) in &[("a", 1), ("b", 1), ("d", 1), ("e", 1)] {
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
            event_type: EventType::PackageAttempted {
                attempt: *attempt_count,
                command: format!("cargo publish -p {name}"),
            },
            package: format!("{name}@0.1.0"),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 500 },
            package: format!("{name}@0.1.0"),
        });
    }

    // Package c: fails first, then retries successfully
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "c".to_string(),
            version: "0.1.0".to_string(),
        },
        package: "c@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "registry timeout".to_string(),
        },
        package: "c@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 2,
            command: "cargo publish -p c".to_string(),
        },
        package: "c@0.1.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 800 },
        package: "c@0.1.0".to_string(),
    });

    // Execution finished
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    // Write and reload
    log.write_to_file(&events_path).expect("write events");
    let loaded = EventLog::read_from_file(&events_path).expect("read events");

    // 2 global + 4×3 simple + 4 for c (started+failed+attempted+published) = 2 + 12 + 4 + 1 = 19
    assert_eq!(loaded.all_events().len(), 19);

    // c had a failure + retry
    let c_events = loaded.events_for_package("c@0.1.0");
    assert_eq!(c_events.len(), 4);

    // Global events
    let global = loaded.events_for_package("all");
    assert_eq!(global.len(), 3); // plan_created + exec_started + exec_finished
}

// ===========================================================================
// 16. Receipt generation with mixed outcomes (published + failed + skipped)
// ===========================================================================

#[test]
fn receipt_with_mixed_outcomes_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "mixed-outcomes-plan".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![
            PackageReceipt {
                name: "a".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 1200,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
            PackageReceipt {
                name: "b".to_string(),
                version: "0.1.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "auth failure".to_string(),
                },
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 5000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
            PackageReceipt {
                name: "c".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Skipped {
                    reason: "version already exists".to_string(),
                },
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 0,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
        ],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");

    assert_eq!(loaded.packages.len(), 3);
    assert!(matches!(loaded.packages[0].state, PackageState::Published));
    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Failed { .. }
    ));
    assert!(matches!(
        loaded.packages[2].state,
        PackageState::Skipped { .. }
    ));

    // Verify specific failed details roundtrip
    if let PackageState::Failed { class, message } = &loaded.packages[1].state {
        assert_eq!(*class, ErrorClass::Permanent);
        assert_eq!(message, "auth failure");
    }
}

// ===========================================================================
// 17. Environment fingerprint in receipt roundtrip
// ===========================================================================

#[test]
fn receipt_environment_fingerprint_roundtrips() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "env-fingerprint-test".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![],
        event_log_path: PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0-rc.1".to_string(),
            cargo_version: Some("1.82.0-nightly".to_string()),
            rust_version: Some("1.82.0-nightly".to_string()),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");

    assert_eq!(loaded.environment.shipper_version, "0.3.0-rc.1");
    assert_eq!(
        loaded.environment.cargo_version.as_deref(),
        Some("1.82.0-nightly")
    );
    assert_eq!(
        loaded.environment.rust_version.as_deref(),
        Some("1.82.0-nightly")
    );
    assert_eq!(loaded.environment.os, "macos");
    assert_eq!(loaded.environment.arch, "aarch64");
}

// ===========================================================================
// 18. Lock acquire + set plan_id + release sequence
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_acquire_set_plan_id_release_sequence() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path();

    // Acquire lock
    let mut lock = shipper::lock::LockFile::acquire(state_dir, None).expect("acquire lock");

    // Lock should exist
    assert!(shipper::lock::LockFile::is_locked(state_dir, None).expect("check"));

    // Read lock info
    let info = shipper::lock::LockFile::read_lock_info(state_dir, None).expect("read info");
    assert_eq!(info.pid, std::process::id());
    assert!(info.plan_id.is_none());

    // Update plan_id
    lock.set_plan_id("lock-test-plan-123").expect("set plan_id");
    let updated_info =
        shipper::lock::LockFile::read_lock_info(state_dir, None).expect("read updated");
    assert_eq!(updated_info.plan_id.as_deref(), Some("lock-test-plan-123"));

    // Second acquire should fail
    let err =
        shipper::lock::LockFile::acquire(state_dir, None).expect_err("double acquire should fail");
    assert!(err.to_string().contains("lock already held"));

    // Release lock
    lock.release().expect("release lock");
    assert!(!shipper::lock::LockFile::is_locked(state_dir, None).expect("check after release"));

    // Re-acquire should succeed after release
    let _lock2 =
        shipper::lock::LockFile::acquire(state_dir, None).expect("re-acquire after release");
}

// ===========================================================================
// 19. Snapshot tests for plan output with various topologies
// ===========================================================================

/// Helper: extract stable plan representation (names + versions in order)
#[derive(Debug)]
#[allow(dead_code)] // fields used via Debug derive for insta snapshots
struct PlanSnapshot {
    packages: Vec<(String, String)>,
    levels: Vec<Vec<String>>,
    dependency_count: usize,
}

fn snapshot_plan(ws: &shipper::plan::PlannedWorkspace) -> PlanSnapshot {
    let packages = ws
        .plan
        .packages
        .iter()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    let levels = ws
        .plan
        .group_by_levels()
        .iter()
        .map(|l| l.packages.iter().map(|p| p.name.clone()).collect())
        .collect();
    let dependency_count = ws.plan.dependencies.values().map(|v| v.len()).sum();
    PlanSnapshot {
        packages,
        levels,
        dependency_count,
    }
}

#[test]
fn snapshot_deep_chain_plan() {
    let td = tempdir().expect("tempdir");
    create_deep_chain_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let snap = snapshot_plan(&ws);

    insta::assert_debug_snapshot!("deep_chain_plan", snap);
}

#[test]
fn snapshot_wide_workspace_plan() {
    let td = tempdir().expect("tempdir");
    create_wide_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let snap = snapshot_plan(&ws);

    insta::assert_debug_snapshot!("wide_workspace_plan", snap);
}

#[test]
fn snapshot_two_crate_plan() {
    let td = tempdir().expect("tempdir");
    create_two_crate_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let snap = snapshot_plan(&ws);

    insta::assert_debug_snapshot!("two_crate_plan", snap);
}

// ===========================================================================
// 20. Error propagation: registry error surfaces through version check
// ===========================================================================

#[test]
fn registry_error_propagates_with_context() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    // Server returns 500 for version check, 503 for crate check
    let handler = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok(req) = server.recv() {
                let url = req.url().to_string();
                if url.contains("/api/v1/crates/") && url.contains("/1.0.0") {
                    let _ = req.respond(
                        tiny_http::Response::from_string("internal error").with_status_code(500),
                    );
                } else {
                    let _ = req.respond(
                        tiny_http::Response::from_string("unavailable").with_status_code(503),
                    );
                }
            }
        }
    });

    let reg = Registry {
        name: "error-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    // version_exists should error
    let ver_err = client
        .version_exists("test-crate", "1.0.0")
        .expect_err("should error");
    let ver_msg = format!("{ver_err:#}");
    assert!(
        ver_msg.contains("unexpected status"),
        "version error: {ver_msg}"
    );

    // crate_exists should also error
    let crate_err = client.crate_exists("test-crate").expect_err("should error");
    let crate_msg = format!("{crate_err:#}");
    assert!(
        crate_msg.contains("unexpected status"),
        "crate error: {crate_msg}"
    );

    handler.join().expect("handler thread");
}
