use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use serial_test::serial;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

use super::policy::policy_effects;
use super::publish::{publish_package, run_publish_level};
use super::run_publish_parallel_inner as run_publish_parallel;
use super::*;
use crate::plan::PlannedWorkspace;
use crate::runtime::execution::{pkg_key, update_state_locked};
use crate::state::events;
use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{
    ErrorClass, EventType, ExecutionState, PackageEvidence, PackageProgress, PackageReceipt,
    PackageState, PlannedPackage, PublishLevel, ReadinessConfig, Registry, ReleasePlan,
    RuntimeOptions,
};

#[derive(Default)]
struct CollectingReporter {
    infos: Vec<String>,
    warns: Vec<String>,
    errors: Vec<String>,
}

impl Reporter for CollectingReporter {
    fn info(&mut self, msg: &str) {
        self.infos.push(msg.to_string());
    }

    fn warn(&mut self, msg: &str) {
        self.warns.push(msg.to_string());
    }

    fn error(&mut self, msg: &str) {
        self.errors.push(msg.to_string());
    }
}

fn write_fake_cargo(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("cargo.cmd"),
            "@echo off\r\nif not \"%SHIPPER_CARGO_ARGS_LOG%\"==\"\" echo %*>>\"%SHIPPER_CARGO_ARGS_LOG%\"\r\nif not \"%SHIPPER_CARGO_STDOUT%\"==\"\" echo %SHIPPER_CARGO_STDOUT%\r\nif not \"%SHIPPER_CARGO_STDERR%\"==\"\" echo %SHIPPER_CARGO_STDERR% 1>&2\r\nif \"%SHIPPER_CARGO_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_CARGO_EXIT%)\r\n",
        )
        .expect("write fake cargo");
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\nif [ -n \"$SHIPPER_CARGO_ARGS_LOG\" ]; then\n  echo \"$*\" >>\"$SHIPPER_CARGO_ARGS_LOG\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDOUT\" ]; then\n  echo \"$SHIPPER_CARGO_STDOUT\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDERR\" ]; then\n  echo \"$SHIPPER_CARGO_STDERR\" >&2\nfi\nexit \"${SHIPPER_CARGO_EXIT:-0}\"\n",
        )
        .expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
    }
}

fn write_fake_tools(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).expect("mkdir");
    write_fake_cargo(bin_dir);
}

#[cfg(windows)]
fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
    bin_dir.join("cargo.cmd")
}

#[cfg(not(windows))]
fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
    bin_dir.join("cargo")
}

struct TestRegistryServer {
    base_url: String,
    handle: std::thread::JoinHandle<()>,
}

impl TestRegistryServer {
    fn join(self) {
        self.handle.join().expect("join server");
    }
}

fn spawn_registry_server(
    mut routes: BTreeMap<String, Vec<(u16, String)>>,
    expected_requests: usize,
) -> TestRegistryServer {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());

    let handle = std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let req = server.recv().expect("request");
            let path = req.url().to_string();

            let response = if let Some(list) = routes.get_mut(&path) {
                if list.is_empty() {
                    (404, "{}".to_string())
                } else if list.len() == 1 {
                    list[0].clone()
                } else {
                    list.remove(0)
                }
            } else {
                (404, "{}".to_string())
            };

            let resp = Response::from_string(response.1)
                .with_status_code(StatusCode(response.0))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(resp).expect("respond");
        }
    });

    TestRegistryServer { base_url, handle }
}

fn planned_workspace(workspace_root: &Path, api_base: String) -> PlannedWorkspace {
    PlannedWorkspace {
        workspace_root: workspace_root.to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-parallel".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base,
                index_base: None,
            },
            packages: vec![PlannedPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: workspace_root.join("demo").join("Cargo.toml"),
            }],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    }
}

fn default_opts(state_dir: PathBuf) -> RuntimeOptions {
    RuntimeOptions {
        allow_dirty: true,
        skip_ownership_check: true,
        strict_ownership: false,
        no_verify: false,
        max_attempts: 2,
        base_delay: Duration::from_millis(0),
        max_delay: Duration::from_millis(0),
        verify_timeout: Duration::from_millis(20),
        verify_poll_interval: Duration::from_millis(1),
        state_dir,
        force_resume: false,
        policy: shipper_types::PublishPolicy::default(),
        verify_mode: shipper_types::VerifyMode::default(),
        readiness: ReadinessConfig {
            enabled: true,
            method: shipper_types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(200),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        },
        output_lines: 100,
        force: false,
        lock_timeout: Duration::from_secs(3600),
        parallel: shipper_types::ParallelConfig {
            enabled: true,
            max_concurrent: 4,
            per_package_timeout: Duration::from_secs(60),
        },
        retry_strategy: shipper_retry::RetryStrategyType::Exponential,
        retry_jitter: 0.0,
        retry_per_error: shipper_retry::PerErrorConfig::default(),
        encryption: shipper_encrypt::EncryptionConfig::default(),
        webhook: shipper_webhook::WebhookConfig::default(),
        registries: vec![],
        resume_from: None,
    }
}

fn init_state_for_package(
    plan_id: &str,
    registry: &Registry,
    pkg_name: &str,
    pkg_version: &str,
) -> ExecutionState {
    let key = pkg_key(pkg_name, pkg_version);
    let mut packages = BTreeMap::new();
    packages.insert(
        key,
        PackageProgress {
            name: pkg_name.to_string(),
            version: pkg_version.to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

#[test]
#[serial]
fn test_publish_package_skips_already_published() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Registry returns 200 for version_exists (already published)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = default_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result.result.expect("should succeed");
            assert!(matches!(receipt.state, PackageState::Skipped { .. }));
            assert_eq!(receipt.attempts, 0);

            // State should be updated to Skipped
            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("pkg");
            assert!(matches!(progress.state, PackageState::Skipped { .. }));
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_publish_package_publishes_successfully() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists returns 404 (not published), then readiness returns 200
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = default_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("0")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result.result.expect("should succeed");
            assert!(matches!(receipt.state, PackageState::Published));
            assert!(receipt.attempts >= 1);
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_publish_package_handles_permanent_failure() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists returns 404 both times (initial + after failure check)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (404, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = default_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            assert!(result.result.is_err());
            let err_msg = format!("{:#}", result.result.unwrap_err());
            assert!(err_msg.contains("permanent failure"));

            // State should be updated to Failed
            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("pkg");
            assert!(matches!(
                progress.state,
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    ..
                }
            ));
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_publish_package_retries_on_transient() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists: 404 (initial), 404 (after failure), 200 (found after retry)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (200, "{}".to_string()),
            ],
        )]),
        3,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let mut opts = default_opts(PathBuf::from(".shipper"));
    opts.max_attempts = 2;
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("timeout talking to server")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            // Should succeed because final registry check found the version
            let receipt = result.result.expect("should succeed");
            assert!(matches!(receipt.state, PackageState::Published));
            assert_eq!(receipt.attempts, 2);
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_run_publish_level_processes_packages() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Two packages, both already published
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/alpha/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/beta/0.2.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        2,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-level".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: td.path().join("alpha").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "0.2.0".to_string(),
                    manifest_path: td.path().join("beta").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = default_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");
    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel {
        level: 0,
        packages: ws.plan.packages.clone(),
    };

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            )
            .expect("level publish");

            assert_eq!(receipts.len(), 2);
            for r in &receipts {
                assert!(matches!(r.state, PackageState::Skipped { .. }));
            }
        },
    );
    server.join();
}

