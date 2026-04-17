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
use shipper::plan;
use shipper::state::events::EventLog;
use shipper::state::execution_state as state;
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

    let events_path = shipper::state::events::events_path(&state_dir);
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
    let events_file = shipper::state::events::events_path(td.path());
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
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
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
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
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
#[allow(unused_mut)]
fn lock_acquire_publish_release_sequence() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    // Step 1: Acquire lock
    let mut lock = shipper::lock::LockFile::acquire(&state_dir, None).expect("acquire");
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

    let events_path = shipper::state::events::events_path(&state_dir);

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

// ===========================================================================
// 28. Error recovery: publish fails, state persisted, resume succeeds
// ===========================================================================

#[test]
fn error_recovery_fail_persist_resume_succeed() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let plan_id = "error-recovery-plan";

    // Phase 1: base publishes, mid fails with retryable error
    let failing_state = sample_state(
        plan_id,
        &[
            ("base", "0.2.0", PackageState::Published, 1),
            (
                "mid",
                "0.2.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "connection reset".to_string(),
                },
                2,
            ),
            ("top", "0.2.0", PackageState::Pending, 0),
        ],
    );
    state::save_state(&state_dir, &failing_state).expect("save failed state");

    // Verify incomplete state exists
    assert!(state::has_incomplete_state(&state_dir));

    // Phase 2: Resume — load state, reset failed package to pending, then publish
    let mut resumed = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert_eq!(resumed.plan_id, plan_id);

    // Reset the failed package for retry
    if let Some(mid) = resumed.packages.get_mut("mid@0.2.0") {
        assert!(matches!(mid.state, PackageState::Failed { .. }));
        mid.state = PackageState::Published;
        mid.attempts = 3;
        mid.last_updated_at = Utc::now();
    }
    if let Some(top) = resumed.packages.get_mut("top@0.2.0") {
        top.state = PackageState::Published;
        top.attempts = 1;
        top.last_updated_at = Utc::now();
    }
    resumed.updated_at = Utc::now();
    state::save_state(&state_dir, &resumed).expect("save resumed state");

    // Phase 3: Write receipt to complete
    let receipt = sample_receipt(plan_id, &["base", "mid", "top"]);
    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Verify no longer incomplete
    assert!(!state::has_incomplete_state(&state_dir));

    // Verify all packages published in final state
    let final_state = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");
    assert!(
        final_state
            .packages
            .values()
            .all(|p| matches!(p.state, PackageState::Published))
    );

    // Mid should show 3 attempts from the retry
    assert_eq!(final_state.packages["mid@0.2.0"].attempts, 3);
}

// ===========================================================================
// 29. Partial publish: first N crates succeed, then failure, verify state
// ===========================================================================

#[test]
fn partial_publish_first_n_succeed_then_failure() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());

    let plan_id = "partial-publish-plan";

    // Simulate: base and mid published, top fails, then verify the persisted state
    let partial_state = sample_state(
        plan_id,
        &[
            ("base", "0.2.0", PackageState::Published, 1),
            ("mid", "0.2.0", PackageState::Published, 1),
            (
                "top",
                "0.2.0",
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "version conflict".to_string(),
                },
                1,
            ),
        ],
    );
    store.save_state(&partial_state).expect("save state");

    // Also persist an event log capturing the partial publish
    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: plan_id.to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 500 },
        package: "base@0.2.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 600 },
        package: "mid@0.2.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "version conflict".to_string(),
        },
        package: "top@0.2.0".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::PartialFailure,
        },
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Verify loaded state matches expectations
    let loaded = store.load_state().expect("load").expect("exists");
    let published: Vec<&str> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Published))
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(published.len(), 2);

    let failed: Vec<&str> = loaded
        .packages
        .values()
        .filter(|p| matches!(p.state, PackageState::Failed { .. }))
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(failed, vec!["top"]);

    // Verify event log captured the partial failure
    let loaded_events = store.load_events().expect("load").expect("exists");
    assert_eq!(loaded_events.all_events().len(), 5);

    let fail_events: Vec<_> = loaded_events
        .all_events()
        .iter()
        .filter(|e| matches!(e.event_type, EventType::PackageFailed { .. }))
        .collect();
    assert_eq!(fail_events.len(), 1);
}

// ===========================================================================
// 30. Plan filtering: --package flag produces correct subset plan
// ===========================================================================

#[test]
fn plan_filtering_single_package_no_deps() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    // Select only "base" — no transitive deps needed since base has none
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: Some(vec!["base".to_string()]),
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    assert_eq!(ws.plan.packages.len(), 1);
    assert_eq!(ws.plan.packages[0].name, "base");
}

