//! Facade integration tests for the shipper crate.
//!
//! Covers cross-module integration scenarios not present in
//! `cross_crate_integration.rs`: multi-crate plan building with
//! package selection, config validation pipeline, state persistence
//! and resumption flow, event emission during operations, auth token
//! resolution integrated with config, and registry checking with a
//! mock HTTP server.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::Utc;
use serial_test::serial;
use tempfile::tempdir;

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::events::EventLog;
use shipper::plan;
use shipper::state;
use shipper::store::{FileStore, StateStore};
use shipper::types::{
    AuthType, EnvironmentFingerprint, ErrorClass, EventType, ExecutionResult, ExecutionState,
    Finishability, PackageEvidence, PackageProgress, PackageReceipt, PackageState, PublishEvent,
    ReadinessMethod, Registry, ReleaseSpec,
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

/// Create a three-crate workspace: `base` (no deps), `mid` depends on `base`,
/// `top` depends on `mid`.
fn create_three_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["base", "mid", "top"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("base/Cargo.toml"),
        r#"
[package]
name = "base"
version = "0.2.0"
edition = "2021"
"#,
    );
    write_file(&root.join("base/src/lib.rs"), "pub fn base_fn() {}\n");

    write_file(
        &root.join("mid/Cargo.toml"),
        r#"
[package]
name = "mid"
version = "0.2.0"
edition = "2021"

[dependencies]
base = { path = "../base", version = "0.2.0" }
"#,
    );
    write_file(
        &root.join("mid/src/lib.rs"),
        "pub fn mid_fn() { base::base_fn(); }\n",
    );

    write_file(
        &root.join("top/Cargo.toml"),
        r#"
[package]
name = "top"
version = "0.2.0"
edition = "2021"

[dependencies]
mid = { path = "../mid", version = "0.2.0" }
"#,
    );
    write_file(
        &root.join("top/src/lib.rs"),
        "pub fn top_fn() { mid::mid_fn(); }\n",
    );
}

/// Create a workspace with a mix of publishable and non-publishable crates.
fn create_mixed_publishability_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["pub_a", "pub_b", "internal"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("pub_a/Cargo.toml"),
        r#"
[package]
name = "pub_a"
version = "1.0.0"
edition = "2021"
"#,
    );
    write_file(&root.join("pub_a/src/lib.rs"), "");

    write_file(
        &root.join("pub_b/Cargo.toml"),
        r#"
[package]
name = "pub_b"
version = "1.0.0"
edition = "2021"

[dependencies]
pub_a = { path = "../pub_a", version = "1.0.0" }
"#,
    );
    write_file(&root.join("pub_b/src/lib.rs"), "");

    write_file(
        &root.join("internal/Cargo.toml"),
        r#"
[package]
name = "internal"
version = "0.0.0"
edition = "2021"
publish = false
"#,
    );
    write_file(&root.join("internal/src/lib.rs"), "");
}

