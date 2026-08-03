use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use chrono::Utc;
use serial_test::serial;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

use super::policy::policy_effects;
use super::publish::{emit_retry_backoff, publish_package, run_publish_level};
use super::run_publish_parallel_inner as run_publish_parallel;
use super::*;
use crate::plan::PlannedWorkspace;
use crate::registry::{RegistryClient, RegistryPolicy};
use crate::runtime::execution::{pkg_key, update_state_locked};
use crate::state::events;
use crate::state::rebuild::{StateRebuildOptions, rebuild_state_from_events};
use shipper_types::{
    AttemptDetail, ErrorClass, EventType, ExecutionState, PackageEvidence, PackageProgress,
    PackageReceipt, PackageState, PlannedPackage, PublishLevel, ReadinessConfig, Registry,
    ReleasePlan, RuntimeOptions,
};

fn error_class_rank(error_class: &Option<ErrorClass>) -> u8 {
    match error_class {
        Some(ErrorClass::Retryable) => 0,
        Some(ErrorClass::Permanent) => 1,
        Some(ErrorClass::Ambiguous) => 2,
        None => 3,
    }
}

fn sort_attempt_history_by_fields(history: &mut [AttemptDetail]) {
    history.sort_by_key(|entry| {
        (
            entry.package.clone(),
            entry.version.clone(),
            entry.attempt,
            entry.max_attempts,
            error_class_rank(&entry.error_class),
            entry.next_attempt_at,
        )
    });
}

fn test_registry_client(ws: &PlannedWorkspace) -> RegistryClient {
    let mut registry = ws.plan.registry.clone();
    if registry.index_base.is_none() {
        registry.index_base = Some(registry.api_base.clone());
    }
    RegistryClient::with_policy(registry, RegistryPolicy::rehearsal()).expect("registry client")
}

fn make_send_reporter() -> Arc<SendReporter> {
    Arc::new(SendReporter::default())
}

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

impl crate::engine::Reporter for CollectingReporter {
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

#[test]
fn emit_retry_backoff_does_not_block_other_reporter_calls_during_sleep() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir state dir");

    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter = make_send_reporter();

    let event_log_for_retry = Arc::clone(&event_log);
    let reporter_for_retry = Arc::clone(&reporter);
    let events_path_for_retry = events_path.clone();
    let delay = Duration::from_millis(250);

    let retry_thread = std::thread::spawn(move || {
        emit_retry_backoff(
            &event_log_for_retry,
            &events_path_for_retry,
            &reporter_for_retry,
            "demo@0.1.0",
            "demo",
            "0.1.0",
            1,
            3,
            delay,
            chrono::Utc::now() + chrono::Duration::milliseconds(250),
            ErrorClass::Retryable,
            "rate limited",
        )
        .expect("backoff should be recorded");
    });

    std::thread::sleep(Duration::from_millis(25));

    let lock_started = Instant::now();
    reporter.info("other worker thread");

    assert!(
        lock_started.elapsed() < Duration::from_millis(100),
        "retry backoff blocked other reporter calls for {:?}",
        lock_started.elapsed()
    );

    retry_thread.join().expect("join retry thread");

    let infos = reporter.drain_infos();
    assert!(
        infos.iter().any(|msg| msg == "other worker thread"),
        "concurrent info should still be recorded"
    );
}

#[test]
fn drain_retry_waits_forwards_live_notice_before_worker_sleep_elapses() {
    struct SignalingReporter {
        tx: Option<mpsc::Sender<Duration>>,
    }

    impl Reporter for SignalingReporter {
        fn info(&mut self, _msg: &str) {}

        fn warn(&mut self, _msg: &str) {}

        fn error(&mut self, _msg: &str) {}

        fn retry_wait(
            &mut self,
            _pkg_name: &str,
            _pkg_version: &str,
            _attempt: u32,
            _max_attempts: u32,
            delay: Duration,
            _reason: ErrorClass,
            _message: &str,
        ) {
            if let Some(tx) = self.tx.take() {
                tx.send(delay).expect("send forwarded delay");
            }
        }
    }

    let send_reporter = make_send_reporter();
    let delay = Duration::from_millis(250);
    let reporter_for_retry = Arc::clone(&send_reporter);
    let retry_thread = std::thread::spawn(move || {
        reporter_for_retry.retry_wait(
            "demo",
            "0.1.0",
            1,
            3,
            delay,
            ErrorClass::Retryable,
            "rate limited",
        );
    });

    std::thread::sleep(Duration::from_millis(25));

    let (tx, rx) = mpsc::channel();
    let mut host_reporter = SignalingReporter { tx: Some(tx) };
    drain_retry_waits(&mut host_reporter, send_reporter.as_ref());

    let forwarded_delay = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("host reporter should observe retry wait promptly");
    assert!(
        forwarded_delay <= delay && forwarded_delay > Duration::ZERO,
        "forwarded delay should be the remaining live backoff, got {forwarded_delay:?}"
    );
    assert!(
        !retry_thread.is_finished(),
        "worker retry sleep should still be in progress when the host observes it"
    );

    retry_thread.join().expect("join retry thread");
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
                api_base: api_base.clone(),
                index_base: Some(api_base),
            },
            packages: vec![PlannedPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: workspace_root.join("demo").join("Cargo.toml"),
                regime: None,
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
        lock_timeout: Duration::from_hours(1),
        parallel: shipper_types::ParallelConfig {
            enabled: true,
            max_concurrent: 4,
            per_package_timeout: Duration::from_mins(1),
        },
        retry_strategy: shipper_retry::RetryStrategyType::Exponential,
        retry_jitter: 0.0,
        retry_per_error: shipper_retry::PerErrorConfig::default(),
        encryption: shipper_encrypt::EncryptionConfig::default(),
        webhook: shipper_webhook::WebhookConfig::default(),
        registries: vec![],
        registry_policies: Default::default(),
        resume_from: None,
        rehearsal_registry: None,
        rehearsal_skip: false,
        rehearsal_smoke_install: None,
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
        attempt_history: Vec::new(),
        packages,
    }
}

fn planned_workspace_with_dependency(workspace_root: &Path, api_base: String) -> PlannedWorkspace {
    PlannedWorkspace {
        workspace_root: workspace_root.to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-multi-level".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: api_base.clone(),
                index_base: Some(api_base),
            },
            packages: vec![
                PlannedPackage {
                    name: "base".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: workspace_root.join("base").join("Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: workspace_root.join("dependent").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    }
}

fn init_state_for_workspace(plan: &PlannedWorkspace) -> ExecutionState {
    let mut packages = BTreeMap::new();
    for p in &plan.plan.packages {
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

    ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: plan.plan.plan_id.clone(),
        registry: plan.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        attempt_history: Vec::new(),
        packages,
    }
}

fn init_state_with_checkpoint(
    plan: &PlannedWorkspace,
    key: &str,
    state: PackageState,
    attempts: u32,
) -> ExecutionState {
    let mut execution_state = init_state_for_workspace(plan);
    if let Some(progress) = execution_state.packages.get_mut(key) {
        progress.state = state;
        progress.attempts = attempts;
    }
    execution_state
}

fn init_state_with_uploaded_checkpoint(
    plan: &PlannedWorkspace,
    uploaded_key: &str,
) -> ExecutionState {
    init_state_with_checkpoint(plan, uploaded_key, PackageState::Uploaded, 1)
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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
            let persisted = events::EventLog::read_from_file(&events_path).expect("read events");
            assert!(
                persisted
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PackageUploaded))
            );
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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
                    regime: None,
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "0.2.0".to_string(),
                    manifest_path: td.path().join("beta").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
        attempt_history: Vec::new(),
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
    let reg = test_registry_client(&ws);
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
                    regime: None,
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
fn test_run_publish_mode_parity_for_multi_level_plan() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server_sequential = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
        ]),
        4,
    );
    let ws_seq = planned_workspace_with_dependency(td.path(), server_sequential.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.parallel.enabled = false;

    let server_parallel = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
        ]),
        4,
    );
    let ws_par = planned_workspace_with_dependency(td.path(), server_parallel.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let opts_par = default_opts(state_dir_par.clone());

    let mut state_seq = init_state_for_workspace(&ws_seq);
    let mut state_par = init_state_for_workspace(&ws_par);
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_reporter = CollectingReporter::default();

    let mut par_reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut state_seq,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            let mut par_receipts = run_publish_parallel(
                &ws_par,
                &opts_par,
                &mut state_par,
                &state_dir_par,
                &reg_par,
                &mut par_reporter,
            )
            .expect("parallel publish");

            let mut seq_receipts = seq_receipts;
            seq_receipts.sort_by(|a, b| a.name.cmp(&b.name));
            par_receipts.sort_by(|a, b| a.name.cmp(&b.name));

            assert_eq!(
                seq_receipts.len(),
                par_receipts.len(),
                "receipt counts must match"
            );
            assert_eq!(
                state_seq.packages.len(),
                state_par.packages.len(),
                "package count"
            );

            for (seq, par) in seq_receipts.iter().zip(par_receipts.iter()) {
                assert_eq!(seq.name, par.name);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.state, par.state);
                assert_eq!(seq.attempts, par.attempts);
            }

            for key in ["base@1.0.0", "dependent@2.0.0"] {
                let seq_pkg = state_seq.packages.get(key).expect("sequential state");
                let par_pkg = state_par.packages.get(key).expect("parallel state");
                assert_eq!(seq_pkg.state, par_pkg.state);
                assert_eq!(seq_pkg.attempts, par_pkg.attempts);
            }

            let mut seq_attempt_history = state_seq.attempt_history.clone();
            let mut par_attempt_history = state_par.attempt_history.clone();
            sort_attempt_history_by_fields(&mut seq_attempt_history);
            sort_attempt_history_by_fields(&mut par_attempt_history);

            assert_eq!(
                seq_attempt_history.len(),
                par_attempt_history.len(),
                "attempt history length"
            );

            for (seq, par) in seq_attempt_history.iter().zip(par_attempt_history.iter()) {
                assert_eq!(seq.package, par.package);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.attempt, par.attempt);
                assert_eq!(seq.max_attempts, par.max_attempts);
                assert_eq!(seq.error_class, par.error_class);
                assert_eq!(seq.next_attempt_at.is_some(), par.next_attempt_at.is_some());
            }
        },
    );

    server_sequential.join();
    server_parallel.join();
}