#[test]
fn plan_filtering_mid_package_pulls_base_dep() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    // Select "mid" — should pull in base as transitive dep
    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: Some(vec!["mid".to_string()]),
    };
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"base"), "base should be included as dep");
    assert!(names.contains(&"mid"), "mid should be included");
    assert!(!names.contains(&"top"), "top should NOT be included");
    assert_eq!(names.len(), 2);

    // Verify ordering: base before mid
    let base_pos = names.iter().position(|&n| n == "base").unwrap();
    let mid_pos = names.iter().position(|&n| n == "mid").unwrap();
    assert!(base_pos < mid_pos, "base must precede mid");
}

// ===========================================================================
// 31. Readiness verification: mock registry responds to version check
// ===========================================================================

#[test]
fn readiness_version_visible_after_publish_mock() {
    // Mock server that returns 404 on first request, 200 on second (simulating propagation)
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    let handler = std::thread::spawn(move || {
        // First request: version not yet visible
        if let Ok(req) = server.recv() {
            let _ =
                req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
        // Second request: version now visible
        if let Ok(req) = server.recv() {
            let body = r#"{"version":{"num":"0.2.0"}}"#;
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

    // First check: not visible
    let visible1 = client.version_exists("my-crate", "0.2.0").expect("check 1");
    assert!(!visible1, "should not be visible on first check");

    // Second check: now visible
    let visible2 = client.version_exists("my-crate", "0.2.0").expect("check 2");
    assert!(visible2, "should be visible on second check");

    handler.join().expect("handler thread");
}

// ===========================================================================
// 32. Event log: verify events.jsonl has correct entries after publish sim
// ===========================================================================

#[test]
fn event_log_jsonl_file_has_correct_structure() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let events_path = shipper::state::events::events_path(&state_dir);
    let plan_id = "jsonl-structure-test";

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
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "alpha@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p alpha".to_string(),
        },
        package: "alpha@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 750 },
        package: "alpha@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });

    log.write_to_file(&events_path).expect("write events");

    // Read raw file and verify it's valid JSONL (one JSON object per line)
    let raw = fs::read_to_string(&events_path).expect("read raw");
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 6, "should have 6 JSONL lines");

    // Each line should be parseable as JSON
    for (i, line) in lines.iter().enumerate() {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i} invalid JSON: {e}"));
        // Each event should have timestamp, event_type, and package fields
        assert!(
            parsed.get("timestamp").is_some(),
            "line {i} missing timestamp"
        );
        assert!(
            parsed.get("event_type").is_some(),
            "line {i} missing event_type"
        );
        assert!(parsed.get("package").is_some(), "line {i} missing package");
    }

    // Reload via EventLog and verify roundtrip
    let loaded = EventLog::read_from_file(&events_path).expect("read events");
    assert_eq!(loaded.all_events().len(), 6);
}

// ===========================================================================
// 33. Receipt generation: verify receipt.json has all expected fields
// ===========================================================================