fn sample_state(plan_id: &str, packages: &[(&str, &str, PackageState, u32)]) -> ExecutionState {
    let mut map = BTreeMap::new();
    for &(name, version, ref pstate, attempts) in packages {
        map.insert(
            format!("{name}@{version}"),
            PackageProgress {
                name: name.to_string(),
                version: version.to_string(),
                attempts,
                state: pstate.clone(),
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
        packages: map,
    }
}

fn sample_receipt(plan_id: &str, pkg_names: &[&str]) -> shipper::types::Receipt {
    let packages = pkg_names
        .iter()
        .map(|name| PackageReceipt {
            name: name.to_string(),
            version: "0.2.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
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
// 1. Plan building from a three-crate workspace — dependency ordering
// ===========================================================================

#[test]
fn three_crate_plan_respects_dependency_order() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    assert_eq!(ws.plan.packages.len(), 3);
    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    // base must come before mid, mid before top
    let base_pos = names.iter().position(|&n| n == "base").unwrap();
    let mid_pos = names.iter().position(|&n| n == "mid").unwrap();
    let top_pos = names.iter().position(|&n| n == "top").unwrap();
    assert!(base_pos < mid_pos, "base must precede mid");
    assert!(mid_pos < top_pos, "mid must precede top");
}

// ===========================================================================
// 2. Plan building with package selection pulls in transitive deps
// ===========================================================================

#[test]
fn plan_with_selected_package_includes_transitive_deps() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: Some(vec!["top".to_string()]),
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Selecting "top" should pull in mid and base as dependencies
    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"base"), "base should be included");
    assert!(names.contains(&"mid"), "mid should be included");
    assert!(names.contains(&"top"), "top should be included");
    assert_eq!(names.len(), 3);
}

// ===========================================================================
// 3. Plan excludes non-publishable crates automatically
// ===========================================================================

#[test]
fn plan_filters_out_non_publishable_crates() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_mixed_publishability_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"pub_a"));
    assert!(names.contains(&"pub_b"));
    assert!(
        !names.contains(&"internal"),
        "non-publishable should be excluded"
    );

    // The skipped list should mention the internal crate
    let skipped_names: Vec<&str> = ws.skipped.iter().map(|s| s.name.as_str()).collect();
    assert!(
        skipped_names.contains(&"internal"),
        "internal should appear in skipped list"
    );
}

// ===========================================================================
// 4. Config validation pipeline: valid → roundtrip → invalid rejection
// ===========================================================================

#[test]
fn config_validation_pipeline() {
    let td = tempdir().expect("tempdir");

    // Generate default template, write, load, validate
    let template = ShipperConfig::default_toml_template();
    let path = td.path().join(".shipper.toml");
    fs::write(&path, &template).expect("write config");

    let config = ShipperConfig::load_from_file(&path).expect("load config");
    config.validate().expect("validate default config");

    // Verify that building runtime options from defaults succeeds
    let opts = config.build_runtime_options(CliOverrides::default());
    assert!(opts.max_attempts > 0);

    // Invalid: zero output lines
    let bad_toml = "[output]\nlines = 0\n";
    let bad_path = td.path().join("bad.toml");
    fs::write(&bad_path, bad_toml).expect("write bad config");
    let bad_config = ShipperConfig::load_from_file(&bad_path);
    if let Ok(cfg) = bad_config {
        assert!(
            cfg.validate().is_err(),
            "zero output lines should fail validation"
        );
    }

    // Invalid: max_delay < base_delay
    let bad_retry = r#"
[retry]
base_delay = "10s"
max_delay = "1s"
"#;
    let bad_retry_path = td.path().join("bad_retry.toml");
    fs::write(&bad_retry_path, bad_retry).expect("write bad retry config");
    let bad_retry_config = ShipperConfig::load_from_file(&bad_retry_path);
    if let Ok(cfg) = bad_retry_config {
        assert!(
            cfg.validate().is_err(),
            "max_delay < base_delay should fail validation"
        );
    }
}

// ===========================================================================
// 5. Config CLI overrides take precedence
// ===========================================================================

#[test]
fn config_cli_overrides_take_precedence() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join(".shipper.toml");
    fs::write(&path, ShipperConfig::default_toml_template()).expect("write config");

    let config = ShipperConfig::load_from_file(&path).expect("load config");

    let overrides = CliOverrides {
        output_lines: Some(512),
        max_attempts: Some(10),
        allow_dirty: true,
        no_verify: true,
        ..Default::default()
    };
    let opts = config.build_runtime_options(overrides);

    assert_eq!(opts.output_lines, 512);
    assert_eq!(opts.max_attempts, 10);
    assert!(opts.allow_dirty);
    assert!(opts.no_verify);
}

// ===========================================================================
// 6. State persistence: three-package partial progress → resume → complete
// ===========================================================================

