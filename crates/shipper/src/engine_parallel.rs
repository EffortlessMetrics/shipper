use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Result, bail};
use chrono::Utc;

use crate::cargo;
use crate::engine::{self, Reporter};
use crate::events;
use crate::registry::RegistryClient;
use crate::state;
use crate::types::{
    self, AttemptEvidence, ErrorClass, EventType, ExecutionState, PackageReceipt, PackageState,
    PlannedPackage, PublishEvent, ReadinessEvidence, RuntimeOptions,
};

use crate::plan::PlannedWorkspace;

/// Result of publishing a single package (for parallel execution)
#[derive(Debug)]
struct PackagePublishResult {
    result: anyhow::Result<PackageReceipt>,
}

/// Publish a single package with retries (parallel-safe version)
#[allow(clippy::too_many_arguments)]
fn publish_package(
    p: &PlannedPackage,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<Mutex<dyn Reporter + Send>>,
) -> PackagePublishResult {
    let key = engine::pkg_key(&p.name, &p.version);
    let pkg_label = format!("{}@{}", p.name, p.version);
    let started_at = Utc::now();
    let start_instant = Instant::now();

    // Record package started event
    {
        let mut log = event_log.lock().unwrap();
        log.record(PublishEvent {
            timestamp: started_at,
            event_type: EventType::PackageStarted {
                name: p.name.clone(),
                version: p.version.clone(),
            },
            package: pkg_label.clone(),
        });
        let _ = log.write_to_file(events_path);
        log.clear();
    }

    // Check if already published
    if let Ok(true) = reg.version_exists(&p.name, &p.version) {
        {
            let mut rep = reporter.lock().unwrap();
            rep.info(&format!(
                "{}@{}: already published (skipping)",
                p.name, p.version
            ));
        }

        let skipped = PackageState::Skipped {
            reason: "already published".into(),
        };
        {
            let mut state = st.lock().unwrap();
            update_state_locked(&mut state, &key, skipped.clone());
            let _ = state::save_state(state_dir, &state);
        }

        // Event: PackageSkipped
        {
            let mut log = event_log.lock().unwrap();
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageSkipped {
                    reason: "already published".to_string(),
                },
                package: pkg_label.clone(),
            });
            let _ = log.write_to_file(events_path);
            log.clear();
        }

        return PackagePublishResult {
            result: Ok(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: skipped,
                started_at,
                finished_at: Utc::now(),
                duration_ms: start_instant.elapsed().as_millis(),
                evidence: types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }),
        };
    }

    {
        let mut rep = reporter.lock().unwrap();
        rep.info(&format!("{}@{}: publishing...", p.name, p.version));
    }

    let mut attempt = 0u32;
    let mut last_err: Option<(ErrorClass, String)> = None;
    let mut attempt_evidence: Vec<AttemptEvidence> = Vec::new();
    let mut readiness_evidence: Vec<ReadinessEvidence> = Vec::new();
    let mut cargo_succeeded = false;

    // Apply policy effects for readiness (Fix 7: parallel mode must respect PublishPolicy::Fast)
    let effects = crate::engine::apply_policy(opts);
    let readiness_config = types::ReadinessConfig {
        enabled: effects.readiness_enabled,
        ..opts.readiness.clone()
    };

    while attempt < opts.max_attempts {
        attempt += 1;
        {
            let mut state = st.lock().unwrap();
            if let Some(pr) = state.packages.get_mut(&key) {
                pr.attempts = attempt;
                pr.last_updated_at = Utc::now();
            }
            let _ = state::save_state(state_dir, &state);
        }

        let command = format!(
            "cargo publish -p {} --registry {}",
            p.name, ws.plan.registry.name
        );

        {
            let mut rep = reporter.lock().unwrap();
            rep.info(&format!(
                "{}@{}: attempt {}/{}",
                p.name, p.version, attempt, opts.max_attempts
            ));
        }

        if !cargo_succeeded {
            // Event: PackageAttempted
            {
                let mut log = event_log.lock().unwrap();
                log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageAttempted {
                        attempt,
                        command: command.clone(),
                    },
                    package: pkg_label.clone(),
                });
            }

            let out = match cargo::cargo_publish(
                &ws.workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
                opts.output_lines,
                Some(opts.parallel.per_package_timeout),
            ) {
                Ok(o) => o,
                Err(e) => {
                    {
                        let mut rep = reporter.lock().unwrap();
                        rep.error(&format!(
                            "{}@{}: cargo publish failed to execute: {}",
                            p.name, p.version, e
                        ));
                    }
                    return PackagePublishResult { result: Err(e) };
                }
            };

            // Collect attempt evidence
            attempt_evidence.push(AttemptEvidence {
                attempt_number: attempt,
                command: command.clone(),
                exit_code: out.exit_code,
                stdout_tail: out.stdout_tail.clone(),
                stderr_tail: out.stderr_tail.clone(),
                timestamp: Utc::now(),
                duration: out.duration,
            });

            // Event: PackageOutput
            {
                let mut log = event_log.lock().unwrap();
                log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageOutput {
                        stdout_tail: out.stdout_tail.clone(),
                        stderr_tail: out.stderr_tail.clone(),
                    },
                    package: pkg_label.clone(),
                });
            }

            if out.exit_code == 0 {
                cargo_succeeded = true;
            } else {
                // Cargo failed, check registry
                {
                    let mut rep = reporter.lock().unwrap();
                    rep.warn(&format!(
                        "{}@{}: cargo publish failed (exit={}); checking registry...",
                        p.name, p.version, out.exit_code
                    ));
                }

                if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
                    {
                        let mut rep = reporter.lock().unwrap();
                        rep.info(&format!(
                            "{}@{}: version is present on registry; treating as published",
                            p.name, p.version
                        ));
                    }

                    {
                        let mut state = st.lock().unwrap();
                        update_state_locked(&mut state, &key, PackageState::Published);
                        let _ = state::save_state(state_dir, &state);
                    }
                    last_err = None;
                    break;
                }

                let (class, msg) =
                    engine::classify_cargo_failure(&out.stderr_tail, &out.stdout_tail);
                last_err = Some((class.clone(), msg.clone()));

                // Event: PackageFailed
                {
                    let mut log = event_log.lock().unwrap();
                    log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackageFailed {
                            class: class.clone(),
                            message: msg.clone(),
                        },
                        package: pkg_label.clone(),
                    });
                }

                match class {
                    ErrorClass::Permanent => {
                        let failed = PackageState::Failed {
                            class,
                            message: msg.clone(),
                        };
                        {
                            let mut state = st.lock().unwrap();
                            update_state_locked(&mut state, &key, failed);
                            let _ = state::save_state(state_dir, &state);
                        }
                        {
                            let mut log = event_log.lock().unwrap();
                            let _ = log.write_to_file(events_path);
                            log.clear();
                        }

                        return PackagePublishResult {
                            result: Err(anyhow::anyhow!(
                                "{}@{}: permanent failure: {}",
                                p.name,
                                p.version,
                                msg
                            )),
                        };
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        let delay = engine::backoff_delay(opts.base_delay, opts.max_delay, attempt);
                        {
                            let mut rep = reporter.lock().unwrap();
                            rep.warn(&format!(
                                "{}@{}: retrying in {}",
                                p.name,
                                p.version,
                                humantime::format_duration(delay)
                            ));
                        }
                        thread::sleep(delay);
                    }
                }
                continue;
            }
        }

        // Readiness verification (runs after first cargo success + all retries)
        {
            let mut rep = reporter.lock().unwrap();
            rep.info(&format!(
                "{}@{}: cargo publish exited successfully; verifying...",
                p.name, p.version
            ));
        }

        let verify_result =
            reg.is_version_visible_with_backoff(&p.name, &p.version, &readiness_config);

        match verify_result {
            Ok((visible, checks)) => {
                readiness_evidence = checks;
                if visible {
                    {
                        let mut state = st.lock().unwrap();
                        update_state_locked(&mut state, &key, PackageState::Published);
                        let _ = state::save_state(state_dir, &state);
                    }
                    last_err = None;

                    // Event: PackagePublished
                    {
                        let mut log = event_log.lock().unwrap();
                        log.record(PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackagePublished {
                                duration_ms: start_instant.elapsed().as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        });
                        let _ = log.write_to_file(events_path);
                        log.clear();
                    }

                    break;
                } else {
                    last_err = Some((ErrorClass::Ambiguous, "publish succeeded locally, but version not observed on registry within timeout".into()));
                    let delay = engine::backoff_delay(opts.base_delay, opts.max_delay, attempt);
                    thread::sleep(delay);
                }
            }
            Err(_) => {
                last_err = Some((ErrorClass::Ambiguous, "readiness check failed".into()));
                let delay = engine::backoff_delay(opts.base_delay, opts.max_delay, attempt);
                thread::sleep(delay);
            }
        }
    }

    let finished_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis();

    if let Some((class, msg)) = last_err {
        // Final chance: maybe it eventually showed up.
        if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
            {
                let mut state = st.lock().unwrap();
                update_state_locked(&mut state, &key, PackageState::Published);
                let _ = state::save_state(state_dir, &state);
            }

            return PackagePublishResult {
                result: Ok(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: st
                        .lock()
                        .unwrap()
                        .packages
                        .get(&key)
                        .map_or(0, |p| p.attempts),
                    state: PackageState::Published,
                    started_at,
                    finished_at,
                    duration_ms,
                    evidence: types::PackageEvidence {
                        attempts: attempt_evidence,
                        readiness_checks: readiness_evidence,
                    },
                }),
            };
        } else {
            let failed = PackageState::Failed {
                class,
                message: msg.clone(),
            };
            {
                let mut state = st.lock().unwrap();
                update_state_locked(&mut state, &key, failed);
                let _ = state::save_state(state_dir, &state);
            }

            // Event: PackageFailed (final)
            {
                let mut log = event_log.lock().unwrap();
                log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: msg.clone(),
                    },
                    package: pkg_label,
                });
                let _ = log.write_to_file(events_path);
                log.clear();
            }

            return PackagePublishResult {
                result: Err(anyhow::anyhow!("{}@{}: failed: {}", p.name, p.version, msg)),
            };
        }
    }

    PackagePublishResult {
        result: Ok(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: st
                .lock()
                .unwrap()
                .packages
                .get(&key)
                .map_or(0, |p| p.attempts),
            state: PackageState::Published,
            started_at,
            finished_at,
            duration_ms,
            evidence: types::PackageEvidence {
                attempts: attempt_evidence,
                readiness_checks: readiness_evidence,
            },
        }),
    }
}