#[test]
fn receipt_all_fields_persisted_and_roundtrip() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = shipper::types::Receipt {
        receipt_version: state::CURRENT_RECEIPT_VERSION.to_string(),
        plan_id: "receipt-fields-test".to_string(),
        registry: Registry::crates_io(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        packages: vec![
            PackageReceipt {
                name: "alpha".to_string(),
                version: "1.0.0".to_string(),
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
                name: "beta".to_string(),
                version: "2.0.0".to_string(),
                attempts: 2,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 3400,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            },
        ],
        event_log_path: std::path::PathBuf::from(".shipper/events.jsonl"),
        git_context: Some(shipper::types::GitContext {
            commit: Some("abc123def456".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v1.0.0".to_string()),
            dirty: Some(false),
        }),
        environment: EnvironmentFingerprint {
            shipper_version: "0.5.0".to_string(),
            cargo_version: Some("1.85.0".to_string()),
            rust_version: Some("1.85.0".to_string()),
            os: "linux".to_string(),
            arch: "aarch64".to_string(),
        },
    };

    state::write_receipt(&state_dir, &receipt).expect("write receipt");

    // Load receipt and verify all fields
    let loaded = state::load_receipt(&state_dir)
        .expect("load receipt")
        .expect("receipt exists");

    assert_eq!(loaded.receipt_version, state::CURRENT_RECEIPT_VERSION);
    assert_eq!(loaded.plan_id, "receipt-fields-test");
    assert_eq!(loaded.registry.name, "crates-io");
    assert_eq!(loaded.packages.len(), 2);

    // Verify package details
    assert_eq!(loaded.packages[0].name, "alpha");
    assert_eq!(loaded.packages[0].version, "1.0.0");
    assert_eq!(loaded.packages[0].attempts, 1);
    assert_eq!(loaded.packages[0].duration_ms, 1200);
    assert!(matches!(loaded.packages[0].state, PackageState::Published));

    assert_eq!(loaded.packages[1].name, "beta");
    assert_eq!(loaded.packages[1].version, "2.0.0");
    assert_eq!(loaded.packages[1].attempts, 2);
    assert_eq!(loaded.packages[1].duration_ms, 3400);

    // Verify git context
    let git = loaded.git_context.expect("git context present");
    assert_eq!(git.commit.as_deref(), Some("abc123def456"));
    assert_eq!(git.branch.as_deref(), Some("main"));
    assert_eq!(git.tag.as_deref(), Some("v1.0.0"));
    assert_eq!(git.dirty, Some(false));

    // Verify environment
    assert_eq!(loaded.environment.shipper_version, "0.5.0");
    assert_eq!(loaded.environment.cargo_version.as_deref(), Some("1.85.0"));
    assert_eq!(loaded.environment.rust_version.as_deref(), Some("1.85.0"));
    assert_eq!(loaded.environment.os, "linux");
    assert_eq!(loaded.environment.arch, "aarch64");

    // Verify event log path
    assert_eq!(
        loaded.event_log_path,
        std::path::PathBuf::from(".shipper/events.jsonl")
    );
}

// ===========================================================================
// 34. Receipt raw JSON has all expected top-level keys
// ===========================================================================

#[test]
fn receipt_json_file_has_expected_keys() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let receipt = sample_receipt("json-keys-test", &["base", "mid"]);
    state::write_receipt(&state_dir, &receipt).expect("write");

    let receipt_file = state::receipt_path(&state_dir);
    let raw = fs::read_to_string(&receipt_file).expect("read raw");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse JSON");

    // Verify all expected top-level keys
    let expected_keys = [
        "receipt_version",
        "plan_id",
        "registry",
        "started_at",
        "finished_at",
        "packages",
        "event_log_path",
        "environment",
    ];
    for key in &expected_keys {
        assert!(
            parsed.get(key).is_some(),
            "receipt JSON missing expected key: {key}"
        );
    }

    // Verify package sub-structure
    let pkgs = parsed["packages"].as_array().expect("packages is array");
    assert_eq!(pkgs.len(), 2);

    let pkg_keys = [
        "name",
        "version",
        "attempts",
        "state",
        "started_at",
        "finished_at",
        "duration_ms",
        "evidence",
    ];
    for pkg in pkgs {
        for key in &pkg_keys {
            assert!(pkg.get(key).is_some(), "package JSON missing key: {key}");
        }
    }
}

// ===========================================================================
// 35. State persistence with ambiguous package state
// ===========================================================================

#[test]
fn state_persists_ambiguous_package_state() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");

    let exec_state = sample_state(
        "ambiguous-test",
        &[
            ("lib-a", "1.0.0", PackageState::Published, 1),
            (
                "lib-b",
                "1.0.0",
                PackageState::Ambiguous {
                    message: "timeout waiting for visibility".to_string(),
                },
                2,
            ),
        ],
    );
    state::save_state(&state_dir, &exec_state).expect("save");

    let loaded = state::load_state(&state_dir)
        .expect("load")
        .expect("exists");

    let lib_b = loaded.packages.get("lib-b@1.0.0").expect("lib-b exists");
    match &lib_b.state {
        PackageState::Ambiguous { message } => {
            assert_eq!(message, "timeout waiting for visibility");
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    assert_eq!(lib_b.attempts, 2);
}

// ===========================================================================
// 36. Event log per-package filtering returns correct events
// ===========================================================================

#[test]
fn event_log_per_package_filtering_correct() {
    let mut log = EventLog::new();

    // Record events for multiple packages
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: "filter-test".to_string(),
            package_count: 3,
        },
        package: "all".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
        },
        package: "alpha@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 100 },
        package: "alpha@1.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "beta".to_string(),
            version: "2.0.0".to_string(),
        },
        package: "beta@2.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "timeout".to_string(),
        },
        package: "beta@2.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageAttempted {
            attempt: 2,
            command: "cargo publish -p beta".to_string(),
        },
        package: "beta@2.0.0".to_string(),
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackagePublished { duration_ms: 200 },
        package: "beta@2.0.0".to_string(),
    });

    // Filter by package
    let alpha_events = log.events_for_package("alpha@1.0.0");
    assert_eq!(alpha_events.len(), 2);
    assert!(matches!(
        alpha_events[0].event_type,
        EventType::PackageStarted { .. }
    ));
    assert!(matches!(
        alpha_events[1].event_type,
        EventType::PackagePublished { .. }
    ));

    let beta_events = log.events_for_package("beta@2.0.0");
    assert_eq!(beta_events.len(), 4); // started + failed + attempted + published

    let global_events = log.events_for_package("all");
    assert_eq!(global_events.len(), 1);

    // Non-existent package returns empty
    let ghost = log.events_for_package("ghost@0.0.0");
    assert!(ghost.is_empty());
}