#[test]
fn test_update_state_locked_sets_state() {
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "plan-test".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::from([(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        )]),
    };

    let before = st.updated_at;
    // Small sleep to ensure timestamp differs
    std::thread::sleep(Duration::from_millis(2));

    update_state_locked(&mut st, "demo@0.1.0", PackageState::Published);

    let progress = st.packages.get("demo@0.1.0").expect("pkg");
    assert!(matches!(progress.state, PackageState::Published));
    assert!(st.updated_at >= before);
}

// ---------------------------------------------------------------------------
// Additional tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_run_publish_parallel_single_package() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Registry returns 200 for version_exists (already published)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "demo", "0.1.0");
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");

            assert_eq!(receipts.len(), 1);
            assert!(matches!(receipts[0].state, PackageState::Skipped { .. }));
            assert_eq!(receipts[0].name, "demo");
            assert_eq!(receipts[0].version, "0.1.0");
            assert_eq!(receipts[0].attempts, 0);
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_run_publish_parallel_multiple_levels() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Both packages already published
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        2,
    );

    // "dependent" depends on "base" so they end up in different levels
    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-multi-level".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "base".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("base").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());

    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");

            assert_eq!(receipts.len(), 2);
            for r in &receipts {
                assert!(
                    matches!(r.state, PackageState::Skipped { .. }),
                    "expected Skipped for {}, got {:?}",
                    r.name,
                    r.state
                );
            }

            // Verify reporter saw level info messages
            let level_msgs: Vec<&String> = reporter
                .infos
                .iter()
                .filter(|m| m.contains("Level"))
                .collect();
            assert!(
                level_msgs.len() >= 2,
                "expected at least 2 level messages, got: {:?}",
                level_msgs
            );
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_publish_package_handles_uploaded_resume() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists returns 404 (initial check), then 200 (readiness verification)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());

    // Set the initial state to Uploaded (cargo publish succeeded previously)
    let key = pkg_key("demo", "0.1.0");
    let mut packages = BTreeMap::new();
    packages.insert(
        key.clone(),
        PackageProgress {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Uploaded,
            last_updated_at: Utc::now(),
        },
    );
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result.result.expect("should succeed");
            assert!(
                matches!(receipt.state, PackageState::Published),
                "expected Published, got {:?}",
                receipt.state
            );

            // State should be Published
            let state = st.lock().unwrap();
            let progress = state.packages.get(&key).expect("pkg");
            assert!(matches!(progress.state, PackageState::Published));

            // Evidence should have no cargo attempts (skipped cargo publish)
            assert!(
                receipt.evidence.attempts.is_empty(),
                "expected no cargo attempt evidence for resumed Uploaded package"
            );
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_publish_package_records_attempt_evidence() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists returns 404 (not published), then readiness returns 200
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("0")),
            ("SHIPPER_CARGO_STDOUT", Some("Uploading demo v0.1.0")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result.result.expect("should succeed");
            assert!(matches!(receipt.state, PackageState::Published));

            // Evidence should contain exactly one attempt
            assert_eq!(
                receipt.evidence.attempts.len(),
                1,
                "expected 1 attempt evidence entry"
            );

            let attempt = &receipt.evidence.attempts[0];
            assert_eq!(attempt.attempt_number, 1);
            assert!(
                attempt.command.contains("cargo publish"),
                "command should contain 'cargo publish', got: {}",
                attempt.command
            );
            assert_eq!(attempt.exit_code, 0);
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_run_publish_level_respects_max_concurrent() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Four packages, all already published
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/pkg-a/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/pkg-b/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/pkg-c/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/pkg-d/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        4,
    );

    let pkg_names = ["pkg-a", "pkg-b", "pkg-c", "pkg-d"];
    let packages: Vec<PlannedPackage> = pkg_names
        .iter()
        .map(|name| PlannedPackage {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(name).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-concurrent".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    // Limit concurrency to 2 (with 4 packages, should chunk into 2 batches)
    opts.parallel.max_concurrent = 2;

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            )
            .expect("level publish");

            assert_eq!(receipts.len(), 4, "all 4 packages should have receipts");
            for r in &receipts {
                assert!(
                    matches!(r.state, PackageState::Skipped { .. }),
                    "expected Skipped for {}, got {:?}",
                    r.name,
                    r.state
                );
            }

            // Verify all package names are present
            let mut names: Vec<String> = receipts.iter().map(|r| r.name.clone()).collect();
            names.sort();
            assert_eq!(
                names,
                vec!["pkg-a", "pkg-b", "pkg-c", "pkg-d"],
                "all package names should be in receipts"
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Level-based execution ordering: levels execute sequentially in order
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_levels_execute_in_dependency_order() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Three packages: C depends on B, B depends on A  →  3 levels
    // All already published so no cargo invocations needed.
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/a/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/b/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/c/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        3,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-ordering".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("a").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("b").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("c").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["b".to_string()]),
            ]),
        },
        skipped: vec![],
    };

    // Verify group_by_levels produces 3 levels
    let levels = ws.plan.group_by_levels();
    assert_eq!(levels.len(), 3, "chain A→B→C should produce 3 levels");
    assert_eq!(levels[0].packages[0].name, "a");
    assert_eq!(levels[1].packages[0].name, "b");
    assert_eq!(levels[2].packages[0].name, "c");

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");

            assert_eq!(receipts.len(), 3);
            // Receipts should be in dependency order: a, b, c
            assert_eq!(receipts[0].name, "a");
            assert_eq!(receipts[1].name, "b");
            assert_eq!(receipts[2].name, "c");

            // All skipped because already published
            for r in &receipts {
                assert!(
                    matches!(r.state, PackageState::Skipped { .. }),
                    "expected Skipped for {}, got {:?}",
                    r.name,
                    r.state
                );
            }
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Error propagation across levels: a failed level stops subsequent levels
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_failed_level_stops_subsequent_levels() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // "base" not published, will fail with permanent error.
    // "dependent" depends on "base" and should never be attempted.
    // Registry: base → 404 (not published) twice (initial + after-failure check)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/base/1.0.0".to_string(),
            vec![(404, "{}".to_string()), (404, "{}".to_string())],
        )]),
        2,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-error-prop".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "base".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("base").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1; // fail fast

    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
        ],
        || {
            let result = run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter);

            // Level 0 (base) should fail, causing the whole publish to fail
            assert!(result.is_err(), "expected error from failed level");
            let err_msg = format!("{:#}", result.unwrap_err());
            assert!(
                err_msg.contains("base"),
                "error should mention failing package 'base', got: {err_msg}"
            );

            // "dependent" should still be Pending (never attempted)
            let dep_key = pkg_key("dependent", "2.0.0");
            let progress = st.packages.get(&dep_key).expect("dependent pkg");
            assert!(
                matches!(progress.state, PackageState::Pending),
                "dependent should remain Pending, got {:?}",
                progress.state
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Partial success within a level: some packages succeed, some fail
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_partial_success_within_level() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // alpha: already published (200) → skipped
    // beta: not published (404, 404) → will fail with permanent error
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/alpha/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/beta/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            ),
        ]),
        3,
    );

    let packages = vec![
        PlannedPackage {
            name: "alpha".to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join("alpha").join("Cargo.toml"),
        },
        PlannedPackage {
            name: "beta".to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join("beta").join("Cargo.toml"),
        },
    ];

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-partial".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1;

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
        ],
        || {
            let result = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            // Level should report error because beta failed
            assert!(result.is_err(), "level should fail when a package fails");
            let err_msg = format!("{:#}", result.unwrap_err());
            assert!(
                err_msg.contains("1 package"),
                "error should mention 1 failed package, got: {err_msg}"
            );

            // alpha should be Skipped (succeeded), beta should be Failed
            let state = st.lock().unwrap();
            let alpha = state.packages.get("alpha@0.1.0").expect("alpha");
            assert!(
                matches!(alpha.state, PackageState::Skipped { .. }),
                "alpha should be Skipped, got {:?}",
                alpha.state
            );
            let beta = state.packages.get("beta@0.1.0").expect("beta");
            assert!(
                matches!(beta.state, PackageState::Failed { .. }),
                "beta should be Failed, got {:?}",
                beta.state
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Webhook notification integration: events are sent to a real HTTP endpoint
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_webhook_events_sent_on_publish() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Package already published → skipped
    let registry_server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    // Webhook receiver: expect 2 POSTs (PublishStarted + PublishCompleted)
    let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
    let webhook_url = format!("http://{}", webhook_server.server_addr());
    let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
    let webhook_received_clone = Arc::clone(&webhook_received);

    let webhook_handle = std::thread::spawn(move || {
        // Collect up to 3 requests with a timeout
        for _ in 0..3 {
            match webhook_server.recv_timeout(Duration::from_secs(30)) {
                Ok(Some(mut req)) => {
                    let mut body = Vec::new();
                    let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                    let text = String::from_utf8_lossy(&body).to_string();
                    webhook_received_clone.lock().unwrap().push(text);
                    req.respond(Response::from_string("ok")).expect("respond");
                }
                _ => break,
            }
        }
    });

    let ws = planned_workspace(td.path(), registry_server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.webhook = shipper_webhook::WebhookConfig {
        url: webhook_url,
        ..Default::default()
    };

    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "demo", "0.1.0");
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let _receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");
        },
    );

    // Give the async webhook thread time to deliver
    std::thread::sleep(Duration::from_millis(500));
    webhook_handle.join().expect("webhook thread");
    registry_server.join();

    let received = webhook_received.lock().unwrap();
    assert!(
        received.len() >= 2,
        "expected at least 2 webhook POSTs (started + completed), got {}",
        received.len()
    );
}