#[test]
#[serial]
fn test_run_publish_mode_retry_parity_for_multi_level_plan() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let transient_responses = vec![
        (404, "{}".to_string()),
        (404, "{}".to_string()),
        (200, "{}".to_string()),
    ];

    let server_sequential = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                transient_responses.clone(),
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                transient_responses.clone(),
            ),
        ]),
        6,
    );
    let ws_seq = planned_workspace_with_dependency(td.path(), server_sequential.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.parallel.enabled = false;
    opts_seq.max_attempts = 2;

    let server_parallel = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                transient_responses.clone(),
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                transient_responses.clone(),
            ),
        ]),
        6,
    );
    let ws_par = planned_workspace_with_dependency(td.path(), server_parallel.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let mut opts_par = default_opts(state_dir_par.clone());
    opts_par.max_attempts = 2;

    let mut state_seq = init_state_for_workspace(&ws_seq);
    let mut state_par = init_state_for_workspace(&ws_par);
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_reporter = CollectingReporter::default();

    let mut par_reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("timeout talking to server")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        || {
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut state_seq,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            let mut par_receipts = run_publish_parallel(
                &ws_par,
                &opts_par,
                &mut state_par,
                &state_dir_par,
                &reg_par,
                &mut par_reporter,
            )
            .expect("parallel publish");

            let mut seq_receipts = seq_receipts;
            seq_receipts.sort_by(|a, b| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.version.cmp(&b.version))
                    .then_with(|| a.attempts.cmp(&b.attempts))
            });
            par_receipts.sort_by(|a, b| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.version.cmp(&b.version))
                    .then_with(|| a.attempts.cmp(&b.attempts))
            });

            assert_eq!(
                seq_receipts.len(),
                par_receipts.len(),
                "receipt counts must match"
            );

            for (seq, par) in seq_receipts.iter().zip(par_receipts.iter()) {
                assert_eq!(seq.name, par.name);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.state, par.state);
                assert_eq!(seq.attempts, par.attempts);
            }

            for key in ["base@1.0.0", "dependent@2.0.0"] {
                let seq_pkg = state_seq.packages.get(key).expect("sequential state");
                let par_pkg = state_par.packages.get(key).expect("parallel state");
                assert_eq!(seq_pkg.state, par_pkg.state);
                assert_eq!(seq_pkg.attempts, par_pkg.attempts);
            }

            let mut seq_attempt_history = state_seq.attempt_history.clone();
            let mut par_attempt_history = state_par.attempt_history.clone();
            sort_attempt_history_by_fields(&mut seq_attempt_history);
            sort_attempt_history_by_fields(&mut par_attempt_history);

            assert!(
                !seq_attempt_history.is_empty(),
                "expected retry attempt history entries",
            );
            assert_eq!(
                seq_attempt_history.len(),
                par_attempt_history.len(),
                "attempt history length"
            );

            for (seq, par) in seq_attempt_history.iter().zip(par_attempt_history.iter()) {
                assert_eq!(seq.package, par.package);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.attempt, par.attempt);
                assert_eq!(seq.max_attempts, par.max_attempts);
                assert_eq!(seq.error_class, par.error_class);
                assert_eq!(seq.next_attempt_at.is_some(), par.next_attempt_at.is_some());
            }
        },
    );

    server_sequential.join();
    server_parallel.join();
}

#[test]
#[serial]
fn test_run_publish_mode_parity_from_uploaded_checkpoint() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server_sequential = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
        ]),
        3,
    );
    let ws_seq = planned_workspace_with_dependency(td.path(), server_sequential.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.parallel.enabled = false;
    opts_seq.readiness.enabled = false;
    opts_seq.max_attempts = 2;

    let server_parallel = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/2.0.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            ),
        ]),
        3,
    );
    let ws_par = planned_workspace_with_dependency(td.path(), server_parallel.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let mut opts_par = default_opts(state_dir_par.clone());
    opts_par.readiness.enabled = false;
    opts_par.max_attempts = 2;

    let mut state_seq = init_state_with_uploaded_checkpoint(&ws_seq, &pkg_key("base", "1.0.0"));
    let mut state_par = init_state_with_uploaded_checkpoint(&ws_par, &pkg_key("base", "1.0.0"));
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_reporter = CollectingReporter::default();

    let mut par_reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("0")),
        ],
        || {
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut state_seq,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            let mut par_receipts = run_publish_parallel(
                &ws_par,
                &opts_par,
                &mut state_par,
                &state_dir_par,
                &reg_par,
                &mut par_reporter,
            )
            .expect("parallel publish");

            let mut seq_receipts = seq_receipts;
            seq_receipts.sort_by(|a, b| a.name.cmp(&b.name));
            par_receipts.sort_by(|a, b| a.name.cmp(&b.name));

            assert_eq!(
                seq_receipts.len(),
                par_receipts.len(),
                "receipt counts match"
            );

            for (seq, par) in seq_receipts.iter().zip(par_receipts.iter()) {
                assert_eq!(seq.name, par.name);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.state, par.state);
                assert_eq!(seq.attempts, par.attempts);
            }

            for key in ["base@1.0.0", "dependent@2.0.0"] {
                let seq_pkg = state_seq.packages.get(key).expect("sequential state");
                let par_pkg = state_par.packages.get(key).expect("parallel state");
                assert_eq!(seq_pkg.state, par_pkg.state);
                assert_eq!(seq_pkg.attempts, par_pkg.attempts);
            }

            let mut seq_attempt_history = state_seq.attempt_history.clone();
            let mut par_attempt_history = state_par.attempt_history.clone();
            sort_attempt_history_by_fields(&mut seq_attempt_history);
            sort_attempt_history_by_fields(&mut par_attempt_history);

            assert_eq!(
                seq_attempt_history.len(),
                par_attempt_history.len(),
                "attempt history length"
            );

            for (seq, par) in seq_attempt_history.iter().zip(par_attempt_history.iter()) {
                assert_eq!(seq.package, par.package);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.attempt, par.attempt);
                assert_eq!(seq.max_attempts, par.max_attempts);
                assert_eq!(seq.error_class, par.error_class);
                assert_eq!(seq.next_attempt_at.is_some(), par.next_attempt_at.is_some());
            }
        },
    );

    server_sequential.join();
    server_parallel.join();
}

#[test]
#[serial]
fn test_run_publish_mode_parity_from_pending_checkpoint_with_previous_attempt() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server_sequential = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );
    let ws_seq = planned_workspace(td.path(), server_sequential.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.parallel.enabled = false;
    opts_seq.readiness.enabled = false;
    opts_seq.max_attempts = 2;

    let server_parallel = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );
    let ws_par = planned_workspace(td.path(), server_parallel.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let mut opts_par = default_opts(state_dir_par.clone());
    opts_par.readiness.enabled = false;
    opts_par.max_attempts = 2;

    let mut state_seq =
        init_state_with_checkpoint(&ws_seq, &pkg_key("demo", "0.1.0"), PackageState::Pending, 1);
    let mut state_par =
        init_state_with_checkpoint(&ws_par, &pkg_key("demo", "0.1.0"), PackageState::Pending, 1);
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_reporter = CollectingReporter::default();
    let mut par_reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("0")),
        ],
        || {
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut state_seq,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            let mut par_receipts = run_publish_parallel(
                &ws_par,
                &opts_par,
                &mut state_par,
                &state_dir_par,
                &reg_par,
                &mut par_reporter,
            )
            .expect("parallel publish");

            let mut seq_receipts = seq_receipts;
            seq_receipts.sort_by(|a, b| a.name.cmp(&b.name));
            par_receipts.sort_by(|a, b| a.name.cmp(&b.name));

            assert_eq!(
                seq_receipts.len(),
                par_receipts.len(),
                "receipt counts match"
            );

            for (seq, par) in seq_receipts.iter().zip(par_receipts.iter()) {
                assert_eq!(seq.name, par.name);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.state, par.state);
                assert_eq!(seq.attempts, par.attempts);
            }

            let mut seq_attempt_history = state_seq.attempt_history.clone();
            let mut par_attempt_history = state_par.attempt_history.clone();
            sort_attempt_history_by_fields(&mut seq_attempt_history);
            sort_attempt_history_by_fields(&mut par_attempt_history);

            assert_eq!(
                seq_attempt_history.len(),
                par_attempt_history.len(),
                "attempts"
            );
            for (seq, par) in seq_attempt_history.iter().zip(par_attempt_history.iter()) {
                assert_eq!(seq.package, par.package);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.attempt, par.attempt);
                assert_eq!(seq.max_attempts, par.max_attempts);
                assert_eq!(seq.error_class, par.error_class);
                assert_eq!(seq.next_attempt_at.is_some(), par.next_attempt_at.is_some());
            }
        },
    );

    server_sequential.join();
    server_parallel.join();
}