#[test]
fn state_persistence_three_package_resume_flow() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "facade-plan-3pkg";

    // Initial state: base published, mid and top pending
    let exec_state = sample_state(
        plan_id,
        &[
            ("base", "0.2.0", PackageState::Published, 1),
            ("mid", "0.2.0", PackageState::Pending, 0),
            ("top", "0.2.0", PackageState::Pending, 0),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save state");
    assert!(state::has_incomplete_state(&state_dir));

    // Reload and verify partial state
    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.plan_id, plan_id);

    let pending: Vec<&str> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Pending))
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(pending.len(), 2);

    // Simulate resumption: publish mid
    let mut resumed = loaded;
    if let Some(mid) = resumed.packages.get_mut("mid@0.2.0") {
        mid.state = PackageState::Published;
        mid.attempts = 1;
        mid.last_updated_at = Utc::now();
    }
    resumed.updated_at = Utc::now();
    state::save_state(&state_dir, &resumed).expect("save after mid");
    assert!(state::has_incomplete_state(&state_dir));

    // Continue: publish top
    let mut final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    if let Some(top) = final_state.packages.get_mut("top@0.2.0") {
        top.state = PackageState::Published;
        top.attempts = 1;
        top.last_updated_at = Utc::now();
    }
    final_state.updated_at = Utc::now();
    state::save_state(&state_dir, &final_state).expect("save final");

    // Write receipt to signal completion
    let receipt = sample_receipt(plan_id, &["base", "mid", "top"]);
    state::write_receipt(&state_dir, &receipt).expect("write receipt");
    assert!(!state::has_incomplete_state(&state_dir));

    // Verify final receipt
    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");
    assert_eq!(loaded_receipt.packages.len(), 3);
    assert!(
        loaded_receipt
            .packages
            .iter()
            .all(|p| matches!(p.state, PackageState::Published))
    );
}

// ===========================================================================
// 7. State clear removes state but not receipt
// ===========================================================================

#[test]
fn state_clear_removes_state_file() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let exec_state = sample_state(
        "clear-test",
        &[("pkg", "1.0.0", PackageState::Published, 1)],
    );
    state::save_state(&state_dir, &exec_state).expect("save");

    assert!(state::load_state(&state_dir).expect("load").is_some());
    state::clear_state(&state_dir).expect("clear");
    assert!(
        state::load_state(&state_dir)
            .expect("load after clear")
            .is_none()
    );
}

// ===========================================================================
// 8. Event emission: full lifecycle with preflight + publish + readiness
// ===========================================================================

#[test]
fn event_emission_full_lifecycle_with_preflight() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::events::events_path(&state_dir);
    let plan_id = "facade-events-001";

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
            output: "3 publishable crates found".to_string(),
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightNewCrateDetected {
            crate_name: "base".to_string(),
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: Finishability::Proven,
        },
        package: "all".to_string(),
    });

    // Plan creation
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // Publish base with readiness
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "base".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p base".to_string(),
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 1200 },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll {
            attempt: 1,
            visible: true,
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessComplete {
            duration_ms: 500,
            attempts: 1,
        },
        package: "base@0.2.0".to_string(),
    });

    // Publish mid — fails once then succeeds
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "mid".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "mid@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "connection reset".to_string(),
        },
        package: "mid@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 2,
            command: "cargo publish -p mid".to_string(),
        },
        package: "mid@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 900 },
        package: "mid@0.2.0".to_string(),
    });

    // Publish top — succeeds first try
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "top".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "top@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p top".to_string(),
        },
        package: "top@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 600 },
        package: "top@0.2.0".to_string(),
    });

    // Execution complete
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

    assert_eq!(loaded.all_events().len(), 20);

    // Verify per-package event counts
    let base_events = loaded.events_for_package("base@0.2.0");
    assert_eq!(base_events.len(), 7); // new-crate + started + attempted + published + readiness×3

    let mid_events = loaded.events_for_package("mid@0.2.0");
    assert_eq!(mid_events.len(), 4); // started + failed + attempted + published

    let top_events = loaded.events_for_package("top@0.2.0");
    assert_eq!(top_events.len(), 3); // started + attempted + published

    let global_events = loaded.events_for_package("all");
    assert_eq!(global_events.len(), 6); // preflight×3 + plan_created + exec_started + exec_finished
}