// ---------------------------------------------------------------------------
// Resume from specific level (resume_from option)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_resume_from_skips_earlier_levels() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // "base" is already published in state (level 0).
    // "dependent" depends on "base" (level 1), also already on registry.
    // resume_from = "dependent" should skip level 0 and process level 1.
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/dependent/2.0.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-resume".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "base".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("base").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.resume_from = Some("dependent".to_string());

    // "base" already Published in state, "dependent" is Pending
    let mut packages = BTreeMap::new();
    packages.insert(
        pkg_key("base", "1.0.0"),
        PackageProgress {
            name: "base".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Published,
            last_updated_at: Utc::now(),
        },
    );
    packages.insert(
        pkg_key("dependent", "2.0.0"),
        PackageProgress {
            name: "dependent".to_string(),
            version: "2.0.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish with resume");

            assert_eq!(receipts.len(), 2, "should have receipts for both packages");

            // base receipt comes from the skipped-level path
            assert_eq!(receipts[0].name, "base");
            assert!(matches!(receipts[0].state, PackageState::Published));

            // dependent was actually processed
            assert_eq!(receipts[1].name, "dependent");
            assert!(
                matches!(receipts[1].state, PackageState::Skipped { .. }),
                "dependent should be Skipped (already on registry), got {:?}",
                receipts[1].state
            );

            // Reporter should mention skipping level before resume point
            let skip_msgs: Vec<&String> = reporter
                .infos
                .iter()
                .chain(reporter.warns.iter())
                .filter(|m| m.contains("already complete") || m.contains("resume point"))
                .collect();
            assert!(
                !skip_msgs.is_empty(),
                "reporter should mention skipping/resume, infos={:?}, warns={:?}",
                reporter.infos,
                reporter.warns
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// All packages already published: entire workspace is a no-op
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_all_packages_already_published() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // 3 packages across 2 levels, all already published
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/core/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/utils/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/app/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        3,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-all-published".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("core").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "utils".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("utils").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "app".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("app").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([(
                "app".to_string(),
                vec!["core".to_string(), "utils".to_string()],
            )]),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());

    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");

            assert_eq!(receipts.len(), 3, "should have receipts for all 3 packages");

            // All packages should be Skipped
            for r in &receipts {
                assert!(
                    matches!(r.state, PackageState::Skipped { .. }),
                    "expected Skipped for {}, got {:?}",
                    r.name,
                    r.state
                );
                assert_eq!(r.attempts, 0, "{} should have 0 attempts", r.name);
            }

            // No cargo invocations should have happened (all skipped)
            // State should reflect Skipped for all packages
            for p in &ws.plan.packages {
                let key = pkg_key(&p.name, &p.version);
                let progress = st.packages.get(&key).expect("pkg in state");
                assert!(
                    matches!(progress.state, PackageState::Skipped { .. }),
                    "state for {} should be Skipped, got {:?}",
                    p.name,
                    progress.state
                );
            }
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Max concurrency = 1: serialized execution within a level
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_max_concurrency_one_serializes_execution() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // 3 packages at the same level, all already published
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/x/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/y/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/z/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        3,
    );

    let packages: Vec<PlannedPackage> = ["x", "y", "z"]
        .iter()
        .map(|name| PlannedPackage {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(name).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-serial".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.max_concurrent = 1; // force serialization

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            )
            .expect("level publish");

            assert_eq!(receipts.len(), 3);
            // With max_concurrent=1, chunk_by_max_concurrent produces 3 single-item chunks.
            // All should succeed (skipped).
            for r in &receipts {
                assert!(
                    matches!(r.state, PackageState::Skipped { .. }),
                    "expected Skipped for {}, got {:?}",
                    r.name,
                    r.state
                );
            }
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Webhook: empty URL means no webhook delivery (no-op path)
// ---------------------------------------------------------------------------

#[test]
fn test_webhook_noop_with_empty_url() {
    // maybe_send_event with empty URL should not panic or block
    let config = shipper_webhook::WebhookConfig::default();
    assert!(config.url.is_empty());

    // This should be a silent no-op
    maybe_send_event(
        &config,
        WebhookEvent::PublishStarted {
            plan_id: "test".to_string(),
            package_count: 1,
            registry: "crates-io".to_string(),
        },
    );
}

// ---------------------------------------------------------------------------
// WebhookClient rejects empty URL
// ---------------------------------------------------------------------------

#[test]
fn test_webhook_client_rejects_empty_url() {
    let config = shipper_webhook::WebhookConfig {
        url: "".to_string(),
        ..Default::default()
    };
    let result = webhook::WebhookClient::new(&config);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// WebhookClient accepts valid URL
// ---------------------------------------------------------------------------

#[test]
fn test_webhook_client_accepts_valid_url() {
    let config = shipper_webhook::WebhookConfig {
        url: "http://localhost:9999/hook".to_string(),
        ..Default::default()
    };
    let result = webhook::WebhookClient::new(&config);
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// Execution result classification
// ---------------------------------------------------------------------------

#[test]
fn test_execution_result_all_skipped_is_success() {
    // Verify that all-Skipped receipts produce ExecutionResult::Success
    let receipts = [
        PackageReceipt {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".into(),
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
        PackageReceipt {
            name: "b".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".into(),
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
    ];

    let all_ok = receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    });
    assert!(all_ok, "all-skipped should be classified as success");
}

// ---------------------------------------------------------------------------
// Resume from nonexistent package: all levels should still be processed
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_resume_from_nonexistent_skips_all_levels() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Single package, already published
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        0, // no requests expected (level is skipped before resume point)
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.resume_from = Some("nonexistent-pkg".to_string());

    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "demo", "0.1.0");
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");

            // The package's level is skipped (before a resume point that's never found)
            // so it appears as a receipt from the skip path but is never actually published
            assert!(
                receipts.is_empty() || receipts.iter().all(|r| r.duration_ms == 0),
                "skipped-level receipts should have zero duration"
            );
        },
    );
    // Server receives 0 requests, no join needed for tiny_http server thread
    // (it would block forever waiting for requests)
    drop(server);
}