// ===========================================================================
// 37. Lock prevents double acquire
// ===========================================================================

#[test]
#[allow(unused_mut)]
fn lock_double_acquire_fails() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");

    let mut lock = shipper::lock::LockFile::acquire(&state_dir, None).expect("first acquire");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("locked"));

    // Second acquire should fail
    let result = shipper::lock::LockFile::acquire(&state_dir, None);
    assert!(result.is_err(), "double acquire should fail");

    // Release first lock
    lock.release().expect("release");
    assert!(!shipper::lock::LockFile::is_locked(&state_dir, None).expect("unlocked"));

    // Now a new acquire should succeed
    let mut lock2 = shipper::lock::LockFile::acquire(&state_dir, None).expect("re-acquire");
    assert!(shipper::lock::LockFile::is_locked(&state_dir, None).expect("locked again"));
    lock2.release().expect("release2");
}

// ===========================================================================
// 38. Full lifecycle: plan → state → events → receipt through FileStore
// ===========================================================================

#[test]
fn full_lifecycle_plan_events_receipt_through_store() {
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

    let store_dir = td.path().join("store");
    let store = FileStore::new(store_dir);

    // Initialize state from plan with all pending
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
        packages: packages.clone(),
    };
    store.save_state(&exec_state).expect("save initial state");

    // Build event log for the full publish
    let mut events = EventLog::new();
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PlanCreated {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    for pkg in &ws.plan.packages {
        events.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
            },
            package: format!("{}@{}", pkg.name, pkg.version),
        });
        events.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 500 },
            package: format!("{}@{}", pkg.name, pkg.version),
        });
    }
    events.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        package: "all".to_string(),
    });
    store.save_events(&events).expect("save events");

    // Update state to all published
    let mut final_packages = BTreeMap::new();
    for pkg in &ws.plan.packages {
        final_packages.insert(
            format!("{}@{}", pkg.name, pkg.version),
            PackageProgress {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
    }
    let final_exec_state = ExecutionState {
        state_version: state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: final_packages,
    };
    store
        .save_state(&final_exec_state)
        .expect("save final state");

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

    // Verify everything roundtrips correctly
    let loaded_state = store.load_state().expect("load").expect("exists");
    assert_eq!(loaded_state.plan_id, ws.plan.plan_id);
    assert!(
        loaded_state
            .packages
            .values()
            .all(|p| matches!(p.state, PackageState::Published))
    );

    let loaded_events = store.load_events().expect("load").expect("exists");
    // plan_created + exec_started + 3*(started + published) + exec_finished = 9
    assert_eq!(loaded_events.all_events().len(), 9);

    let loaded_receipt = store.load_receipt().expect("load").expect("exists");
    assert_eq!(loaded_receipt.plan_id, ws.plan.plan_id);
    assert_eq!(loaded_receipt.packages.len(), 3);
}

// ===========================================================================
// 39. Registry: multi-crate ownership check with mock
// ===========================================================================

#[test]
fn registry_multi_crate_ownership_check_mock() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start server");
    let addr = server.server_addr().to_ip().expect("addr");
    let api_base = format!("http://{}:{}", addr.ip(), addr.port());

    // Handler responds: first crate owned, second crate forbidden (not owned)
    let handler = std::thread::spawn(move || {
        // First request: owned crate
        if let Ok(req) = server.recv() {
            let body = r#"{"users":[{"id":1,"login":"dev","name":"Dev"}]}"#;
            let resp = tiny_http::Response::from_string(body).with_status_code(200);
            let _ = req.respond(resp);
        }
        // Second request: forbidden
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

    // First crate: ownership verified
    let owned = client
        .verify_ownership("my-crate", "valid-token")
        .expect("verify first");
    assert!(owned, "first crate should be owned");

    // Second crate: ownership denied
    let not_owned = client
        .verify_ownership("other-crate", "valid-token")
        .expect("verify second");
    assert!(!not_owned, "second crate should not be owned");

    handler.join().expect("handler thread");
}