// ===========================================================================
// 9. Events persisted through FileStore match direct reads
// ===========================================================================

#[test]
fn events_through_store_match_direct_event_log() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: "store-events-test".to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "base".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "base@0.2.0".to_string(),
    });

    store.save_events(&events).expect("save via store");

    // Load via store
    let via_store = store
        .load_events()
        .expect("load events")
        .expect("events exist");
    assert_eq!(via_store.all_events().len(), 3);

    // Load via direct file read
    let events_file = shipper::events::events_path(td.path());
    let via_file = EventLog::read_from_file(&events_file).expect("read directly");
    assert_eq!(via_file.all_events().len(), 3);
}

// ===========================================================================
// 10. Auth token resolution via env var (integration with config)
// ===========================================================================

#[test]
#[serial]
fn auth_token_resolved_from_env_for_crates_io() {
    temp_env::with_vars(
        [
            ("CARGO_REGISTRY_TOKEN", Some("test-token-abc")),
            ("CARGO_HOME", Some("__nonexistent_cargo_home__")),
        ],
        || {
            let token = shipper::auth::resolve_token("crates-io").expect("resolve");
            assert_eq!(token.as_deref(), Some("test-token-abc"));

            let auth_type = shipper::auth::detect_auth_type("crates-io").expect("detect");
            assert_eq!(auth_type, Some(AuthType::Token));
        },
    );
}

#[test]
#[serial]
fn auth_token_resolved_from_named_registry_env() {
    temp_env::with_vars(
        [
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_MY_REG_TOKEN", Some("private-token-xyz")),
            ("CARGO_HOME", Some("__nonexistent_cargo_home__")),
        ],
        || {
            let token = shipper::auth::resolve_token("my-reg").expect("resolve");
            assert_eq!(token.as_deref(), Some("private-token-xyz"));
        },
    );
}

#[test]
#[serial]
fn auth_returns_none_when_no_token_configured() {
    temp_env::with_vars(
        [
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ("CARGO_HOME", Some("__nonexistent_cargo_home__")),
        ],
        || {
            let token = shipper::auth::resolve_token("crates-io").expect("resolve");
            assert!(token.is_none());

            let auth_type = shipper::auth::detect_auth_type("crates-io").expect("detect");
            assert!(auth_type.is_none());
        },
    );
}

// ===========================================================================
// 11. Registry: crate_exists and check_new_crate with mock server
// ===========================================================================

#[test]
fn registry_crate_exists_with_mock() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let url = req.url().to_string();
            if url.contains("/api/v1/crates/existing-crate") {
                let body = r#"{"crate":{"name":"existing-crate"}}"#;
                let resp = tiny_http::Response::from_string(body).with_status_code(200);
                let _ = req.respond(resp);
            } else {
                let _ = req
                    .respond(tiny_http::Response::from_string("not found").with_status_code(404));
            }
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let exists = client.crate_exists("existing-crate").expect("check");
    assert!(exists);

    handler.join().expect("handler thread");
}

#[test]
fn registry_check_new_crate_returns_true_for_404() {
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
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let is_new = client.check_new_crate("brand-new-crate").expect("check");
    assert!(is_new, "404 should mean it's a new crate");

    handler.join().expect("handler thread");
}

// ===========================================================================
// 12. Registry: list_owners with mock server
// ===========================================================================