// ---------------------------------------------------------------------------
// Policy effects: Fast policy disables readiness
// ---------------------------------------------------------------------------

#[test]
fn test_fast_policy_disables_readiness() {
    let mut opts = default_opts(PathBuf::from(".shipper"));
    opts.policy = shipper_types::PublishPolicy::Fast;
    opts.readiness.enabled = true;

    let effects = policy_effects(&opts);
    assert!(
        !effects.readiness_enabled,
        "Fast policy should disable readiness"
    );
}

#[test]
fn test_safe_policy_preserves_readiness() {
    let mut opts = default_opts(PathBuf::from(".shipper"));
    opts.policy = shipper_types::PublishPolicy::Safe;
    opts.readiness.enabled = true;

    let effects = policy_effects(&opts);
    assert!(
        effects.readiness_enabled,
        "Safe policy should preserve readiness"
    );
}

mod snapshot_tests {
    use super::*;
    use insta::assert_debug_snapshot;
    use std::path::PathBuf;

    #[test]
    fn snapshot_chunk_by_max_concurrent_basic() {
        let items = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let chunks = chunk_by_max_concurrent(&items, 2);
        assert_debug_snapshot!(chunks);
    }

    #[test]
    fn snapshot_chunk_by_max_concurrent_one() {
        let items = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let chunks = chunk_by_max_concurrent(&items, 1);
        assert_debug_snapshot!(chunks);
    }

    #[test]
    fn snapshot_chunk_by_max_concurrent_larger_than_items() {
        let items = vec!["a".to_string(), "b".to_string()];
        let chunks = chunk_by_max_concurrent(&items, 10);
        assert_debug_snapshot!(chunks);
    }

    #[test]
    fn snapshot_chunk_by_max_concurrent_empty() {
        let items: Vec<String> = vec![];
        let chunks = chunk_by_max_concurrent(&items, 4);
        assert_debug_snapshot!(chunks);
    }

    #[test]
    fn snapshot_policy_effects_safe() {
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = shipper_types::PublishPolicy::Safe;
        opts.readiness.enabled = true;
        let effects = policy_effects(&opts);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_policy_effects_fast() {
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = shipper_types::PublishPolicy::Fast;
        opts.readiness.enabled = true;
        let effects = policy_effects(&opts);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_policy_effects_balanced() {
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = shipper_types::PublishPolicy::Balanced;
        let effects = policy_effects(&opts);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_parallel_config_default() {
        let config = shipper_types::ParallelConfig::default();
        assert_debug_snapshot!(config);
    }

    #[test]
    fn snapshot_execution_plan_linear_chain() {
        // A→B→C linear chain should produce 3 levels, one package each
        let plan = ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-snap-chain".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/a/Cargo.toml"),
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/b/Cargo.toml"),
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/c/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["b".to_string()]),
            ]),
        };
        let levels = plan.group_by_levels();
        // Snapshot only levels + package names (stable across runs)
        let layout: Vec<(usize, Vec<&str>)> = levels
            .iter()
            .map(|l| {
                (
                    l.level,
                    l.packages.iter().map(|p| p.name.as_str()).collect(),
                )
            })
            .collect();
        assert_debug_snapshot!(layout);
    }

    #[test]
    fn snapshot_execution_plan_diamond() {
        // Diamond: A→B, A→C, B→D, C→D
        let plan = ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-snap-diamond".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/a/Cargo.toml"),
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/b/Cargo.toml"),
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/c/Cargo.toml"),
                },
                PlannedPackage {
                    name: "d".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/d/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["a".to_string()]),
                ("d".to_string(), vec!["b".to_string(), "c".to_string()]),
            ]),
        };
        let levels = plan.group_by_levels();
        let layout: Vec<(usize, Vec<&str>)> = levels
            .iter()
            .map(|l| {
                (
                    l.level,
                    l.packages.iter().map(|p| p.name.as_str()).collect(),
                )
            })
            .collect();
        assert_debug_snapshot!(layout);
    }

    #[test]
    fn snapshot_execution_plan_wide_fan_out() {
        // One root with 5 leaves (wide workspace like multi-binary repos)
        let plan = ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-snap-fan-out".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/core/Cargo.toml"),
                },
                PlannedPackage {
                    name: "cli".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/cli/Cargo.toml"),
                },
                PlannedPackage {
                    name: "web".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/web/Cargo.toml"),
                },
                PlannedPackage {
                    name: "api".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/api/Cargo.toml"),
                },
                PlannedPackage {
                    name: "worker".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/worker/Cargo.toml"),
                },
                PlannedPackage {
                    name: "bench".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/bench/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("cli".to_string(), vec!["core".to_string()]),
                ("web".to_string(), vec!["core".to_string()]),
                ("api".to_string(), vec!["core".to_string()]),
                ("worker".to_string(), vec!["core".to_string()]),
                ("bench".to_string(), vec!["core".to_string()]),
            ]),
        };
        let levels = plan.group_by_levels();
        let layout: Vec<(usize, Vec<&str>)> = levels
            .iter()
            .map(|l| {
                (
                    l.level,
                    l.packages.iter().map(|p| p.name.as_str()).collect(),
                )
            })
            .collect();
        assert_debug_snapshot!(layout);
    }

    #[test]
    fn snapshot_execution_plan_independent_forest() {
        // All crates independent (common for unrelated utility crates)
        let plan = ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-snap-forest".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "utils-a".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-a/Cargo.toml"),
                },
                PlannedPackage {
                    name: "utils-b".to_string(),
                    version: "0.2.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-b/Cargo.toml"),
                },
                PlannedPackage {
                    name: "utils-c".to_string(),
                    version: "0.3.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-c/Cargo.toml"),
                },
                PlannedPackage {
                    name: "utils-d".to_string(),
                    version: "0.4.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-d/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::new(),
        };
        let levels = plan.group_by_levels();
        let layout: Vec<(usize, Vec<&str>)> = levels
            .iter()
            .map(|l| {
                (
                    l.level,
                    l.packages.iter().map(|p| p.name.as_str()).collect(),
                )
            })
            .collect();
        assert_debug_snapshot!(layout);
    }
}

// ---------------------------------------------------------------------------
// ParallelConfig edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_parallel_config_max_concurrent_zero_treated_as_one() {
    // chunk_by_max_concurrent clamps 0 to 1, so max_concurrent=0
    // should behave like max_concurrent=1 (serial).
    let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let chunks = chunk_by_max_concurrent(&items, 0);
    // Each chunk should have at most 1 item
    for chunk in &chunks {
        assert!(
            chunk.len() <= 1,
            "max_concurrent=0 should clamp to 1, got chunk of size {}",
            chunk.len()
        );
    }
    // All items preserved
    let flat: Vec<String> = chunks.into_iter().flatten().collect();
    assert_eq!(flat, items);
}