#[test]
#[serial]
fn test_run_publish_mode_parity_from_ambiguous_checkpoint() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server_sequential = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );
    let ws_seq = planned_workspace(td.path(), server_sequential.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.parallel.enabled = false;
    opts_seq.readiness.enabled = false;
    opts_seq.max_attempts = 1;

    let server_parallel = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(404, "{}".to_string()), (200, "{}".to_string())],
        )]),
        2,
    );
    let ws_par = planned_workspace(td.path(), server_parallel.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let mut opts_par = default_opts(state_dir_par.clone());
    opts_par.readiness.enabled = false;
    opts_par.max_attempts = 1;

    let mut state_seq = init_state_with_checkpoint(
        &ws_seq,
        &pkg_key("demo", "0.1.0"),
        PackageState::Ambiguous {
            message: "previous ambiguous upload".to_string(),
        },
        0,
    );
    let mut state_par = init_state_with_checkpoint(
        &ws_par,
        &pkg_key("demo", "0.1.0"),
        PackageState::Ambiguous {
            message: "previous ambiguous upload".to_string(),
        },
        0,
    );
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_reporter = CollectingReporter::default();
    let mut par_reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("0")),
        ],
        || {
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut state_seq,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            let mut par_receipts = run_publish_parallel(
                &ws_par,
                &opts_par,
                &mut state_par,
                &state_dir_par,
                &reg_par,
                &mut par_reporter,
            )
            .expect("parallel publish");

            let mut seq_receipts = seq_receipts;
            seq_receipts.sort_by(|a, b| a.name.cmp(&b.name));
            par_receipts.sort_by(|a, b| a.name.cmp(&b.name));

            assert_eq!(
                seq_receipts.len(),
                par_receipts.len(),
                "receipt counts match"
            );

            for (seq, par) in seq_receipts.iter().zip(par_receipts.iter()) {
                assert_eq!(seq.name, par.name);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.state, par.state);
                assert_eq!(seq.attempts, par.attempts);
            }

            let mut seq_attempt_history = state_seq.attempt_history.clone();
            let mut par_attempt_history = state_par.attempt_history.clone();
            sort_attempt_history_by_fields(&mut seq_attempt_history);
            sort_attempt_history_by_fields(&mut par_attempt_history);

            assert_eq!(
                seq_attempt_history.len(),
                par_attempt_history.len(),
                "attempt histories"
            );
            for (seq, par) in seq_attempt_history.iter().zip(par_attempt_history.iter()) {
                assert_eq!(seq.package, par.package);
                assert_eq!(seq.version, par.version);
                assert_eq!(seq.attempt, par.attempt);
                assert_eq!(seq.max_attempts, par.max_attempts);
                assert_eq!(seq.error_class, par.error_class);
                assert_eq!(seq.next_attempt_at.is_some(), par.next_attempt_at.is_some());
            }
        },
    );

    server_sequential.join();
    server_parallel.join();
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
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter = make_send_reporter();

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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
                    regime: None,
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("b").join("Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("c").join("Cargo.toml"),
                    regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
                    regime: None,
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
            regime: None,
        },
        PlannedPackage {
            name: "beta".to_string(),
            version: "0.1.0".to_string(),
            manifest_path: td.path().join("beta").join("Cargo.toml"),
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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

    // The public parallel entry point announces the run start. The outer
    // publish finalizer owns PublishCompleted so serial and parallel terminal
    // notifications stay in one place.
    let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
    let webhook_url = format!("http://{}", webhook_server.server_addr());
    let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
    let webhook_received_clone = Arc::clone(&webhook_received);

    let webhook_handle = std::thread::spawn(move || {
        if let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_secs(2)) {
            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
            let text = String::from_utf8_lossy(&body).to_string();
            webhook_received_clone.lock().unwrap().push(text);
            req.respond(Response::from_string("ok")).expect("respond");
        }
        while let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_millis(50)) {
            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
            let text = String::from_utf8_lossy(&body).to_string();
            webhook_received_clone.lock().unwrap().push(text);
            req.respond(Response::from_string("ok")).expect("respond");
            if webhook_received_clone.lock().unwrap().len() > 1 {
                break;
            }
        }
    });

    let ws = planned_workspace(td.path(), registry_server.base_url.clone());
    let reg = test_registry_client(&ws);
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
                super::run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter)
                    .expect("parallel publish");
        },
    );

    // Give the async webhook thread time to deliver
    std::thread::sleep(Duration::from_millis(500));
    webhook_handle.join().expect("webhook thread");
    registry_server.join();

    let received = webhook_received.lock().unwrap();
    assert_eq!(received.len(), 1, "expected only PublishStarted");
    assert!(received[0].contains("PublishStarted") || received[0].contains("publish_started"));
}

#[test]
#[serial]
fn test_webhook_failure_is_non_blocking_and_mode_parity_holds() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
    let webhook_url = format!("http://{}", webhook_server.server_addr());
    let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
    let webhook_received_clone = Arc::clone(&webhook_received);
    let webhook_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let webhook_done_clone = std::sync::Arc::clone(&webhook_done);

    let webhook_handle = std::thread::spawn(move || {
        let mut total = 0usize;
        while !webhook_done_clone.load(std::sync::atomic::Ordering::Acquire) {
            if let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_millis(100)) {
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                let text = String::from_utf8_lossy(&body).to_string();
                webhook_received_clone.lock().unwrap().push(text);
                total = total.saturating_add(1);
                req.respond(Response::from_string("down").with_status_code(StatusCode(503)))
                    .expect("respond");
                if total >= 8 {
                    break;
                }
            }
        }
    });

    let registry_server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![
                (404, "{}".to_string()),
                (200, "{}".to_string()),
                (404, "{}".to_string()),
                (200, "{}".to_string()),
            ],
        )]),
        4,
    );

    let ws_seq = planned_workspace(td.path(), registry_server.base_url.clone());
    let reg_seq = test_registry_client(&ws_seq);
    let state_dir_seq = td.path().join(".shipper-seq");
    fs::create_dir_all(&state_dir_seq).expect("mkdir");
    let mut opts_seq = default_opts(state_dir_seq.clone());
    opts_seq.webhook = shipper_webhook::WebhookConfig {
        url: webhook_url.clone(),
        ..Default::default()
    };
    opts_seq.readiness.enabled = false;
    let seq_events_path = events::events_path(&state_dir_seq);
    let mut seq_event_log = events::EventLog::new();
    let mut seq_state =
        init_state_for_package(&ws_seq.plan.plan_id, &ws_seq.plan.registry, "demo", "0.1.0");
    let mut seq_reporter = CollectingReporter::default();

    let ws_par = planned_workspace(td.path(), registry_server.base_url.clone());
    let reg_par = test_registry_client(&ws_par);
    let state_dir_par = td.path().join(".shipper-par");
    fs::create_dir_all(&state_dir_par).expect("mkdir");
    let mut opts_par = default_opts(state_dir_par.clone());
    opts_par.webhook = shipper_webhook::WebhookConfig {
        url: webhook_url,
        ..Default::default()
    };
    opts_par.readiness.enabled = false;
    let mut par_state =
        init_state_for_package(&ws_par.plan.plan_id, &ws_par.plan.registry, "demo", "0.1.0");
    let mut par_reporter = CollectingReporter::default();

    temp_env::with_var(
        "SHIPPER_CARGO_BIN",
        Some(fake_cargo_path(&bin).to_str().expect("utf8")),
        || {
            crate::engine::publish::notify_publish_started(&ws_seq, &opts_seq);
            let seq_receipts = crate::engine::execute_package::run_sequential_scheduler(
                &ws_seq,
                &opts_seq,
                &mut seq_state,
                &state_dir_seq,
                &reg_seq,
                &mut seq_event_log,
                &seq_events_path,
                &mut seq_reporter,
            )
            .expect("sequential publish");

            temp_env::with_var("SHIPPER_CARGO_EXIT", Some("0"), || {
                let par_receipts = super::run_publish_parallel(
                    &ws_par,
                    &opts_par,
                    &mut par_state,
                    &state_dir_par,
                    &reg_par,
                    &mut par_reporter,
                )
                .expect("parallel publish");

                assert_eq!(
                    seq_receipts.len(),
                    par_receipts.len(),
                    "receipt counts should match"
                );
                assert_eq!(seq_state.packages.len(), par_state.packages.len());

                let seq_pkg = seq_state
                    .packages
                    .get("demo@0.1.0")
                    .expect("sequential package");
                let par_pkg = par_state
                    .packages
                    .get("demo@0.1.0")
                    .expect("parallel package");
                assert_eq!(seq_pkg.state, par_pkg.state);
                assert_eq!(seq_pkg.attempts, par_pkg.attempts);
            });
        },
    );

    webhook_done.store(true, std::sync::atomic::Ordering::Release);
    registry_server.join();
    webhook_handle.join().expect("webhook thread");

    let received = webhook_received.lock().unwrap();
    assert!(
        received.len() >= 2,
        "expected webhook delivery attempts despite webhook failures"
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
                    regime: None,
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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

            assert_eq!(
                receipts.len(),
                1,
                "only packages at and after resume point receive receipts"
            );

            // dependent was actually processed
            assert_eq!(receipts[0].name, "dependent");
            assert!(
                matches!(receipts[0].state, PackageState::Skipped { .. }),
                "dependent should be Skipped (already on registry), got {:?}",
                receipts[0].state
            );

            let events_path = events::events_path(&state_dir);
            let events = events::EventLog::read_from_file(&events_path).expect("read events");
            assert!(
                events.all_events().iter().any(|event| {
                    event.package == "base@1.0.0"
                        && matches!(event.event_type, EventType::PackageSkipped { .. })
                }),
                "terminal package before resume point must have a durable skip event"
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

#[test]
#[serial]
fn test_resume_from_skip_mode_parity_preserves_durable_evidence() {
    let run_case = |parallel: bool| {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_args = td.path().join("cargo-args.log");
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
                plan_id: "plan-resume-parity".to_string(),
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
                        regime: None,
                    },
                    PlannedPackage {
                        name: "dependent".to_string(),
                        version: "2.0.0".to_string(),
                        manifest_path: td.path().join("dependent").join("Cargo.toml"),
                        regime: None,
                    },
                ],
                dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
            },
            skipped: vec![],
        };
        let reg = test_registry_client(&ws);
        let state_dir = td.path().join(".shipper");
        let events_path = events::events_path(&state_dir);
        let mut opts = default_opts(state_dir.clone());
        opts.resume_from = Some("dependent".to_string());
        opts.readiness.enabled = false;

        let mut packages = BTreeMap::new();
        packages.insert(
            pkg_key("base", "1.0.0"),
            PackageProgress {
                name: "base".to_string(),
                version: "1.0.0".to_string(),
                attempts: 2,
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
        let mut state = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: vec![],
            packages,
        };
        let mut reporter = CollectingReporter::default();
        let cargo_bin = fake_cargo_path(&bin).to_str().expect("utf8").to_string();
        let cargo_args_path = cargo_args.to_str().expect("utf8").to_string();
        let receipts = temp_env::with_vars(
            [
                ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
                ("SHIPPER_CARGO_ARGS_LOG", Some(cargo_args_path)),
            ],
            || {
                if parallel {
                    run_publish_parallel(&ws, &opts, &mut state, &state_dir, &reg, &mut reporter)
                } else {
                    let mut event_log = events::EventLog::new();
                    crate::engine::execute_package::run_sequential_scheduler(
                        &ws,
                        &opts,
                        &mut state,
                        &state_dir,
                        &reg,
                        &mut event_log,
                        &events_path,
                        &mut reporter,
                    )
                }
            },
        )
        .expect("resume-from publish");
        server.join();

        let event_log = events::EventLog::read_from_file(&events_path).expect("read events");
        let signatures = event_log
            .all_events()
            .iter()
            .map(|event| format!("{}::{:?}", event.package, event.event_type))
            .collect::<Vec<_>>();
        let cargo_invocations = fs::read_to_string(&cargo_args).unwrap_or_default();
        (receipts, state, signatures, cargo_invocations)
    };

    let (sequential_receipts, sequential_state, sequential_events, sequential_cargo) =
        run_case(false);
    let (parallel_receipts, parallel_state, parallel_events, parallel_cargo) = run_case(true);

    assert_eq!(sequential_events, parallel_events);
    assert_eq!(sequential_cargo, parallel_cargo);
    assert!(sequential_cargo.trim().is_empty());
    assert_eq!(sequential_receipts.len(), 1);
    assert_eq!(parallel_receipts.len(), 1);
    assert_eq!(sequential_receipts[0].name, "dependent");
    assert_eq!(parallel_receipts[0].name, "dependent");
    assert_eq!(sequential_receipts[0].state, parallel_receipts[0].state);
    assert_eq!(
        sequential_state.packages.len(),
        parallel_state.packages.len()
    );
    for (key, sequential_progress) in &sequential_state.packages {
        let parallel_progress = parallel_state.packages.get(key).expect("parallel package");
        assert_eq!(sequential_progress.name, parallel_progress.name);
        assert_eq!(sequential_progress.version, parallel_progress.version);
        assert_eq!(sequential_progress.attempts, parallel_progress.attempts);
        assert_eq!(sequential_progress.state, parallel_progress.state);
    }
    assert_eq!(
        sequential_state.attempt_history.len(),
        parallel_state.attempt_history.len(),
        "attempt history length"
    );
    for (sequential, parallel) in sequential_state
        .attempt_history
        .iter()
        .zip(parallel_state.attempt_history.iter())
    {
        assert_eq!(sequential.package, parallel.package);
        assert_eq!(sequential.version, parallel.version);
        assert_eq!(sequential.attempt, parallel.attempt);
        assert_eq!(sequential.max_attempts, parallel.max_attempts);
        assert_eq!(sequential.error_class, parallel.error_class);
        assert_eq!(
            sequential.next_attempt_at.is_some(),
            parallel.next_attempt_at.is_some()
        );
    }
    assert!(sequential_events.iter().any(|event| {
        event.contains("base@1.0.0::PackageSkipped")
            && event.contains("resume: state already published")
    }));
}