#[test]
fn registry_list_owners_with_mock() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let url = req.url().to_string();
            if url.contains("/owners") {
                let body = r#"{"users":[{"id":1,"login":"alice","name":"Alice"},{"id":2,"login":"bob","name":null}]}"#;
                let resp = tiny_http::Response::from_string(body).with_status_code(200);
                let _ = req.respond(resp);
            } else {
                let _ = req
                    .respond(tiny_http::Response::from_string("not found").with_status_code(404));
            }
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let owners = client
        .list_owners("my-crate", "fake-token")
        .expect("list owners");
    assert_eq!(owners.users.len(), 2);
    assert_eq!(owners.users[0].login, "alice");
    assert_eq!(owners.users[1].login, "bob");

    handler.join().expect("handler thread");
}

// ===========================================================================
// 13. Registry: multi-request session — version check for all planned packages
// ===========================================================================

#[test]
fn registry_multi_version_check_for_planned_packages() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Mock server responds to version checks: base exists, mid/top don't
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let pkg_count = ws.plan.packages.len();
    let handler = std::thread::spawn(move || {
        for _ in 0..pkg_count {
            if let Ok(req) = server.recv() {
                let url = req.url().to_string();
                if url.contains("/base/") {
                    let body = r#"{"version":{"num":"0.2.0"}}"#;
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

    let mut published = vec![];
    let mut unpublished = vec![];
    for pkg in &ws.plan.packages {
        let exists = client
            .version_exists(&pkg.name, &pkg.version)
            .expect("version check");
        if exists {
            published.push(pkg.name.as_str());
        } else {
            unpublished.push(pkg.name.as_str());
        }
    }

    assert_eq!(published, vec!["base"]);
    assert!(unpublished.contains(&"mid"));
    assert!(unpublished.contains(&"top"));

    handler.join().expect("handler thread");
}

// ===========================================================================
// 14. Plan + state + store: full lifecycle through FileStore
// ===========================================================================

#[test]
fn plan_state_store_full_lifecycle() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    // Build plan
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    // Initialize state from plan
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

    // Use FileStore
    let store_dir = td.path().join("store");
    let store = FileStore::new(store_dir.clone());

    store.save_state(&exec_state).expect("save state");

    // Build events
    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Write receipt
    let receipt = sample_receipt(
        &ws.plan.plan_id,
        &ws.plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
    );
    store.save_receipt(&receipt).expect("save receipt");

    // Verify everything roundtrips
    let loaded_state = store.load_state().expect("load").expect("exists");
    assert_eq!(loaded_state.plan_id, ws.plan.plan_id);
    assert_eq!(loaded_state.packages.len(), 3);

    let loaded_receipt = store.load_receipt().expect("load").expect("exists");
    assert_eq!(loaded_receipt.plan_id, ws.plan.plan_id);
    assert_eq!(loaded_receipt.packages.len(), 3);

    let loaded_events = store.load_events().expect("load").expect("exists");
    assert_eq!(loaded_events.all_events().len(), 1);

    // Clear and verify
    store.clear().expect("clear");
    assert!(store.load_state().expect("load after clear").is_none());
    assert!(store.load_receipt().expect("load after clear").is_none());
}

// ===========================================================================
// 15. Plan determinism: three-crate workspace produces stable plan_id
// ===========================================================================

#[test]
fn three_crate_plan_determinism() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };

    let ws1 = plan::build_plan(&spec).expect("plan 1");
    let ws2 = plan::build_plan(&spec).expect("plan 2");

    assert_eq!(ws1.plan.plan_id, ws2.plan.plan_id);
    assert_eq!(ws1.plan.packages.len(), ws2.plan.packages.len());
    for (a, b) in ws1.plan.packages.iter().zip(ws2.plan.packages.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.version, b.version);
    }
}

// ===========================================================================
// 16. State version validation through store
// ===========================================================================

#[test]
fn store_schema_version_validation() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    // Current versions should be valid
    store
        .validate_version(state::CURRENT_RECEIPT_VERSION)
        .expect("current version valid");
    store
        .validate_version(state::CURRENT_STATE_VERSION)
        .expect("state version valid");

    // Ancient/invalid versions should be rejected
    assert!(store.validate_version("shipper.receipt.v0").is_err());
    assert!(store.validate_version("invalid").is_err());
    assert!(store.validate_version("").is_err());
}