/// Helper function to update state while holding the lock
fn update_state_locked(st: &mut ExecutionState, key: &str, new_state: PackageState) {
    if let Some(pr) = st.packages.get_mut(key) {
        pr.state = new_state;
        pr.last_updated_at = Utc::now();
    }
    st.updated_at = Utc::now();
}

/// Publish packages in a single level in parallel
#[allow(clippy::too_many_arguments)]
fn run_publish_level(
    level: &types::PublishLevel,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<Mutex<dyn Reporter + Send>>,
) -> Result<Vec<PackageReceipt>> {
    let num_packages = level.packages.len();
    let max_concurrent = opts.parallel.max_concurrent.min(num_packages);

    reporter.lock().unwrap().info(&format!(
        "Level {}: publishing {} packages (max concurrent: {})",
        level.level, num_packages, max_concurrent
    ));

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Process packages in batches limited by max_concurrent
    for chunk in level.packages.chunks(max_concurrent) {
        let mut handles: Vec<std::thread::JoinHandle<PackagePublishResult>> = Vec::new();

        // Start all packages in this chunk
        for p in chunk {
            let p = p.clone();
            let ws_clone = ws.clone();
            let opts_clone = opts.clone();
            let reg_clone = reg.clone();
            let st_clone = Arc::clone(st);
            let state_dir = state_dir.to_path_buf();
            let event_log_clone = Arc::clone(event_log);
            let events_path = events_path.to_path_buf();
            let reporter_clone = Arc::clone(reporter);

            let handle = thread::spawn(move || {
                publish_package(
                    &p,
                    &ws_clone,
                    &opts_clone,
                    &reg_clone,
                    &st_clone,
                    &state_dir,
                    &event_log_clone,
                    &events_path,
                    &reporter_clone,
                )
            });

            handles.push(handle);
        }

        // Wait for all packages in this chunk to complete, collecting all results
        for handle in handles {
            let result = handle
                .join()
                .map_err(|_| anyhow::anyhow!("publish thread panicked"))?;
            match result.result {
                Ok(receipt) => all_receipts.push(receipt),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
    }

    if !errors.is_empty() {
        bail!(
            "parallel publish failed for {} package(s): {}",
            errors.len(),
            errors.join("; ")
        );
    }

    Ok(all_receipts)
}

/// Run publish in parallel mode, processing dependency levels sequentially
/// and packages within each level concurrently.
pub fn run_publish_parallel(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    reporter: &mut dyn Reporter,
) -> Result<Vec<PackageReceipt>> {
    let levels = ws.plan.group_by_levels();

    reporter.info(&format!(
        "parallel publish: {} levels, {} packages total",
        levels.len(),
        ws.plan.packages.len()
    ));

    // Initialize event log
    let events_path = events::events_path(state_dir);
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));

    // Wrap state and reporter in Arc<Mutex<>> for thread safety
    let st_arc = Arc::new(Mutex::new(st.clone()));

    // Create a thread-safe reporter wrapper
    struct SendReporter {
        infos: Mutex<Vec<String>>,
        warns: Mutex<Vec<String>>,
        errors: Mutex<Vec<String>>,
    }
    impl Reporter for SendReporter {
        fn info(&mut self, msg: &str) {
            self.infos.lock().unwrap().push(msg.to_string());
        }
        fn warn(&mut self, msg: &str) {
            self.warns.lock().unwrap().push(msg.to_string());
        }
        fn error(&mut self, msg: &str) {
            self.errors.lock().unwrap().push(msg.to_string());
        }
    }

    let send_reporter = Arc::new(Mutex::new(SendReporter {
        infos: Mutex::new(Vec::new()),
        warns: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
    }));

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();

    for level in &levels {
        let level_receipts = run_publish_level(
            level,
            ws,
            opts,
            reg,
            &st_arc,
            state_dir,
            &event_log,
            &events_path,
            &(send_reporter.clone() as Arc<Mutex<dyn Reporter + Send>>),
        )?;
        all_receipts.extend(level_receipts);
    }

    // Replay messages to the real reporter
    {
        let sr = send_reporter.lock().unwrap();
        for msg in sr.infos.lock().unwrap().iter() {
            reporter.info(msg);
        }
        for msg in sr.warns.lock().unwrap().iter() {
            reporter.warn(msg);
        }
        for msg in sr.errors.lock().unwrap().iter() {
            reporter.error(msg);
        }
    }

    // Copy updated state back
    let updated_st = st_arc.lock().unwrap();
    *st = updated_st.clone();

    Ok(all_receipts)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use chrono::Utc;
    use serial_test::serial;
    use tempfile::tempdir;
    use tiny_http::{Header, Response, Server, StatusCode};

    use super::*;
    use crate::plan::PlannedWorkspace;
    use crate::types::{
        PackageProgress, PlannedPackage, PublishLevel, ReadinessConfig, Registry, ReleasePlan,
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
            fs::set_permissions(path, perms).expect("chmod");
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
            policy: crate::types::PublishPolicy::default(),
            verify_mode: crate::types::VerifyMode::default(),
            readiness: ReadinessConfig {
                enabled: true,
                method: crate::types::ReadinessMethod::Api,
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
            parallel: crate::types::ParallelConfig {
                enabled: true,
                max_concurrent: 4,
                per_package_timeout: Duration::from_secs(60),
            },
        }
    }

    fn init_state_for_package(
        plan_id: &str,
        registry: &Registry,
        pkg_name: &str,
        pkg_version: &str,
    ) -> ExecutionState {
        let key = engine::pkg_key(pkg_name, pkg_version);
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
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
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
        let reg = RegistryClient::new(ws.plan.registry.clone()).expect("client");
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
        let reg = RegistryClient::new(ws.plan.registry.clone()).expect("client");
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
        let reg = RegistryClient::new(ws.plan.registry.clone()).expect("client");
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
        let reg = RegistryClient::new(ws.plan.registry.clone()).expect("client");
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

        let reg = RegistryClient::new(ws.plan.registry.clone()).expect("client");
        let opts = default_opts(PathBuf::from(".shipper"));
        let state_dir = td.path().join(".shipper");
        let mut packages = BTreeMap::new();
        for p in &ws.plan.packages {
            packages.insert(
                engine::pkg_key(&p.name, &p.version),
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
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
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
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
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
}