#[test]
fn skipped_level_resume_action_reports_poisoned_state_lock() {
    let registry = Registry {
        name: "crates-io".to_string(),
        api_base: "http://127.0.0.1".to_string(),
        index_base: None,
    };
    let st_arc = Arc::new(Mutex::new(init_state_for_package(
        "plan-poison",
        &registry,
        "base",
        "1.0.0",
    )));
    let poisoned_state = Arc::clone(&st_arc);

    let poison_result = std::thread::spawn(move || {
        let _guard = poisoned_state.lock().expect("lock execution state");
        panic!("poison execution state lock for regression test");
    })
    .join();
    assert!(poison_result.is_err(), "test setup should poison the lock");

    let packages = vec![PlannedPackage {
        name: "base".to_string(),
        version: "1.0.0".to_string(),
        manifest_path: PathBuf::from("base/Cargo.toml"),
        regime: None,
    }];

    let action_err = match determine_level_resume_action(&packages, &st_arc, Some("dependent")) {
        Ok(_) => panic!("poisoned state lock should fail resume action selection"),
        Err(err) => err,
    };
    assert!(
        action_err
            .to_string()
            .contains("execution state lock poisoned while checking completed level"),
        "unexpected resume action error: {action_err:#}"
    );
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
                    regime: None,
                },
                PlannedPackage {
                    name: "utils".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("utils").join("Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "app".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("app").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([(
                "app".to_string(),
                vec!["core".to_string(), "utils".to_string()],
            )]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
    let reg = test_registry_client(&ws);
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
                    regime: None,
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/b/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/c/Cargo.toml"),
                    regime: None,
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
                    regime: None,
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/b/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/c/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "d".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/d/Cargo.toml"),
                    regime: None,
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
                    regime: None,
                },
                PlannedPackage {
                    name: "cli".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/cli/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "web".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/web/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "api".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/api/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "worker".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/worker/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "bench".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("/ws/bench/Cargo.toml"),
                    regime: None,
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
                    regime: None,
                },
                PlannedPackage {
                    name: "utils-b".to_string(),
                    version: "0.2.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-b/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "utils-c".to_string(),
                    version: "0.3.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-c/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "utils-d".to_string(),
                    version: "0.4.0".to_string(),
                    manifest_path: PathBuf::from("/ws/utils-d/Cargo.toml"),
                    regime: None,
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
                regime: None,
            },
            PlannedPackage {
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("b/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "c".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("c/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "d".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("d/Cargo.toml"),
                regime: None,
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
                regime: None,
            },
            PlannedPackage {
                name: "utils".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("utils/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "app".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("app/Cargo.toml"),
                regime: None,
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
                regime: None,
            },
            PlannedPackage {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("bar/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("baz/Cargo.toml"),
                regime: None,
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
                regime: None,
            },
            PlannedPackage {
                name: "l2".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l2/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "l3".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l3/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "l4".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l4/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "l5".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("l5/Cargo.toml"),
                regime: None,
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
                    regime: None,
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("b").join("Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("c").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["b".to_string()]),
            ]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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

            let a_key = pkg_key("a", "1.0.0");
            let a_progress = st.packages.get(&a_key).expect("a");
            assert!(
                matches!(a_progress.state, PackageState::Failed { .. }),
                "a should be marked Failed, got {:?}",
                a_progress.state
            );

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

#[test]
#[serial]
fn test_error_in_second_level_preserves_first_level_success_state() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // Level 0 publishes as already existing in registry, so it is marked Skipped.
    // Level 1 depends on level 0 and fails, proving the first level result is still
    // synchronized back on the failure return path.
    let server = spawn_registry_server(
        BTreeMap::from([
            (
                "/api/v1/crates/base/1.0.0".to_string(),
                vec![(200, "{}".to_string())],
            ),
            (
                "/api/v1/crates/dependent/1.0.0".to_string(),
                vec![(404, "{}".to_string())],
            ),
        ]),
        2,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-halt-second-level".to_string(),
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
                    regime: None,
                },
                PlannedPackage {
                    name: "dependent".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("dependent").join("Cargo.toml"),
                    regime: None,
                },
            ],
            dependencies: BTreeMap::from([("dependent".to_string(), vec!["base".to_string()])]),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
            ("SHIPPER_CARGO_STDERR", Some("transient publish failure")),
        ],
        || {
            let result = run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter);

            assert!(result.is_err(), "publish should fail");

            let base_key = pkg_key("base", "1.0.0");
            let base_progress = st.packages.get(&base_key).expect("base");
            assert!(
                matches!(base_progress.state, PackageState::Skipped { .. }),
                "base should be marked Skipped, got {:?}",
                base_progress.state
            );

            let dependent_key = pkg_key("dependent", "1.0.0");
            let dependent_progress = st.packages.get(&dependent_key).expect("dependent");
            assert!(
                matches!(dependent_progress.state, PackageState::Failed { .. }),
                "dependent should be marked Failed, got {:?}",
                dependent_progress.state
            );
            assert_eq!(
                dependent_progress.attempts, 1,
                "expected one failed publish attempt"
            );
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_error_in_level_replays_buffered_messages_to_host() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // A single level failure should still flush buffered reporter messages before
    // returning; previously this path returned early and dropped per-level logs.
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
            plan_id: "plan-halt-level-message".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![PlannedPackage {
                name: "base".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: td.path().join("base").join("Cargo.toml"),
                regime: None,
            }],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1;

    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "base", "1.0.0");
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

            assert!(result.is_err(), "level should fail");

            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|msg| msg.contains("Level 0: publishing")),
                "level message should be replayed to host reporter"
            );
            assert!(
                reporter
                    .errors
                    .iter()
                    .chain(reporter.warns.iter())
                    .any(|msg| msg.contains("base")),
                "level messages should be replayed to host reporter"
            );

            let progress = st
                .packages
                .get("base@1.0.0")
                .expect("base should remain tracked");
            assert!(
                matches!(progress.state, PackageState::Failed { .. }),
                "base should be failed, got {:?}",
                progress.state
            );
        },
    );
    server.join();
}