// ===========================================================================
// 17. Event log clear and re-record
// ===========================================================================

#[test]
fn event_log_clear_and_rerecord() {
    let mut log = EventLog::new();

    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    assert_eq!(log.all_events().len(), 1);

    log.clear();
    assert_eq!(log.all_events().len(), 0);

    // Re-record after clear
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: "after-clear".to_string(),
            package_count: 1,
        },
        package: "all".to_string(),
    });
    assert_eq!(log.all_events().len(), 1);

    // Verify file persistence after clear + re-record
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    log.write_to_file(&path).expect("write");
    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.all_events().len(), 1);
}

// ===========================================================================
// 18. Auth + config: token resolves correctly when config specifies registry
// ===========================================================================

#[test]
#[serial]
fn auth_token_integration_with_custom_registry_config() {
    let td = tempdir().expect("tempdir");

    // Write config with custom registry
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
[registry]
name = "my-private"
api_base = "https://my-registry.example.com"
"#,
    );

    let config = ShipperConfig::load_from_file(&td.path().join(".shipper.toml")).expect("load");
    let _opts = config.build_runtime_options(CliOverrides::default());

    // Verify auth resolves from the named registry env var
    temp_env::with_vars(
        [
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_MY_PRIVATE_TOKEN", Some("private-tok-123")),
            ("CARGO_HOME", Some("__nonexistent_cargo_home__")),
        ],
        || {
            let token = shipper::auth::resolve_token("my-private").expect("resolve");
            assert_eq!(token.as_deref(), Some("private-tok-123"));
        },
    );
}

// ===========================================================================
// 19. Registry: verify_ownership with mock returning 403
// ===========================================================================

#[test]
fn registry_verify_ownership_handles_forbidden() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        if let Ok(req) = server.recv() {
            let _ =
                req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
        }
    });

    let reg = Registry {
        name: "test-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let owned = client
        .verify_ownership("some-crate", "bad-token")
        .expect("verify");
    assert!(!owned, "403 should mean ownership not verified");

    handler.join().expect("handler thread");
}

// ===========================================================================
// 20. Failed package in state: round-trip preserves error class
// ===========================================================================

#[test]
fn state_preserves_failed_package_state() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let exec_state = sample_state(
        "failed-test",
        &[
            ("lib-a", "1.0.0", PackageState::Published, 1),
            (
                "lib-b",
                "1.0.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "network timeout".to_string(),
                },
                3,
            ),
            ("lib-c", "1.0.0", PackageState::Pending, 0),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save");

    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");

    let lib_b = loaded.packages.get("lib-b@1.0.0").expect("lib-b exists");
    assert!(matches!(lib_b.state, PackageState::Failed { .. }));
    assert_eq!(lib_b.attempts, 3);

    let lib_c = loaded.packages.get("lib-c@1.0.0").expect("lib-c exists");
    assert!(matches!(lib_c.state, PackageState::Pending));
    assert_eq!(lib_c.attempts, 0);
}

// ===========================================================================
// 21. Receipt with mixed outcomes (published + failed + skipped) through store
// ===========================================================================