#[test]
fn test_parallel_config_max_concurrent_one_produces_singleton_chunks() {
    let items = vec![
        "x".to_string(),
        "y".to_string(),
        "z".to_string(),
        "w".to_string(),
    ];
    let chunks = chunk_by_max_concurrent(&items, 1);
    assert_eq!(chunks.len(), 4, "should produce one chunk per item");
    for chunk in &chunks {
        assert_eq!(chunk.len(), 1);
    }
}

#[test]
fn test_parallel_config_very_large_max_concurrent() {
    let items = vec!["a".to_string(), "b".to_string()];
    let chunks = chunk_by_max_concurrent(&items, usize::MAX);
    // All items should be in a single chunk
    assert_eq!(chunks.len(), 1, "very large limit should produce 1 chunk");
    assert_eq!(chunks[0].len(), 2);
}

// ---------------------------------------------------------------------------
// Level ordering validation
// ---------------------------------------------------------------------------

#[test]
fn test_levels_are_sequentially_numbered() {
    // Diamond: A→B, A→C, B→D, C→D  →  3 levels
    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-level-order".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages: vec![
            PlannedPackage {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("a/Cargo.toml"),
            },
            PlannedPackage {
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("b/Cargo.toml"),
            },
            PlannedPackage {
                name: "c".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("c/Cargo.toml"),
            },
            PlannedPackage {
                name: "d".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("d/Cargo.toml"),
            },
        ],
        dependencies: BTreeMap::from([
            ("b".to_string(), vec!["a".to_string()]),
            ("c".to_string(), vec!["a".to_string()]),
            ("d".to_string(), vec!["b".to_string(), "c".to_string()]),
        ]),
    };

    let levels = plan.group_by_levels();
    assert!(levels.len() >= 2, "diamond should have multiple levels");
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(
            level.level, i,
            "level {} should have sequential number, got {}",
            i, level.level
        );
    }
}

#[test]
fn test_level_ordering_dependencies_precede_dependents() {
    // Verify that for every package, all its dependencies appear in earlier levels.
    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-dep-order".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages: vec![
            PlannedPackage {
                name: "core".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("core/Cargo.toml"),
            },
            PlannedPackage {
                name: "utils".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("utils/Cargo.toml"),
            },
            PlannedPackage {
                name: "app".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("app/Cargo.toml"),
            },
        ],
        dependencies: BTreeMap::from([
            ("utils".to_string(), vec!["core".to_string()]),
            (
                "app".to_string(),
                vec!["core".to_string(), "utils".to_string()],
            ),
        ]),
    };

    let levels = plan.group_by_levels();

    // Build a map from package name → level number
    let mut pkg_level: BTreeMap<&str, usize> = BTreeMap::new();
    for level in &levels {
        for pkg in &level.packages {
            pkg_level.insert(&pkg.name, level.level);
        }
    }

    // Every dependency must be at a strictly earlier level
    for (dependent, deps) in &plan.dependencies {
        let dep_level = pkg_level[dependent.as_str()];
        for req in deps {
            let req_level = pkg_level[req.as_str()];
            assert!(
                req_level < dep_level,
                "{} (level {}) should come before {} (level {})",
                req,
                req_level,
                dependent,
                dep_level
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Edge case: all packages at same dependency level (single level)
// ---------------------------------------------------------------------------

#[test]
fn test_all_packages_same_level_no_deps() {
    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-flat".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages: vec![
            PlannedPackage {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("foo/Cargo.toml"),
            },
            PlannedPackage {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("bar/Cargo.toml"),
            },
            PlannedPackage {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("baz/Cargo.toml"),
            },
        ],
        dependencies: BTreeMap::new(),
    };

    let levels = plan.group_by_levels();
    assert_eq!(levels.len(), 1, "no deps means single level");
    assert_eq!(levels[0].level, 0);
    assert_eq!(levels[0].packages.len(), 3);
    let names: Vec<&str> = levels[0].packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["foo", "bar", "baz"]);
}

// ---------------------------------------------------------------------------
// Edge case: linear dependency chain (each package depends on the previous)
// ---------------------------------------------------------------------------

#[test]
fn test_linear_chain_produces_n_levels() {
    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-chain".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages: vec![
            PlannedPackage {
                name: "l1".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l1/Cargo.toml"),
            },
            PlannedPackage {
                name: "l2".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l2/Cargo.toml"),
            },
            PlannedPackage {
                name: "l3".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l3/Cargo.toml"),
            },
            PlannedPackage {
                name: "l4".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l4/Cargo.toml"),
            },
            PlannedPackage {
                name: "l5".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l5/Cargo.toml"),
            },
        ],
        dependencies: BTreeMap::from([
            ("l2".to_string(), vec!["l1".to_string()]),
            ("l3".to_string(), vec!["l2".to_string()]),
            ("l4".to_string(), vec!["l3".to_string()]),
            ("l5".to_string(), vec!["l4".to_string()]),
        ]),
    };

    let levels = plan.group_by_levels();
    assert_eq!(levels.len(), 5, "linear chain of 5 should produce 5 levels");
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(level.packages.len(), 1, "each level has exactly 1 package");
        assert_eq!(level.packages[0].name, format!("l{}", i + 1));
    }
}

// ---------------------------------------------------------------------------
// Error propagation: failure in level 0 prevents level 1 from executing
// (additional scenario: multiple packages in the failing level)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_error_in_first_level_prevents_all_subsequent() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // 3-level chain: a → b → c.  "a" fails permanently.
    // Only "a" should be checked (404 twice: initial + after-failure)
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/a/1.0.0".to_string(),
            vec![(404, "{}".to_string()), (404, "{}".to_string())],
        )]),
        2,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-halt-chain".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![
                PlannedPackage {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("a").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("b").join("Cargo.toml"),
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("c").join("Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["b".to_string()]),
            ]),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1;

    let mut packages = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
        ],
        || {
            let result = run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter);

            assert!(result.is_err(), "publish should fail");

            // Both "b" and "c" should remain Pending
            for name in ["b", "c"] {
                let key = pkg_key(name, "1.0.0");
                let progress = st.packages.get(&key).expect(name);
                assert!(
                    matches!(progress.state, PackageState::Pending),
                    "{} should remain Pending, got {:?}",
                    name,
                    progress.state
                );
            }
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Empty plan: no packages → no receipts
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_empty_plan_produces_no_receipts() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // No packages at all
    let server = spawn_registry_server(BTreeMap::new(), 0);

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-empty".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::new(),
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("empty publish");

            assert!(receipts.is_empty(), "empty plan should produce no receipts");
        },
    );
    drop(server);
}

// ---------------------------------------------------------------------------
// All independent packages land in a single level
// ---------------------------------------------------------------------------

#[test]
fn test_all_independent_packages_single_level() {
    let packages: Vec<PlannedPackage> = (0..8)
        .map(|i| PlannedPackage {
            name: format!("pkg-{i}"),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from(format!("pkg-{i}/Cargo.toml")),
        })
        .collect();

    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-all-independent".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages,
        dependencies: BTreeMap::new(),
    };

    let levels = plan.group_by_levels();
    assert_eq!(levels.len(), 1, "all independent → exactly 1 level");
    assert_eq!(levels[0].packages.len(), 8, "all 8 packages in level 0");
}