#[test]
#[serial]
fn test_error_in_level_replays_retryable_messages_to_host() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // A retryable failure path should still surface retry notices and buffered
    // messages before the outer failure return. This guards against dropping
    // operator logs on multi-attempt transient paths.
    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/base/1.0.0".to_string(),
            vec![
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
                (404, "{}".to_string()),
            ],
        )]),
        4,
    );

    let ws = PlannedWorkspace {
        workspace_root: td.path().to_path_buf(),
        plan: ReleasePlan {
            plan_version: "1".to_string(),
            plan_id: "plan-halt-level-retry-message".to_string(),
            created_at: Utc::now(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: server.base_url.clone(),
                index_base: None,
            },
            packages: vec![PlannedPackage {
                name: "base".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: td.path().join("base").join("Cargo.toml"),
                regime: None,
            }],
            dependencies: BTreeMap::new(),
        },
        skipped: vec![],
    };

    let reg = test_registry_client(&ws);
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 2;
    opts.readiness.enabled = false;

    let mut st = init_state_for_package(&ws.plan.plan_id, &ws.plan.registry, "base", "1.0.0");
    let mut reporter = CollectingReporter::default();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("HTTP 503 service unavailable")),
        ],
        || {
            let result = run_publish_parallel(&ws, &opts, &mut st, &state_dir, &reg, &mut reporter);

            assert!(result.is_err(), "level should fail after retry exhaustion");

            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|msg| msg.contains("Level 0: publishing")),
                "level message should be replayed to host reporter"
            );
            assert!(
                reporter
                    .warns
                    .iter()
                    .any(|msg| msg.contains("next attempt")),
                "retry wait should be replayed to host reporter"
            );
            assert!(
                reporter
                    .errors
                    .iter()
                    .chain(reporter.warns.iter())
                    .any(|msg| msg.contains("Retryable") || msg.contains("transient")),
                "failure context should be replayed to host reporter"
            );

            let progress = st
                .packages
                .get("base@1.0.0")
                .expect("base should remain tracked");
            match &progress.state {
                PackageState::Failed { class, .. } => assert_eq!(
                    *class,
                    ErrorClass::Retryable,
                    "expected retryable failure class, got {:?}",
                    class
                ),
                other => panic!("expected failed state, got {:?}", other),
            }
            assert_eq!(progress.attempts, 2, "expected two publish attempts");
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

    let reg = test_registry_client(&ws);
    let state_dir = td.path().join(".shipper");
    let opts = default_opts(state_dir.clone());
    let mut st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        attempt_history: Vec::new(),
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
            regime: None,
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
        regime: None,
    }];
    let mut deps = BTreeMap::new();
    for i in 0..6 {
        let name = format!("leaf-{i}");
        packages.push(PlannedPackage {
            name: name.clone(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from(format!("{name}/Cargo.toml")),
            regime: None,
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
            regime: None,
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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
        packages: state_packages,
    }));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let mut reporter = CollectingReporter::default();
    let send_reporter = make_send_reporter();

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
                &mut reporter,
                &send_reporter,
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
            regime: None,
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

    let reg = test_registry_client(&ws);
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
        attempt_history: Vec::new(),
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
                regime: None,
            }],
            ..ws.plan
        },
        ..ws
    };

    let reg = test_registry_client(&ws);
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
                    regime: None,
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
                    regime: None,
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
                    regime: None,
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
                    regime: None,
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
    // The failure is classified as Ambiguous → reconcile_ambiguous_upload fires and
    // the second reconciler poll sees the version as visible. Expected: the
    // package becomes Published after one retry.
    //
    // Request sequence (readiness disabled):
    //   1. entry "already published" check (execute_package.rs) → 404
    //   2. first reconcile's version_exists (via is_version_visible_with_backoff) → 404
    //   3. second reconcile's version_exists (via is_version_visible_with_backoff) → 200
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
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
            // Two attempts are expected: first attempt ambiguity -> first
            // reconcile miss, second attempt ambiguity -> second reconcile hit.
            assert_eq!(receipt.attempts, 2);
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
    let infos = reporter.drain_infos();
    assert!(
        infos
            .iter()
            .any(|msg| msg.contains("reconciliation outcome: Published")
                && msg.contains("without retry")),
        "expected operator-facing Published reconciliation action, infos: {infos:?}"
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
    //   2. attempt 1 reconcile (enabled=false, single call) → 404 → NotPublished
    //   3. attempt 2 reconcile → 404 → NotPublished
    //   4. post-loop final "if last_err, maybe visible" check (execute_package.rs) → 404
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
            ],
        )]),
        4,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
            match &progress.state {
                PackageState::Failed { class, .. } => {
                    assert_eq!(
                        *class,
                        ErrorClass::Ambiguous,
                        "expected ambiguous failure class after retry exhaustion"
                    );
                }
                other => panic!("expected failed state, got {:?}", other),
            }
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
    let infos = reporter.drain_infos();
    assert!(
        infos
            .iter()
            .any(|msg| msg.contains("reconciliation outcome: NotPublished")
                && msg.contains("retry under publish policy")),
        "expected operator-facing NotPublished retry action, infos: {infos:?}"
    );

    server.join();
}

#[test]
#[serial]
fn publish_package_retry_exhaustion_preserves_retryable_class() {
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
            ],
        )]),
        4,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

    temp_env::with_vars(
        [
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(&bin).to_str().expect("utf8")),
            ),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("HTTP 503 service unavailable")),
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

            let err = result.result.expect_err("retry should eventually fail");
            let msg = err.to_string();
            assert!(
                msg.contains("failed"),
                "expected failure message, got: {msg}"
            );

            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("package progress");
            assert_eq!(progress.attempts, 2, "expected 2 cargo attempts");
            match &progress.state {
                PackageState::Failed { class, .. } => {
                    assert_eq!(
                        *class,
                        ErrorClass::Retryable,
                        "expected retryable failure class after retry exhaustion"
                    );
                }
                other => panic!("expected failed state, got {:?}", other),
            }
        },
    );

    server.join();
}

#[test]
#[serial]
fn reconcile_bdd_resume_from_ambiguous_state_skips_republish() {
    // Scenario (from PR #115 resume-path reconcile):
    //   A prior run left demo@0.1.0 in PackageState::Ambiguous. On resume,
    //   The resume-path reconcile block (execute_package.rs) fires BEFORE the
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
            vec![(200, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

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
    let infos = reporter.drain_infos();
    assert!(
        infos
            .iter()
            .any(|msg| msg.contains("reconciliation outcome: Published")
                && msg.contains("without republish")),
        "expected operator-facing resume Published action, infos: {infos:?}"
    );

    server.join();
}

#[test]
#[serial]
fn reconcile_bdd_resume_from_ambiguous_state_still_unknown_writes_report() {
    // Scenario:
    //   A prior run left demo@0.1.0 in PackageState::Ambiguous. On resume,
    //   the entry "already published" check returns 404, then registry
    //   reconciliation cannot reach truth. Shipper must halt, must not
    //   republish, and must leave a reconciliation artifact for operators.
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(500, "{}".to_string())],
        )]),
        1,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
    let opts = reconcile_scenario_opts(PathBuf::from(".shipper"));
    let state_dir = td.path().join(".shipper");

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
    let reporter = make_send_reporter();
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

            let err = result
                .result
                .expect_err("resume-path StillUnknown must halt with Err");
            let msg = err.to_string();
            assert!(
                msg.contains("resume reconciliation still inconclusive"),
                "expected resume inconclusive error, got: {msg}"
            );
        },
    );

    let cargo_invoked = std::fs::read_to_string(&cargo_log)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    assert!(
        !cargo_invoked,
        "cargo should not be invoked when resume reconciliation is StillUnknown"
    );

    let report_path = crate::state::execution_state::reconciliation_path(&state_dir);
    let report_json = std::fs::read_to_string(&report_path).expect("read reconciliation report");
    let report: shipper_types::ReconciliationReport =
        serde_json::from_str(&report_json).expect("parse reconciliation report");
    assert_eq!(report.schema_version, "shipper.reconciliation.v1");
    assert_eq!(report.records.len(), 1);
    assert_eq!(
        report.records[0].trigger,
        shipper_types::ReconciliationTrigger::ResumeAmbiguousState
    );
    assert_eq!(report.records[0].cargo_exit_class, None);
    assert_eq!(
        report.records[0].operator_action,
        shipper_types::ReconciliationOperatorAction::OperatorActionRequired
    );
    let errors = reporter.drain_errors();
    assert!(
        errors
            .iter()
            .any(|msg| msg.contains("reconciliation outcome: StillUnknown")
                && msg.contains("stop before blind retry")),
        "expected operator-facing StillUnknown stop action, errors: {errors:?}"
    );

    server.join();
}

#[test]
#[serial]
fn reconcile_bdd_ambiguous_resolves_to_still_unknown() {
    // Scenario: cargo exits ambiguously and the registry is itself unhealthy —
    // every reconciliation query returns 5xx. `version_exists` bails with Err
    // for non-200/404 statuses, which `reconcile_ambiguous_upload` translates
    // into `ReconciliationOutcome::StillUnknown`. Expected: the package is
    // marked `PackageState::Ambiguous`, the publish result is Err, cargo is
    // NOT retried (no blind-retry after StillUnknown), and an operator-visible
    // `PublishReconciled { outcome: StillUnknown }` event is persisted.
    //
    // Request sequence (readiness disabled):
    //   1. entry "already published" check (execute_package.rs) → 500 → Err
    //   2. reconcile's single version_exists (via is_version_visible_with_backoff,
    //      enabled=false) → 500 → Err → StillUnknown
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    let server = spawn_registry_server(
        BTreeMap::from([(
            "/api/v1/crates/demo/0.1.0".to_string(),
            vec![(500, "{}".to_string()), (500, "{}".to_string())],
        )]),
        2,
    );

    let ws = planned_workspace(td.path(), server.base_url.clone());
    let reg = test_registry_client(&ws);
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
    let reporter = make_send_reporter();

    // Track cargo invocations to assert we did NOT blind-retry after
    // StillUnknown — exactly one attempt should hit cargo.
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

            let err = result
                .result
                .expect_err("StillUnknown reconciliation must halt with Err");
            let msg = err.to_string();
            assert!(
                msg.contains("reconciliation inconclusive"),
                "expected inconclusive-reconciliation error, got: {msg}"
            );

            // State must be Ambiguous so a subsequent resume triggers the
            // resume-path reconcile block rather than a silent retry.
            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("package progress");
            assert!(
                matches!(progress.state, PackageState::Ambiguous { .. }),
                "expected Ambiguous state after StillUnknown, got {:?}",
                progress.state
            );
            // Exactly one cargo attempt — StillUnknown must not trigger another.
            assert_eq!(
                progress.attempts, 1,
                "cargo should run exactly once; StillUnknown must not blind-retry"
            );
        },
    );

    // Cargo call log corroborates the attempts counter: one invocation total.
    let cargo_invocations = std::fs::read_to_string(&cargo_log)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    assert_eq!(
        cargo_invocations, 1,
        "cargo should have been invoked exactly once"
    );

    // Verify the operator-visible reconciliation events were persisted.
    let persisted = events::EventLog::read_from_file(&events_path).expect("read events");
    let has_reconciling = persisted
        .all_events()
        .iter()
        .any(|e| matches!(e.event_type, EventType::PublishReconciling { .. }));
    let has_reconciled_still_unknown = persisted.all_events().iter().any(|e| {
        matches!(
            &e.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::StillUnknown { .. }
            }
        )
    });
    assert!(has_reconciling, "expected PublishReconciling event");
    assert!(
        has_reconciled_still_unknown,
        "expected PublishReconciled with StillUnknown outcome"
    );
    let errors = reporter.drain_errors();
    assert!(
        errors
            .iter()
            .any(|msg| msg.contains("reconciliation outcome: StillUnknown")
                && msg.contains("stop before blind retry")),
        "expected operator-facing StillUnknown stop action, errors: {errors:?}"
    );
    let report_path = crate::state::execution_state::reconciliation_path(&state_dir);
    let report_json = std::fs::read_to_string(&report_path).expect("read reconciliation report");
    let report: shipper_types::ReconciliationReport =
        serde_json::from_str(&report_json).expect("parse reconciliation report");
    assert_eq!(report.schema_version, "shipper.reconciliation.v1");
    assert_eq!(report.records.len(), 1);
    assert_eq!(
        report.records[0].operator_action,
        shipper_types::ReconciliationOperatorAction::OperatorActionRequired
    );

    server.join();
}