#[test]
fn receipt_mixed_outcomes_through_file_store() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "mixed-receipt-facade".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![
            PackageReceipt {
                name: "base".to_string(),
                version: "0.2.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 800,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "mid".to_string(),
                version: "0.2.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "connection reset by peer".to_string(),
                },
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 15000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
            PackageReceipt {
                name: "top".to_string(),
                version: "0.2.0".to_string(),
                attempts: 0,
                state: PackageState::Skipped {
                    reason: "dependency mid failed".to_string(),
                },
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 0,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            },
        ],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    store.save_receipt(&receipt).expect("save receipt");
    let loaded = store
        .load_receipt()
        .expect("load receipt")
        .expect("receipt exists");

    assert_eq!(loaded.packages.len(), 3);

    // Verify state of each package
    assert!(matches!(loaded.packages[0].state, PackageState::Published));
    assert_eq!(loaded.packages[0].name, "base");

    assert!(matches!(
        loaded.packages[1].state,
        PackageState::Failed { .. }
    ));
    if let PackageState::Failed { class, message } = &loaded.packages[1].state {
        assert_eq!(*class, ErrorClass::Retryable);
        assert_eq!(message, "connection reset by peer");
    }

    assert!(matches!(
        loaded.packages[2].state,
        PackageState::Skipped { .. }
    ));
    if let PackageState::Skipped { reason } = &loaded.packages[2].state {
        assert_eq!(reason, "dependency mid failed");
    }
}

// ===========================================================================
// 22. Environment fingerprint in receipt survives store roundtrip
// ===========================================================================

#[test]
fn environment_fingerprint_in_receipt_through_store() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "env-fp-test".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
        },
    };

    store.save_receipt(&receipt).expect("save");
    let loaded = store.load_receipt().expect("load").expect("exists");

    assert_eq!(loaded.environment.shipper_version, "0.3.0");
    assert!(loaded.environment.cargo_version.is_none());
    assert!(loaded.environment.rust_version.is_none());
    assert_eq!(loaded.environment.os, "windows");
    assert_eq!(loaded.environment.arch, "x86_64");
}

// ===========================================================================
// 23. Lock acquire + publish simulation + release sequence
// ===========================================================================

#[test]
fn lock_acquire_publish_release_sequence() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    // Step 1: Acquire lock
    let lock = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("locked"));

    // Step 2: Set plan_id (simulating engine linking the plan to the lock)
    lock.set_plan_id("facade-lock-plan").expect("set plan_id");

    // Step 3: Simulate publish by writing state
    let exec_state = sample_state(
        "facade-lock-plan",
        &[
            ("base", "0.2.0", PackageState::Published, 1),
            ("mid", "0.2.0", PackageState::Published, 1),
            ("top", "0.2.0", PackageState::Published, 1),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save state");

    // Step 4: Write receipt
    let receipt = sample_receipt("facade-lock-plan", &["base", "mid", "top"]);
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Step 5: Release lock
    lock.release().expect("release");
    assert!(!shipper::lock::LockFile::is_locked(&state_dir, None).expect("unlocked"));

    // Verify state and receipt are accessible after lock release
    let loaded_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded_state.plan_id, "facade-lock-plan");

    let loaded_receipt = state::load_receipt(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(loaded_receipt.packages.len(), 3);
}

// ===========================================================================
// 24. Snapshot tests for three-crate plan
// ===========================================================================

/// Helper: extract stable plan info for snapshot testing
#[derive(Debug)]
#[allow(dead_code)] // fields used via Debug derive for insta snapshots
struct FacadePlanSnapshot {
    packages: Vec<(String, String)>,
    levels: Vec<Vec<String>>,
    skipped_count: usize,
}

fn facade_snapshot_plan(ws: &shipper::plan::PlannedWorkspace) -> FacadePlanSnapshot {
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
    FacadePlanSnapshot {
        packages,
        levels,
        skipped_count: ws.skipped.len(),
    }
}

#[test]
fn snapshot_three_crate_linear_plan() {
    let td = tempdir().expect("tempdir");
    create_three_crate_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let snap = facade_snapshot_plan(&ws);

    insta::assert_debug_snapshot!("three_crate_linear_plan", snap);
}

#[test]
fn snapshot_mixed_publishability_plan() {
    let td = tempdir().expect("tempdir");
    create_mixed_publishability_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let snap = facade_snapshot_plan(&ws);

    insta::assert_debug_snapshot!("mixed_publishability_plan", snap);
}

// ===========================================================================
// 25. Error propagation: registry server errors through the full stack
// ===========================================================================

#[test]
fn registry_server_error_propagates_through_version_check() {
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
        name: "broken-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    // The error should propagate through the version check
    let err = client
        .version_exists("some-crate", "1.0.0")
        .expect_err("500 should error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unexpected status"),
        "error message should include status context: {msg}"
    );

    handler.join().expect("handler thread");
}