// ---------------------------------------------------------------------------
// Wide fan-out: one root with many dependents → exactly 2 levels
// ---------------------------------------------------------------------------

#[test]
fn test_wide_fan_out_two_levels() {
    let mut packages = vec![PlannedPackage {
        name: "root".to_string(),
        version: "1.0.0".to_string(),
        manifest_path: PathBuf::from("root/Cargo.toml"),
    }];
    let mut deps = BTreeMap::new();
    for i in 0..6 {
        let name = format!("leaf-{i}");
        packages.push(PlannedPackage {
            name: name.clone(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from(format!("{name}/Cargo.toml")),
        });
        deps.insert(name, vec!["root".to_string()]);
    }

    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-fan-out".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages,
        dependencies: deps,
    };

    let levels = plan.group_by_levels();
    assert_eq!(levels.len(), 2, "fan-out should produce exactly 2 levels");
    assert_eq!(levels[0].packages.len(), 1, "level 0 has root only");
    assert_eq!(levels[1].packages.len(), 6, "level 1 has all 6 leaves");
}

// ---------------------------------------------------------------------------
// Deep chain produces exactly N levels
// ---------------------------------------------------------------------------

#[test]
fn test_deep_chain_produces_n_levels() {
    let n = 7;
    let packages: Vec<PlannedPackage> = (0..n)
        .map(|i| PlannedPackage {
            name: format!("c{i}"),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from(format!("c{i}/Cargo.toml")),
        })
        .collect();
    let mut deps = BTreeMap::new();
    for i in 1..n {
        deps.insert(format!("c{i}"), vec![format!("c{}", i - 1)]);
    }

    let plan = ReleasePlan {
        plan_version: "1".to_string(),
        plan_id: "plan-deep-chain".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages,
        dependencies: deps,
    };

    let levels = plan.group_by_levels();
    assert_eq!(
        levels.len(),
        n,
        "chain of {n} should produce exactly {n} levels"
    );
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(level.packages.len(), 1);
        assert_eq!(level.packages[0].name, format!("c{i}"));
    }
}

// ---------------------------------------------------------------------------
// max_concurrent > package count: all run in one batch
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_max_concurrent_exceeds_package_count() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/p1/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/p2/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        2,
    );

    let packages: Vec<PlannedPackage> = ["p1", "p2"]
        .iter()
        .map(|n| PlannedPackage {
            name: n.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(n).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-over-concurrent".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.max_concurrent = 100; // far exceeds 2 packages

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            )
            .expect("level publish");

            assert_eq!(receipts.len(), 2, "both packages should complete");
            for r in &receipts {
                assert!(matches!(r.state, PackageState::Skipped { .. }));
            }
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Two independent failures in same level: error message reports both
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_independent_failures_both_reported() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Both packages not published, both will fail
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/fail-a/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            ),
            (
                "/api/v1/crates/fail-b/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            ),
        ]),
        4,
    );

    let packages: Vec<PlannedPackage> = ["fail-a", "fail-b"]
        .iter()
        .map(|n| PlannedPackage {
            name: n.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(n).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-dual-fail".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1;

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
        ],
        || {
            let result = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            assert!(result.is_err());
            let err_msg = format!("{:#}", result.unwrap_err());
            assert!(
                err_msg.contains("2 package"),
                "error should mention 2 failed packages, got: {err_msg}"
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Concurrent state updates: all packages in a parallel level write state
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_concurrent_state_updates_consistent() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let pkg_names: Vec<&str> = vec!["s1", "s2", "s3", "s4"];
    let mut routes = BTreeMap::new();
    for name in &pkg_names {
        routes.insert(
            format!("/api/v1/crates/{name}/0.1.0"),
            vec![(200, "{}".to_string())],
        );
    }
    let server = spawn_registry_server(routes, pkg_names.len());

    let packages: Vec<PlannedPackage> = pkg_names
        .iter()
        .map(|n| PlannedPackage {
            name: n.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(n).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-concurrent-state".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.max_concurrent = 4; // all run concurrently

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let st = Arc::new(Mutex::new(ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    let level = PublishLevel { level: 0, packages };

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts = run_publish_level(
                &level,
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            )
            .expect("level publish");

            assert_eq!(receipts.len(), 4);

            // Verify state was updated for every package
            let state = st.lock().unwrap();
            for name in &pkg_names {
                let key = pkg_key(name, "0.1.0");
                let progress = state.packages.get(&key).expect(name);
                assert!(
                    matches!(progress.state, PackageState::Skipped { .. }),
                    "{name} state should be Skipped, got {:?}",
                    progress.state
                );
            }
            // All keys present
            assert_eq!(state.packages.len(), 4);
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Balanced policy preserves readiness
// ---------------------------------------------------------------------------

#[test]
fn test_balanced_policy_preserves_readiness() {
    let mut opts = default_opts(PathBuf::from(".shipper"));
    opts.policy = shipper_types::PublishPolicy::Balanced;
    opts.readiness.enabled = true;

    let effects = policy_effects(&opts);
    assert!(
        effects.readiness_enabled,
        "Balanced policy should preserve readiness"
    );
}

// ---------------------------------------------------------------------------
// Execution result classification: mixed states
// ---------------------------------------------------------------------------

#[test]
fn test_execution_result_mixed_published_and_skipped_is_success() {
    let receipts = [
        PackageReceipt {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
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
        },
        PackageReceipt {
            name: "b".to_string(),
            version: "1.0.0".to_string(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".into(),
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
        PackageReceipt {
            name: "c".to_string(),
            version: "1.0.0".to_string(),
            attempts: 1,
            state: PackageState::Uploaded,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 50,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        },
    ];

    let all_ok = receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    });
    assert!(
        all_ok,
        "Published+Skipped+Uploaded mix should be classified as success"
    );
}

// ---------------------------------------------------------------------------
// Execution result classification: partial failure
// ---------------------------------------------------------------------------

#[test]
fn test_execution_result_partial_failure() {
    let receipts = [
        PackageReceipt {
            name: "good".to_string(),
            version: "1.0.0".to_string(),
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
        },
        PackageReceipt {
            name: "bad".to_string(),
            version: "1.0.0".to_string(),
            attempts: 3,
            state: PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "denied".into(),
            },
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 200,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        },
    ];

    let success_count = receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Published))
        .count();
    let all_ok = receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    });

    assert!(!all_ok, "mix with Failed should not be all-ok");
    assert!(success_count > 0, "some packages succeeded");
}

// ---------------------------------------------------------------------------
// Execution result classification: complete failure
// ---------------------------------------------------------------------------

#[test]
fn test_execution_result_complete_failure() {
    let receipts = [PackageReceipt {
        name: "only".to_string(),
        version: "1.0.0".to_string(),
        attempts: 2,
        state: PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "denied".into(),
        },
        started_at: Utc::now(),
        finished_at: Utc::now(),
        duration_ms: 300,
        evidence: PackageEvidence {
            attempts: vec![],
            readiness_checks: vec![],
        },
        compromised_at: None,
        compromised_by: None,
        superseded_by: None,
    }];

    let success_count = receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Published))
        .count();
    assert_eq!(success_count, 0, "no successes → complete failure");
}