/// Regression test for #418: the "final chance after error" fallback branch
/// (`execute_package.rs`) must emit `PackagePublished` to `events.jsonl`,
/// not just advance `state.json`. Without it, `state.json` says Published
/// while `events.jsonl` has no projection for the package — a drift the
/// finalization consistency check rejects loudly.
///
/// Scenario: cargo exits 1 with a transient error, version_exists returns
/// 404 (initial), 404 (after the failed attempt), then 200 (the "final
/// chance" check finds the package on the registry). The package ends up
/// Published via the buggy branch. We assert the `PackagePublished` event
/// is present in `events.jsonl`.
#[test]
#[serial]
fn publish_package_final_chance_success_emits_package_published_event() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);

    // version_exists: 404 (initial), 404 (after failure), 200 (final chance)
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
    let reg = test_registry_client(&ws);
    let mut opts = default_opts(PathBuf::from(".shipper"));
    opts.max_attempts = 1;
    let state_dir = td.path().join(".shipper");
    std::fs::create_dir_all(&state_dir).expect("mkdir state");
    let st = Arc::new(Mutex::new(init_state_for_package(
        &ws.plan.plan_id,
        &ws.plan.registry,
        "demo",
        "0.1.0",
    )));
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));
    let events_path = events::events_path(&state_dir);
    let reporter = make_send_reporter();

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

            // The package should be reported Published (via the final-chance branch).
            let receipt = result.result.expect("should succeed via final-chance");
            assert!(
                matches!(receipt.state, PackageState::Published),
                "expected Published via final-chance, got {:?}",
                receipt.state
            );

            // State must agree.
            let state = st.lock().unwrap();
            let progress = state.packages.get("demo@0.1.0").expect("pkg");
            assert!(
                matches!(progress.state, PackageState::Published),
                "state.json should be Published, got {:?}",
                progress.state
            );

            // The invariant: events.jsonl MUST contain a PackagePublished event.
            let persisted =
                events::EventLog::read_from_file(&events_path).expect("read events file");
            let has_published = persisted
                .all_events()
                .iter()
                .any(|e| matches!(e.event_type, EventType::PackagePublished { .. }));
            assert!(
                has_published,
                "#418 regression: final-chance Published branch emitted no PackagePublished event \
                 (events.jsonl has {} events: {:?})",
                persisted.all_events().len(),
                persisted
                    .all_events()
                    .iter()
                    .map(|e| format!("{:?}", e.event_type))
                    .collect::<Vec<_>>()
            );
        },
    );
    server.join();
}

// ---------------------------------------------------------------------------
// #153 mode-parity corpus harness
//
// Table-driven scenarios executed in BOTH sequential and parallel modes.
// Asserts package state + attempts parity, and that rebuild_state_from_events
// projects package states consistent with persisted state (with the trusted
// resume-skip allowance for already-terminal packages).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum ModeParityScenario {
    CleanPublish,
    ReadinessTimeoutThenVisible,
    RetryableExhaustion,
    PermanentFailure,
    AlreadyPublishedInState,
    AmbiguousResolvesPublished,
    AmbiguousRetriesWhenNotPublished,
    AmbiguousStillUnknown,
    StillUnknownResume,
    EventWriteFailure,
}

#[derive(Debug)]
struct ModeParityCase {
    name: &'static str,
    scenario: ModeParityScenario,
}

const MODE_PARITY_CORPUS: &[ModeParityCase] = &[
    ModeParityCase {
        name: "clean_publish",
        scenario: ModeParityScenario::CleanPublish,
    },
    ModeParityCase {
        name: "readiness_timeout_then_visible",
        scenario: ModeParityScenario::ReadinessTimeoutThenVisible,
    },
    ModeParityCase {
        name: "retryable_exhaustion",
        scenario: ModeParityScenario::RetryableExhaustion,
    },
    ModeParityCase {
        name: "permanent_failure",
        scenario: ModeParityScenario::PermanentFailure,
    },
    ModeParityCase {
        name: "already_published_in_state",
        scenario: ModeParityScenario::AlreadyPublishedInState,
    },
    ModeParityCase {
        name: "ambiguous_resolves_published",
        scenario: ModeParityScenario::AmbiguousResolvesPublished,
    },
    ModeParityCase {
        name: "ambiguous_retries_when_not_published",
        scenario: ModeParityScenario::AmbiguousRetriesWhenNotPublished,
    },
    ModeParityCase {
        name: "ambiguous_still_unknown",
        scenario: ModeParityScenario::AmbiguousStillUnknown,
    },
    ModeParityCase {
        name: "still_unknown_resume",
        scenario: ModeParityScenario::StillUnknownResume,
    },
    ModeParityCase {
        name: "event_write_failure",
        scenario: ModeParityScenario::EventWriteFailure,
    },
];

#[derive(Debug)]
struct ModeRunOutcome {
    ok: bool,
    error: Option<String>,
    receipts: Vec<PackageReceipt>,
    state: ExecutionState,
    events_path: PathBuf,
    state_dir: PathBuf,
    cargo_args_log: PathBuf,
}

fn mode_parity_routes(
    scenario: ModeParityScenario,
) -> (BTreeMap<String, Vec<(u16, String)>>, usize) {
    match scenario {
        ModeParityScenario::CleanPublish => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        ),
        ModeParityScenario::ReadinessTimeoutThenVisible => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                // Initial version check, first readiness poll (timeout),
                // then the next readiness attempt observes visibility.
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (200, "{}".to_string()),
                ],
            )]),
            3,
        ),
        ModeParityScenario::RetryableExhaustion => (
            // Each failed Cargo attempt performs a negative visibility check;
            // the final readiness check also remains negative.
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                ],
            )]),
            4,
        ),
        ModeParityScenario::PermanentFailure => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            )]),
            2,
        ),
        ModeParityScenario::AlreadyPublishedInState => (
            // Terminal gate must not consult the registry for already-complete
            // packages; zero expected requests keeps the mock from hanging.
            BTreeMap::new(),
            0,
        ),
        ModeParityScenario::AmbiguousResolvesPublished => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                // Initial visibility check, first ambiguous reconciliation,
                // then the second reconciliation observes visibility.
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (200, "{}".to_string()),
                ],
            )]),
            3,
        ),
        ModeParityScenario::AmbiguousRetriesWhenNotPublished => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                // Initial visibility check, one reconciliation per attempt,
                // then the final visibility check after exhaustion.
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                ],
            )]),
            4,
        ),
        ModeParityScenario::AmbiguousStillUnknown => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                // Initial visibility check and the single reconciliation
                // query both fail closed as StillUnknown.
                vec![(500, "{}".to_string()), (500, "{}".to_string())],
            )]),
            2,
        ),
        ModeParityScenario::StillUnknownResume => (
            BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                // Resume-path reconcile with readiness disabled → single query.
                vec![(500, "{}".to_string())],
            )]),
            1,
        ),
        ModeParityScenario::EventWriteFailure => (BTreeMap::new(), 0),
    }
}

fn mode_parity_seed_state(ws: &PlannedWorkspace, scenario: ModeParityScenario) -> ExecutionState {
    match scenario {
        ModeParityScenario::CleanPublish
        | ModeParityScenario::ReadinessTimeoutThenVisible
        | ModeParityScenario::RetryableExhaustion
        | ModeParityScenario::PermanentFailure => init_state_for_workspace(ws),
        ModeParityScenario::AmbiguousResolvesPublished
        | ModeParityScenario::AmbiguousRetriesWhenNotPublished
        | ModeParityScenario::AmbiguousStillUnknown => init_state_for_workspace(ws),
        ModeParityScenario::AlreadyPublishedInState => {
            init_state_with_checkpoint(ws, &pkg_key("demo", "0.1.0"), PackageState::Published, 1)
        }
        ModeParityScenario::StillUnknownResume => init_state_with_checkpoint(
            ws,
            &pkg_key("demo", "0.1.0"),
            PackageState::Ambiguous {
                message: "prior reconciliation inconclusive".to_string(),
            },
            1,
        ),
        ModeParityScenario::EventWriteFailure => init_state_for_workspace(ws),
    }
}

fn mode_parity_cargo_env<'a>(
    scenario: ModeParityScenario,
    cargo_bin: &'a str,
    cargo_args_log: &'a str,
) -> Vec<(&'static str, Option<&'a str>)> {
    match scenario {
        ModeParityScenario::CleanPublish
        | ModeParityScenario::ReadinessTimeoutThenVisible
        | ModeParityScenario::AlreadyPublishedInState => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_EXIT", Some("0")),
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        ModeParityScenario::RetryableExhaustion => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_ARGS_LOG", Some(cargo_args_log)),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("timeout talking to server")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        ModeParityScenario::AmbiguousResolvesPublished
        | ModeParityScenario::AmbiguousRetriesWhenNotPublished
        | ModeParityScenario::AmbiguousStillUnknown => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_ARGS_LOG", Some(cargo_args_log)),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        ModeParityScenario::PermanentFailure => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_EXIT", Some("1")),
            ("SHIPPER_CARGO_STDERR", Some("permission denied")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        ModeParityScenario::StillUnknownResume => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_EXIT", Some("0")),
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
        ModeParityScenario::EventWriteFailure => vec![
            ("SHIPPER_CARGO_BIN", Some(cargo_bin)),
            ("SHIPPER_CARGO_ARGS_LOG", Some(cargo_args_log)),
            ("SHIPPER_CARGO_EXIT", Some("0")),
            ("SHIPPER_CARGO_STDERR", Some("")),
            ("SHIPPER_CARGO_STDOUT", Some("")),
        ],
    }
}

