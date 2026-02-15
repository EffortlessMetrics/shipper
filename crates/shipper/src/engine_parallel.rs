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

        let success = out.exit_code == 0;

        if success {
            {
                let mut rep = reporter.lock().unwrap();
                rep.info(&format!(
                    "{}@{}: cargo publish exited successfully; verifying...",
                    p.name, p.version
                ));
            }

            let verify_result =
                reg.is_version_visible_with_backoff(&p.name, &p.version, &opts.readiness);

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
                    }
                }
                Err(_) => {
                    last_err = Some((ErrorClass::Ambiguous, "readiness check failed".into()));
                }
            }
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

            let (class, msg) = engine::classify_cargo_failure(&out.stderr_tail, &out.stdout_tail);
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