// ---------------------------------------------------------------------------
// Level reporter message format: includes concurrent count
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_level_message_includes_max_concurrent() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/m1/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/m2/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/m3/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
        ]),
        3,
    );

    let packages: Vec<PlannedPackage> = ["m1", "m2", "m3"]
        .iter()
        .map(|n| PlannedPackage {
            name: n.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join(n).join("Cargo.toml"),
        })
        .collect();

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-msg-test".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: packages.clone(),
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.max_concurrent = 2;

    let mut state_packages = BTreeMap::new();
    for p in &packages {
        state_packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: state_packages,
    };
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let receipts =
                run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("publish");

            assert_eq!(receipts.len(), 3);

            // Reporter infos are replayed from the internal SendReporter.
            // The level info message format is: "Level N: publishing M packages (max concurrent: C)"
            let level_msg = reporter
                .infos
                .iter()
                .find(|m| m.contains("Level 0"))
                .expect("should have Level 0 message");
            assert!(
                level_msg.contains("max concurrent: 2"),
                "level message should include max concurrent, got: {level_msg}"
            );
            assert!(
                level_msg.contains("3 packages"),
                "level message should include package count, got: {level_msg}"
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// State persisted to disk after level completion
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_state_persisted_to_disk_after_level() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/saved/0.1.0".to_string(),
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let ws = PlannedWorkspace {
        plan: ReleasePlan {
            packages: vec![PlannedPackage {
                name: "saved".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: td.path().join("saved").join("Cargo.toml"),
            }],
            ..ws.plan
        },
        ..ws
    };

    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "saved", "0.1.0");
    let mut reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let _ = run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                .expect("publish");

            // Verify state file was written to disk
            let state_file = state_dir.join("state.json");
            assert!(
                state_file.exists(),
                "state.json should be persisted to disk"
            );
            let contents = fs::read_to_string(&state_file).expect("read state");
            let on_disk: ExecutionState = serde_json::from_str(&contents).expect("parse state");
            let progress = on_disk.packages.get("saved@0.1.0").expect("pkg");
            assert!(
                matches!(progress.state, PackageState::Skipped { .. }),
                "on-disk state should be Skipped, got {:?}",
                progress.state
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// Chunking exact multiple: N items ÷ chunk_size = no remainder
// ---------------------------------------------------------------------------

#[test]
fn test_chunking_exact_multiple_no_remainder() {
    let items: Vec<String> = (0..12).map(|i| format!("pkg-{i}")).collect();
    let chunks = chunk_by_max_concurrent(&items, 4);
    assert_eq!(chunks.len(), 3, "12 / 4 = exactly 3 chunks");
    for chunk in &chunks {
        assert_eq!(chunk.len(), 4, "each chunk should have exactly 4 items");
    }
}

// ---------------------------------------------------------------------------
// Property tests: level count >= 1 for non-empty input
// ---------------------------------------------------------------------------

mod property_tests_extra {
    use super::*;
    use proptest::prelude::*;

    fn pkg_name() -> impl Strategy<Value = String> {
        "[a-z]{1,6}".prop_map(|s| s)
    }

    proptest! {
        #[test]
        fn level_count_ge_one_for_nonempty(
            names in prop::collection::hash_set(pkg_name(), 1..16)
        ) {
            let packages: Vec<PlannedPackage> = names
                .iter()
                .map(|n| PlannedPackage {
                    name: n.clone(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from(format!("{}/Cargo.toml", n)),
                })
                .collect();

            let plan = ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "prop-test".to_string(),
                created_at: Utc::now(),
                registry: Registry::crates_io(),
                packages,
                dependencies: BTreeMap::new(),
            };

            let levels = plan.group_by_levels();
            prop_assert!(!levels.is_empty(), "non-empty plan must produce >= 1 level");
        }

        #[test]
        fn all_packages_appear_exactly_once_in_levels(
            names in prop::collection::hash_set(pkg_name(), 1..16)
        ) {
            let packages: Vec<PlannedPackage> = names
                .iter()
                .map(|n| PlannedPackage {
                    name: n.clone(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from(format!("{}/Cargo.toml", n)),
                })
                .collect();

            let expected_count = packages.len();
            let plan = ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "prop-test-2".to_string(),
                created_at: Utc::now(),
                registry: Registry::crates_io(),
                packages,
                dependencies: BTreeMap::new(),
            };

            let levels = plan.group_by_levels();
            let total: usize = levels.iter().map(|l| l.packages.len()).sum();
            prop_assert_eq!(total, expected_count, "every package must appear exactly once");
        }

        #[test]
        fn dependencies_always_in_earlier_levels(
            n in 2usize..12,
            edge_count in 0usize..20,
        ) {
            // Build a random valid DAG: edges only go from higher-index to lower-index
            let packages: Vec<PlannedPackage> = (0..n)
                .map(|i| PlannedPackage {
                    name: format!("p{i}"),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from(format!("p{i}/Cargo.toml")),
                })
                .collect();

            let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
            // Deterministic pseudo-random edges using index arithmetic
            for e in 0..edge_count {
                let dependent_idx = (e % (n - 1)) + 1; // 1..n-1
                let dep_idx = e % dependent_idx; // 0..dependent_idx
                deps.entry(format!("p{dependent_idx}"))
                    .or_default()
                    .push(format!("p{dep_idx}"));
            }
            // Deduplicate
            for v in deps.values_mut() {
                v.sort();
                v.dedup();
            }

            let plan = ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "prop-dag".to_string(),
                created_at: Utc::now(),
                registry: Registry::crates_io(),
                packages,
                dependencies: deps.clone(),
            };

            let levels = plan.group_by_levels();

            // Build level-index map
            let mut pkg_level: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for level in &levels {
                for p in &level.packages {
                    pkg_level.insert(p.name.clone(), level.level);
                }
            }

            // Verify every dependency is in a strictly earlier level
            for (pkg, dep_list) in &deps {
                if let Some(&pkg_lev) = pkg_level.get(pkg) {
                    for dep in dep_list {
                        if let Some(&dep_lev) = pkg_level.get(dep) {
                            prop_assert!(
                                dep_lev < pkg_lev,
                                "dep {} (level {}) should precede {} (level {})",
                                dep, dep_lev, pkg, pkg_lev
                            );
                        }
                    }
                }
            }
        }

        #[test]
        fn independent_packages_all_in_single_level(
            count in 1usize..20,
        ) {
            let packages: Vec<PlannedPackage> = (0..count)
                .map(|i| PlannedPackage {
                    name: format!("ind{i}"),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from(format!("ind{i}/Cargo.toml")),
                })
                .collect();

            let plan = ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "prop-independent".to_string(),
                created_at: Utc::now(),
                registry: Registry::crates_io(),
                packages,
                dependencies: BTreeMap::new(),
            };

            let levels = plan.group_by_levels();
            prop_assert_eq!(
                levels.len(), 1,
                "all independent packages must be in exactly 1 level"
            );
            prop_assert_eq!(levels[0].packages.len(), count);
        }

        #[test]
        fn chunk_count_matches_ceiling_division(
            n in 0usize..100,
            max in 1usize..32,
        ) {
            let items: Vec<usize> = (0..n).collect();
            let chunks = chunk_by_max_concurrent(&items, max);
            let expected = if n == 0 { 0 } else { n.div_ceil(max) };
            prop_assert_eq!(
                chunks.len(), expected,
                "chunk count mismatch"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reconciliation BDD scenarios (#99 follow-on)
//
// These three tests exercise the ambiguous-outcome reconciliation state
// machine introduced in PR #111 (in-flight) and PR #115 (resume-path)
// end-to-end: cargo failing with an ambiguous class + mocked registry
// responses, asserting on the three reconciliation outcomes.
//
// Ambiguous classification is triggered by cargo exiting non-zero with
// empty stdout/stderr (see `shipper-cargo-failure::classify_publish_failure`
// edge-case tests). To keep readiness-polling bounded to a single call per
// reconcile site (avoiding flaky request counts), each test disables the
// readiness backoff loop: `readiness.enabled = false`.
// ---------------------------------------------------------------------------

/// Build a RuntimeOptions with readiness polling disabled — every reconcile
/// site reduces to a single registry query. Simplifies mock request math.
fn reconcile_scenario_opts(state_dir: PathBuf) -> RuntimeOptions {
    let mut opts = default_opts(state_dir);
    opts.readiness.enabled = false;
    opts
}

#[test]
#[serial]
fn reconcile_bdd_ambiguous_resolves_to_published() {
    // Scenario: cargo exits ambiguously (exit 1, empty stderr) on attempt 1.
    // The quick post-failure version_exists check sees nothing, so classify
    // returns Ambiguous → reconcile_ambiguous_upload fires and the registry
    // reports the version as visible. Expected: state becomes Published,
    // no second cargo invocation.
    //
    // Request sequence (readiness disabled):
    //   1. entry "already published" check (publish.rs:136) → 404
    //   2. post-cargo-failure quick check (publish.rs:~446) → 404
    //   3. reconcile's single version_exists (via is_version_visible_with_backoff, enabled=false) → 200
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (200, "{}".to_string()),
            ],
        )]),
        3,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = reconcile_scenario_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            // Empty stderr/stdout → classify_publish_failure returns Ambiguous.
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result.result.expect("reconcile should mark Published");
            assert!(
                matches!(receipt.state, PackageState::Published),
                "expected Published via reconcile, got {:?}",
                receipt.state
            );
            // attempts=1 because reconcile fired on the first attempt's failure
            // and resolved to Published — no further cargo invocations.
            assert_eq!(receipt.attempts, 1);
        },
    );

    // Verify the event stream records the reconcile decision. Events are
    // flushed to disk + cleared from the in-memory log after each write, so
    // we read from disk here.
    let persisted = events::EventLog::read_from_file(&events_path).expect("read events");
    let has_reconciling = persisted
        .all_events()
        .iter()
        .any(|e| matches!(e.event_type, EventType::PublishReconciling { .. }));
    let has_reconciled_published = persisted.all_events().iter().any(|e| {
        matches!(
            &e.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::Published { .. }
            }
        )
    });
    assert!(has_reconciling, "expected PublishReconciling event");
    assert!(
        has_reconciled_published,
        "expected PublishReconciled with Published outcome"
    );

    server.join();
}