#[test]
fn registry_server_error_propagates_through_crate_check() {
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
        name: "broken-registry".to_string(),
        api_base,
        index_base: None,
    };
    let client = shipper::registry::RegistryClient::new(reg).expect("client");

    let err = client
        .crate_exists("some-crate")
        .expect_err("503 should error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("unexpected status"),
        "error message should include status context: {msg}"
    );

    handler.join().expect("handler thread");
}

// ===========================================================================
// 26. Event log generation for full publish flow with preflight and mixed outcomes
// ===========================================================================

#[test]
fn event_log_full_publish_with_skipped_and_failed() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::events::events_path(&state_dir);

    let mut log = EventLog::new();

    // Preflight
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: Finishability::NotProven,
        },
        package: "all".to_string(),
    });

    // Plan
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: "mixed-events-plan".to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    // base: published
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "base".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "base@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 500 },
        package: "base@0.2.0".to_string(),
    });

    // mid: failed permanently
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "mid".to_string(),
            version: "0.2.0".to_string(),
        },
        package: "mid@0.2.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "invalid token".to_string(),
        },
        package: "mid@0.2.0".to_string(),
    });

    // top: skipped because mid failed
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageSkipped {
            reason: "dependency mid failed".to_string(),
        },
        package: "top@0.2.0".to_string(),
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

    assert_eq!(loaded.all_events().len(), 10);

    // base got started + published = 2
    let base_events = loaded.events_for_package("base@0.2.0");
    assert_eq!(base_events.len(), 2);

    // mid got started + failed = 2
    let mid_events = loaded.events_for_package("mid@0.2.0");
    assert_eq!(mid_events.len(), 2);

    // top got skipped = 1
    let top_events = loaded.events_for_package("top@0.2.0");
    assert_eq!(top_events.len(), 1);

    // global = preflight_started + preflight_complete + plan_created + exec_started + exec_finished = 5
    let global = loaded.events_for_package("all");
    assert_eq!(global.len(), 5);
}

// ===========================================================================
// 27. State persistence: resume with skipped packages preserved
// ===========================================================================

#[test]
fn state_resume_preserves_skipped_packages() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    // Initial state: base published, mid skipped, top pending
    let exec_state = sample_state(
        "skip-resume-test",
        &[
            ("base", "0.2.0", PackageState::Published, 1),
            (
                "mid",
                "0.2.0",
                PackageState::Skipped {
                    reason: "version already exists".to_string(),
                },
                0,
            ),
            ("top", "0.2.0", PackageState::Pending, 0),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save");

    // Load and simulate resume
    let mut loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");

    // Skipped packages should remain skipped
    let mid = loaded.packages.get("mid@0.2.0").expect("mid");
    assert!(matches!(mid.state, PackageState::Skipped { .. }));

    // Only pending packages need work
    let actionable: Vec<&str> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Pending))
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(actionable, vec!["top"]);

    // Publish top
    if let Some(top) = loaded.packages.get_mut("top@0.2.0") {
        top.state = PackageState::Published;
        top.attempts = 1;
        top.last_updated_at = Utc::now();
    }
    state::save_state(&state_dir, &loaded).expect("save resumed");

    // Final verify: skipped is still skipped
    let final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert!(matches!(
        final_state.packages["mid@0.2.0"].state,
        PackageState::Skipped { .. }
    ));
    assert!(matches!(
        final_state.packages["top@0.2.0"].state,
        PackageState::Published
    ));
}