fn run_mode_parity_case(
    scenario: ModeParityScenario,
    parallel: bool,
    workspace_root: &Path,
    cargo_bin: &str,
) -> ModeRunOutcome {
    let (routes, expected_requests) = mode_parity_routes(scenario);
    let server = if expected_requests == 0 {
        None
    } else {
        Some(spawn_registry_server(routes, expected_requests))
    };
    let api_base = server
        .as_ref()
        .map(|s| s.base_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:9".to_string());

    let label = if parallel { "par" } else { "seq" };
    let state_dir = workspace_root.join(format!(".shipper-{label}"));
    fs::create_dir_all(&state_dir).expect("mkdir state dir");

    let ws = planned_workspace(workspace_root, api_base);
    let reg = test_registry_client(&ws);
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = parallel;
    opts.readiness.enabled = matches!(scenario, ModeParityScenario::ReadinessTimeoutThenVisible);
    opts.max_attempts = if matches!(
        scenario,
        ModeParityScenario::ReadinessTimeoutThenVisible
            | ModeParityScenario::RetryableExhaustion
            | ModeParityScenario::AmbiguousResolvesPublished
            | ModeParityScenario::AmbiguousRetriesWhenNotPublished
    ) {
        2
    } else {
        1
    };
    if matches!(
        scenario,
        ModeParityScenario::AmbiguousResolvesPublished
            | ModeParityScenario::AmbiguousRetriesWhenNotPublished
            | ModeParityScenario::AmbiguousStillUnknown
    ) {
        opts.readiness.enabled = false;
    }
    if matches!(scenario, ModeParityScenario::ReadinessTimeoutThenVisible) {
        opts.base_delay = Duration::ZERO;
        opts.max_delay = Duration::ZERO;
        opts.readiness.initial_delay = Duration::ZERO;
        opts.readiness.max_delay = Duration::ZERO;
        opts.readiness.max_total_wait = Duration::ZERO;
        opts.readiness.poll_interval = Duration::ZERO;
        opts.readiness.jitter_factor = 0.0;
    }

    let mut state = mode_parity_seed_state(&ws, scenario);
    let events_path = events::events_path(&state_dir);
    let cargo_args_log = state_dir.join("cargo-args.log");
    let cargo_args_log_path = cargo_args_log.to_str().expect("cargo args log path");
    if matches!(scenario, ModeParityScenario::EventWriteFailure) {
        // A directory at the event-log path fails on every supported platform
        // without relying on administrator privileges or readonly semantics.
        fs::create_dir_all(&events_path).expect("event path directory");
    }
    let mut event_log = events::EventLog::new();
    let mut reporter = CollectingReporter::default();

    let env = mode_parity_cargo_env(scenario, cargo_bin, cargo_args_log_path);
    let result = temp_env::with_vars(env, || {
        if parallel {
            run_publish_parallel(&ws, &opts, &mut state, &state_dir, &reg, &mut reporter)
        } else {
            crate::engine::execute_package::run_sequential_scheduler(
                &ws,
                &opts,
                &mut state,
                &state_dir,
                &reg,
                &mut event_log,
                &events_path,
                &mut reporter,
            )
        }
    });
    let error = result.as_ref().err().map(ToString::to_string);
    let ok = result.is_ok();
    let receipts = result.as_ref().ok().cloned().unwrap_or_default();

    if let Some(server) = server {
        server.join();
    }

    ModeRunOutcome {
        ok,
        error,
        receipts,
        state,
        events_path,
        state_dir,
        cargo_args_log,
    }
}

fn assert_mode_parity_rebuild(outcome: &ModeRunOutcome, scenario: ModeParityScenario, mode: &str) {
    assert!(
        outcome.events_path.exists(),
        "{mode}: expected events.jsonl for scenario {scenario:?} (ok={})",
        outcome.ok
    );

    let rebuilt = rebuild_state_from_events(
        &outcome.events_path,
        StateRebuildOptions::new(outcome.state.registry.clone())
            .with_fallback_plan_id(&outcome.state.plan_id),
    )
    .unwrap_or_else(|e| panic!("{mode}: rebuild_state_from_events failed: {e}"));

    let trusted_skips: std::collections::BTreeSet<String> =
        events::EventLog::read_from_file(&outcome.events_path)
            .expect("read events for trusted skips")
            .all_events()
            .iter()
            .filter_map(|event| match &event.event_type {
                EventType::PackageSkipped { reason }
                    if reason.starts_with("resume: state already ") =>
                {
                    Some(event.package.clone())
                }
                _ => None,
            })
            .collect();

    for (key, progress) in &outcome.state.packages {
        match rebuilt.packages.get(key) {
            Some(event_progress) if event_progress.state == progress.state => {}
            Some(event_progress)
                if matches!(
                    (&progress.state, &event_progress.state),
                    (PackageState::Published, PackageState::Skipped { .. })
                        | (PackageState::Skipped { .. }, PackageState::Skipped { .. })
                ) && trusted_skips.contains(key) => {}
            Some(event_progress) => panic!(
                "{mode}: rebuild drift for {key}: state={:?} rebuild={:?}",
                progress.state, event_progress.state
            ),
            None if matches!(progress.state, PackageState::Pending) => {}
            None => panic!("{mode}: rebuild missing {key} (state={:?})", progress.state),
        }
    }

    // Attempt history length is stable across rebuild for these scenarios
    // (no in-flight attempt bookkeeping that rebuild cannot represent).
    assert_eq!(
        outcome.state.attempt_history.len(),
        rebuilt.attempt_history.len(),
        "{mode}: attempt_history length must match rebuild (state={} rebuild={})",
        outcome.state.attempt_history.len(),
        rebuilt.attempt_history.len()
    );

    // Silence unused field warning for state_dir in future corpus extensions.
    let _ = &outcome.state_dir;
}

fn stable_event_write_error_contract(error: &str) -> Option<&str> {
    let start_marker = "failed to write package-start event";
    let end_marker = "failed to open events file";
    let start = error.find(start_marker)?;
    let end = error[start..].find(end_marker)? + start + end_marker.len();
    Some(&error[start..end])
}

fn assert_mode_parity_pair(case: &ModeParityCase, seq: &ModeRunOutcome, par: &ModeRunOutcome) {
    assert_eq!(
        seq.ok, par.ok,
        "{}: success/failure parity (seq.ok={} par.ok={})",
        case.name, seq.ok, par.ok
    );

    assert_eq!(
        seq.state.packages.len(),
        par.state.packages.len(),
        "{}: package count",
        case.name
    );

    for (key, seq_pkg) in &seq.state.packages {
        let par_pkg = par
            .state
            .packages
            .get(key)
            .unwrap_or_else(|| panic!("{}: parallel missing {key}", case.name));
        assert_eq!(
            seq_pkg.state, par_pkg.state,
            "{}: package state for {key}",
            case.name
        );
        assert_eq!(
            seq_pkg.attempts, par_pkg.attempts,
            "{}: package attempts for {key}",
            case.name
        );
    }

    assert_receipt_parity(case.name, seq, par);
    if !matches!(case.scenario, ModeParityScenario::EventWriteFailure) {
        assert_semantic_event_sequences_match(seq, par, case.name);
    }

    match case.scenario {
        ModeParityScenario::CleanPublish => {
            assert!(seq.ok, "{}: clean publish should succeed", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Published),
                "{}: expected Published, got {:?}",
                case.name,
                pkg.state
            );
        }
        ModeParityScenario::ReadinessTimeoutThenVisible => {
            assert!(seq.ok, "{}: readiness recovery should succeed", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Published),
                "{}: expected Published after readiness recovery, got {:?}",
                case.name,
                pkg.state
            );

            for (mode, path) in [("seq", &seq.events_path), ("par", &par.events_path)] {
                let persisted = events::EventLog::read_from_file(path)
                    .unwrap_or_else(|e| panic!("{} {mode}: read events: {e}", case.name));
                let all_events = persisted.all_events();
                let timeout_count = all_events
                    .iter()
                    .filter(|event| matches!(event.event_type, EventType::ReadinessTimeout { .. }))
                    .count();
                assert_eq!(
                    timeout_count, 1,
                    "{} {mode}: expected exactly one readiness timeout event",
                    case.name
                );
                let timeout_index = all_events
                    .iter()
                    .position(|event| {
                        matches!(event.event_type, EventType::ReadinessTimeout { .. })
                    })
                    .unwrap_or_else(|| {
                        panic!("{} {mode}: expected readiness timeout event", case.name)
                    });
                let complete_count = all_events
                    .iter()
                    .filter(|event| matches!(event.event_type, EventType::ReadinessComplete { .. }))
                    .count();
                assert_eq!(
                    complete_count, 1,
                    "{} {mode}: expected exactly one readiness complete event",
                    case.name
                );
                let complete_index = all_events
                    .iter()
                    .position(|event| {
                        matches!(event.event_type, EventType::ReadinessComplete { .. })
                    })
                    .unwrap_or_else(|| {
                        panic!("{} {mode}: expected readiness complete event", case.name)
                    });
                let published_index = all_events
                    .iter()
                    .position(|event| {
                        matches!(event.event_type, EventType::PackagePublished { .. })
                    })
                    .unwrap_or_else(|| {
                        panic!("{} {mode}: expected package published event", case.name)
                    });

                assert!(
                    timeout_index < complete_index && complete_index < published_index,
                    "{} {mode}: readiness recovery events out of order",
                    case.name
                );
                assert_eq!(
                    all_events
                        .iter()
                        .filter(|event| matches!(event.event_type, EventType::PackageUploaded))
                        .count(),
                    1,
                    "{} {mode}: retry after readiness timeout must not re-upload",
                    case.name
                );
            }
        }
        ModeParityScenario::RetryableExhaustion => {
            assert!(!seq.ok, "{}: retryable exhaustion should err", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(
                    pkg.state,
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        ..
                    }
                ),
                "{}: expected Failed/Retryable, got {:?}",
                case.name,
                pkg.state
            );
            for (mode, outcome) in [("seq", seq), ("par", par)] {
                assert_eq!(
                    cargo_invocation_count(outcome),
                    2,
                    "{} {mode}: retryable failure must exhaust exactly two Cargo attempts",
                    case.name
                );
            }
        }
        ModeParityScenario::PermanentFailure => {
            assert!(!seq.ok, "{}: permanent failure should err", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(
                    pkg.state,
                    PackageState::Failed {
                        class: ErrorClass::Permanent,
                        ..
                    }
                ),
                "{}: expected Failed/Permanent, got {:?}",
                case.name,
                pkg.state
            );
        }
        ModeParityScenario::AlreadyPublishedInState => {
            assert!(seq.ok, "{}: terminal gate should succeed", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Published),
                "{}: state must remain Published, got {:?}",
                case.name,
                pkg.state
            );
            // Both modes must emit the trusted resume-skip event.
            for (mode, path) in [("seq", &seq.events_path), ("par", &par.events_path)] {
                let raw = fs::read_to_string(path)
                    .unwrap_or_else(|e| panic!("{} {mode}: read events: {e}", case.name));
                assert!(
                    raw.contains("resume: state already")
                        && (raw.contains("package_skipped") || raw.contains("PackageSkipped")),
                    "{} {mode}: expected resume terminal-skip event, events:\n{raw}",
                    case.name
                );
            }
        }
        ModeParityScenario::AmbiguousResolvesPublished => {
            assert!(seq.ok, "{}: visible ambiguity should succeed", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Published),
                "{}: expected Published after reconciliation, got {:?}",
                case.name,
                pkg.state
            );
            for (mode, outcome) in [("seq", seq), ("par", par)] {
                assert_eq!(
                    cargo_invocation_count(outcome),
                    2,
                    "{} {mode}: visible ambiguity must retry Cargo once",
                    case.name
                );
                assert_reconciled_event(outcome, "Published", mode, case.name);
            }
        }
        ModeParityScenario::AmbiguousRetriesWhenNotPublished => {
            assert!(!seq.ok, "{}: unresolved ambiguity should fail", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(
                    pkg.state,
                    PackageState::Failed {
                        class: ErrorClass::Ambiguous,
                        ..
                    }
                ),
                "{}: expected Failed/Ambiguous, got {:?}",
                case.name,
                pkg.state
            );
            for (mode, outcome) in [("seq", seq), ("par", par)] {
                assert_eq!(
                    cargo_invocation_count(outcome),
                    2,
                    "{} {mode}: not-published ambiguity must retry exactly once",
                    case.name
                );
                assert_reconciled_event(outcome, "NotPublished", mode, case.name);
            }
        }
        ModeParityScenario::AmbiguousStillUnknown => {
            assert!(
                !seq.ok,
                "{}: StillUnknown must stop with an error",
                case.name
            );
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Ambiguous { .. }),
                "{}: expected Ambiguous after StillUnknown, got {:?}",
                case.name,
                pkg.state
            );
            for (mode, outcome) in [("seq", seq), ("par", par)] {
                assert_eq!(
                    cargo_invocation_count(outcome),
                    1,
                    "{} {mode}: StillUnknown must not blind-retry Cargo",
                    case.name
                );
                assert_reconciled_event(outcome, "StillUnknown", mode, case.name);
            }
        }
        ModeParityScenario::StillUnknownResume => {
            assert!(!seq.ok, "{}: StillUnknown should err", case.name);
            let pkg = seq.state.packages.get("demo@0.1.0").expect("demo");
            assert!(
                matches!(pkg.state, PackageState::Ambiguous { .. }),
                "{}: expected Ambiguous after StillUnknown, got {:?}",
                case.name,
                pkg.state
            );
        }
        ModeParityScenario::EventWriteFailure => {
            assert!(!seq.ok, "{}: event write failure should err", case.name);
            let seq_error = seq
                .error
                .as_deref()
                .unwrap_or("missing error for event write failure");
            let par_error = par
                .error
                .as_deref()
                .unwrap_or("missing error for event write failure");
            for (mode, outcome) in [("seq", seq), ("par", par)] {
                let error = outcome
                    .error
                    .as_deref()
                    .unwrap_or("missing error for event write failure");
                assert!(
                    error.contains("failed to write package-start event")
                        && error.contains("failed to open events file"),
                    "{} {mode}: expected stable event-log write error, got {error}",
                    case.name
                );
                assert!(
                    outcome.events_path.is_dir(),
                    "{} {mode}: event path must remain the failure fixture",
                    case.name
                );
                let pkg = outcome.state.packages.get("demo@0.1.0").expect("demo");
                assert!(
                    matches!(pkg.state, PackageState::Pending) && pkg.attempts == 0,
                    "{} {mode}: event failure must not mutate pending state, got {:?} attempts={}",
                    case.name,
                    pkg.state,
                    pkg.attempts
                );
                assert_eq!(
                    outcome.state.attempt_history.len(),
                    0,
                    "{} {mode}: event failure must not fabricate attempt history",
                    case.name
                );
                if outcome.cargo_args_log.exists() {
                    let args = fs::read_to_string(&outcome.cargo_args_log).unwrap_or_else(|e| {
                        panic!("{} {mode}: read cargo args log: {e}", case.name)
                    });
                    assert!(
                        args.trim().is_empty(),
                        "{} {mode}: Cargo must not run before the event-write failure, args: {args:?}",
                        case.name
                    );
                }
            }
            let seq_contract = stable_event_write_error_contract(seq_error)
                .expect("sequential event-write contract");
            let par_contract = stable_event_write_error_contract(par_error)
                .expect("parallel event-write contract");
            assert_eq!(
                seq_contract, par_contract,
                "{}: sequential and parallel event-write errors must share the same stable contract",
                case.name
            );
        }
    }

    if !matches!(case.scenario, ModeParityScenario::EventWriteFailure) {
        assert_mode_parity_rebuild(seq, case.scenario, &format!("{}:seq", case.name));
        assert_mode_parity_rebuild(par, case.scenario, &format!("{}:par", case.name));
    }
}