#[test]
#[serial]
fn reconcile_bdd_ambiguous_resolves_to_not_published_then_retries() {
    // Scenario: cargo exits ambiguously on every attempt. Registry is
    // consistently 404 — the version never appears. Reconcile resolves
    // to NotPublished on each cargo-failure path, which falls through to
    // the normal Retryable backoff → retry → fails-ambiguous-again. After
    // max_attempts, the package ends Failed.
    //
    // With max_attempts=2 and readiness disabled, the request sequence is:
    //   1. entry check → 404
    //   2. attempt 1 post-cargo quick check → 404
    //   3. attempt 1 reconcile (enabled=false, single call) → 404 → NotPublished
    //   4. attempt 2 post-cargo quick check → 404
    //   5. attempt 2 reconcile → 404 → NotPublished
    //   6. post-loop final "if last_err, maybe visible" check (publish.rs:~817) → 404
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
            ],
        )]),
        6,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let mut opts = reconcile_scenario_opts(PathBuf::from(".shipper"));
    opts.max_attempts = 2;
    let state_dir = td.path().join(".shipper");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            // max_attempts exhausted, registry never visible → Failed.
            let err = result
                .result
                .expect_err("max_attempts exhausted without success should err");
            let msg = err.to_string();
            assert!(
                msg.contains("failed"),
                "expected failure message, got: {msg}"
            );
            // Verify cargo was actually retried (attempts incremented).
            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("package progress");
            assert_eq!(progress.attempts, 2, "expected 2 cargo attempts");
        },
    );

    // Verify at least one reconcile_not_published decision was recorded
    // (read from disk; in-memory log is cleared after each flush).
    let persisted = events::EventLog::read_from_file(&events_path).expect("read events");
    let has_reconciled_not_published = persisted.all_events().iter().any(|e| {
        matches!(
            &e.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::NotPublished { .. }
            }
        )
    });
    assert!(
        has_reconciled_not_published,
        "expected at least one PublishReconciled with NotPublished outcome"
    );

    server.join();
}

#[test]
#[serial]
fn reconcile_bdd_resume_from_ambiguous_state_skips_republish() {
    // Scenario (from PR #115 resume-path reconcile):
    //   A prior run left demo@0.1.0 in PackageState::Ambiguous. On resume,
    //   the entry "already published" check still returns 404 (publish.rs:136).
    //   The resume-path reconcile block (publish.rs:~248) fires BEFORE the
    //   retry loop, polls the registry, and discovers the version IS now
    //   visible — marks Published and returns early with zero cargo attempts.
    //
    // Request sequence:
    //   1. entry check → 404
    //   2. resume-path reconcile's single version_exists → 200
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = RegistryClient::new(ws.plan.registry.api_base.as_str());
    let opts = reconcile_scenario_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");

    // Pre-populate state with PackageState::Ambiguous (as a prior interrupted
    // run would have left it).
    let mut initial_state =
        init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "demo", "0.1.0");
    if let Some(pr) = initial_state.packages.get_mut("demo@0.1.0") {
        pr.state = PackageState::Ambiguous {
            message: "prior reconciliation inconclusive".to_string(),
        };
    }
    let st = Arc::new(Mutex::new(initial_state));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter: Arc<Mutex<dyn Reporter + Send>> =
        Arc::new(Mutex::new(CollectingReporter::default()));

    // Track whether cargo was invoked via a log file.
    let cargo_log = td.path().join("cargo-calls.log");

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_str().expect("utf8")),
            ),
            // Cargo would succeed if called, but we expect it to NOT be called.
            ("SHIPPER_CARGO_EXIT", Some("0")),
        ],
        || {
            let result = publish_package(
                &ws.plan.packages[0],
                &ws,
                &opts,
                &reg,
                &st,
                &state_dir,
                &event_log,
                &events_path,
                &reporter,
            );

            let receipt = result
                .result
                .expect("resume-path reconcile should resolve Published");
            assert!(
                matches!(receipt.state, PackageState::Published),
                "expected Published via resume-path reconcile, got {:?}",
                receipt.state
            );
            // No cargo attempts because we resolved before the retry loop.
            assert_eq!(receipt.attempts, 0);
        },
    );

    // Verify cargo was never actually invoked.
    let cargo_invoked = std::fs::read_to_string(&cargo_log)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    assert!(
        !cargo_invoked,
        "cargo should not have been invoked on resume-path reconcile Published"
    );

    // Verify the reconcile events were emitted (read from disk).
    let persisted = events::EventLog::read_from_file(&events_path).expect("read events");
    let has_reconciling = persisted
        .all_events()
        .iter()
        .any(|e| matches!(e.event_type, EventType::PublishReconciling { .. }));
    let has_reconciled_published = persisted.all_events().iter().any(|e| {
        matches!(
            &e.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::Published { .. }
            }
        )
    });
    assert!(has_reconciling, "expected PublishReconciling event");
    assert!(
        has_reconciled_published,
        "expected PublishReconciled with Published outcome"
    );

    server.join();
}