fn cargo_invocation_count(outcome: &ModeRunOutcome) -> usize {
    fs::read_to_string(&outcome.cargo_args_log)
        .map(|contents| {
            contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        })
        .unwrap_or(0)
}

fn assert_receipt_parity(case_name: &str, seq: &ModeRunOutcome, par: &ModeRunOutcome) {
    let seq_receipts = normalized_receipts(&seq.receipts, case_name, "seq");
    let par_receipts = normalized_receipts(&par.receipts, case_name, "par");
    assert_eq!(
        seq_receipts, par_receipts,
        "{case_name}: sequential and parallel receipt evidence differs"
    );
}

fn normalized_receipts(
    receipts: &[PackageReceipt],
    case_name: &str,
    mode: &str,
) -> Vec<serde_json::Value> {
    let mut normalized = receipts
        .iter()
        .map(|receipt| {
            let mut value = serde_json::to_value(receipt)
                .unwrap_or_else(|e| panic!("{case_name} {mode}: serialize receipt: {e}"));
            strip_nondeterministic_receipt_fields(&mut value);
            value
        })
        .collect::<Vec<_>>();
    normalized.sort_by_key(|value| {
        (
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            value
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    });
    normalized
}

fn strip_nondeterministic_receipt_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for field in [
                "started_at",
                "finished_at",
                "duration_ms",
                "timestamp",
                "duration",
            ] {
                fields.remove(field);
            }
            for child in fields.values_mut() {
                strip_nondeterministic_receipt_fields(child);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                strip_nondeterministic_receipt_fields(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn assert_semantic_event_sequences_match(
    seq: &ModeRunOutcome,
    par: &ModeRunOutcome,
    case_name: &str,
) {
    let seq_events = semantic_event_sequence(&seq.events_path, case_name, "seq");
    let par_events = semantic_event_sequence(&par.events_path, case_name, "par");
    assert_eq!(
        seq_events, par_events,
        "{case_name}: sequential and parallel semantic event sequences differ"
    );
}

fn semantic_event_sequence(
    path: &Path,
    case_name: &str,
    mode: &str,
) -> Vec<(String, serde_json::Value)> {
    events::EventLog::read_from_file(path)
        .unwrap_or_else(|e| panic!("{case_name} {mode}: read events: {e}"))
        .all_events()
        .iter()
        .map(|event| {
            let mut event_type = serde_json::to_value(&event.event_type)
                .unwrap_or_else(|e| panic!("{case_name} {mode}: serialize event: {e}"));
            strip_nondeterministic_event_fields(&mut event_type);
            (event.package.clone(), event_type)
        })
        .collect()
}

fn strip_nondeterministic_event_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for field in ["duration_ms", "elapsed_ms", "next_attempt_at", "until"] {
                fields.remove(field);
            }
            for child in fields.values_mut() {
                strip_nondeterministic_event_fields(child);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                strip_nondeterministic_event_fields(value);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn assert_reconciled_event(outcome: &ModeRunOutcome, expected: &str, mode: &str, case_name: &str) {
    let persisted = events::EventLog::read_from_file(&outcome.events_path)
        .unwrap_or_else(|e| panic!("{case_name} {mode}: read events: {e}"));
    let found = persisted.all_events().iter().any(|event| match expected {
        "Published" => matches!(
            &event.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::Published { .. }
            }
        ),
        "NotPublished" => matches!(
            &event.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::NotPublished { .. }
            }
        ),
        "StillUnknown" => matches!(
            &event.event_type,
            EventType::PublishReconciled {
                outcome: shipper_types::ReconciliationOutcome::StillUnknown { .. }
            }
        ),
        _ => false,
    });
    assert!(
        found,
        "{case_name} {mode}: expected PublishReconciled outcome {expected}"
    );
}

#[test]
#[serial]
fn mode_parity_corpus_sequential_matches_parallel() {
    let td = tempdir().expect("tempdir");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);
    let cargo_bin = fake_cargo_path(&bin);
    let cargo_bin = cargo_bin.to_str().expect("utf8");

    for case in MODE_PARITY_CORPUS {
        let case_root = td.path().join(case.name);
        fs::create_dir_all(&case_root).expect("case root");

        let seq = run_mode_parity_case(case.scenario, false, &case_root, cargo_bin);
        let par = run_mode_parity_case(case.scenario, true, &case_root, cargo_bin);
        assert_mode_parity_pair(case, &seq, &par);
    }
}
