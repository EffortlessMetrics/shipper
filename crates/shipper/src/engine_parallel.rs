use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::auth;
use crate::cargo;
use crate::events;
use crate::git;
use crate::lock;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::state;
use crate::types;
use crate::types::{
    AuthType, ErrorClass, EventType, ExecutionState, PackageProgress, PackageReceipt, PackageState,
    PublishEvent, Receipt, RuntimeOptions, PublishLevel, PlannedPackage,
};

/// Type alias for tracking seen requests in tests
#[allow(dead_code)]
type SeenRequests = Arc<Mutex<Vec<(String, Option<String>)>>;

/// Type alias for tracking seen requests in tests
pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

/// Wrapper for Reporter trait to enable thread-safe access
struct ReporterWrapper<'a> {
    inner: &'a mut dyn Reporter,
}

impl<'a> Reporter for ReporterWrapper<'a> {
    fn info(&mut self, msg: &str) {
        self.inner.info(msg);
    }

    fn warn(&mut self, msg: &str) {
        self.inner.warn(msg);
    }

    fn error(&mut self, msg: &str) {
        self.inner.error(msg);
    }
}

/// Result of publishing a single package (for parallel execution)
#[derive(Debug)]
struct PackagePublishResult {
    package: PlannedPackage,
    result: anyhow::Result<PackageReceipt>,
}

/// Publish a single package with retries
fn publish_package(
    p: &PlannedPackage,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    event_log: &Arc<Mutex<events::EventLog>>,
    reporter: &Arc<Mutex<dyn Reporter + Send>>,
) -> PackagePublishResult {
    let key = pkg_key(&p.name, &p.version);
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
            package: format!("{}@{}", p.name, p.version),
        });
    }

    // Check if already published
    if let Ok(visible) = reg.version_exists(&p.name, &p.version) {
        if visible {
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
                update_state_locked(&mut state, &key, skipped);
            }

            return PackagePublishResult {
                package: p.clone(),
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
    }

    {
        let mut rep = reporter.lock().unwrap();
        rep.info(&format!("{}@{}: publishing...", p.name, p.version));
    }

    let mut attempt = 0;
    let mut last_err: Option<(ErrorClass, String)> = None;

    while attempt < opts.max_attempts {
        attempt += 1;
        {
            let mut state = st.lock().unwrap();
            let pr = state.packages.get_mut(&key).unwrap();
            pr.attempts = attempt;
            pr.last_updated_at = Utc::now();
            drop(state);
            state::save_state(&st.lock().unwrap()).ok();
        }

        {
            let mut rep = reporter.lock().unwrap();
            rep.info(&format!(
                "{}@{}: attempt {}/{}",
                p.name, p.version, attempt, opts.max_attempts
            ));
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
                // Cargo publish failed to execute
                {
                    let mut rep = reporter.lock().unwrap();
                    rep.error(&format!(
                        "{}@{}: cargo publish failed to execute: {}",
                        p.name, p.version, e
                    ));
                }
                return PackagePublishResult {
                    package: p.clone(),
                    result: Err(e),
                };
            }
        };

        // Record attempt event
        {
            let mut log = event_log.lock().unwrap();
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageAttempted {
                    attempt,
                    command: format!(
                        "cargo publish -p {} --registry {}",
                        p.name, ws.plan.registry.name
                    ),
                },
                package: format!("{}@{}", p.name, p.version),
            });

        // Record output event
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageOutput {
                stdout_tail: out.stdout_tail.clone(),
                stderr_tail: out.stderr_tail.clone(),
            },
            package: format!("{}@{}", p.name, p.version),
        });

        let success = out.exit_code == 0;

        if success {
            {
                let mut rep = reporter.lock().unwrap();
                rep.info(&format!(
                    "{}@{}: cargo publish exited successfully; verifying...",
                    p.name, p.version
                ));
            }

            let visible = verify_published(
                reg,
                &p.name,
                &p.version,
                &opts.readiness,
                &mut *reporter.lock().unwrap(),
                &mut *event_log.lock().unwrap(),
            );

            if visible {
                {
                    let mut state = st.lock().unwrap();
                    update_state_locked(&mut state, &key, PackageState::Published);
                }
                last_err = None;

                // Record package published event
                {
                    let mut log = event_log.lock().unwrap();
                    log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackagePublished {
                            duration_ms: out.duration.as_millis() as u64,
                        },
                        package: format!("{}@{}", p.name, p.version),
                    });
                }

                break;
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
                    }
                    last_err = None;
                    break;
                }

                let (class, msg) = classify_cargo_failure(&out.stderr_tail, &out.stdout_tail);
                last_err = Some((class.clone(), msg.clone()));

                // Record package failed event
                {
                    let mut log = event_log.lock().unwrap();
                    log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackageFailed {
                            class: class.clone(),
                            message: msg.clone(),
                        },
                        package: format!("{}@{}", p.name, p.version),
                    });
                }

                match class {
                    ErrorClass::Permanent => {
                        let failed = PackageState::Failed {
                            class,
                            message: msg,
                        };
                        {
                            let mut state = st.lock().unwrap();
                            update_state_locked(&mut state, &key, failed);
                        }

                        return PackagePublishResult {
                            package: p.clone(),
                            result: Err(anyhow::anyhow!(
                                "{}@{}: permanent failure: {}",
                                p.name,
                                p.version,
                                last_err.unwrap().1
                            )),
                        };
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        let delay = backoff_delay(opts.base_delay, opts.max_delay, attempt);
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

        // If we got here without breaking, and there are attempts left, loop.
    }

    let finished_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis();

    if let Some((class, msg)) = last_err {
        // Final chance: maybe it eventually showed up.
        if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
            {
                let mut state = st.lock().unwrap();
                update_state_locked(&mut state, &key, PackageState::Published);
            }

            return PackagePublishResult {
                package: p.clone(),
                result: Ok(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: st.lock().unwrap().packages.get(&key).unwrap().attempts,
                    state: PackageState::Published,
                    started_at,
                    finished_at,
                    duration_ms,
                    evidence: types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                }),
            };
        } else {
            let failed = PackageState::Failed {
                class: class.clone(),
                message: msg.clone(),
            };
            {
                let mut state = st.lock().unwrap();
                update_state_locked(&mut state, &key, failed);
            }

            return PackagePublishResult {
                package: p.clone(),
                result: Err(anyhow::anyhow!("{}@{}: failed: {}", p.name, p.version, msg)),
            };
        }
    }

    PackagePublishResult {
        package: p.clone(),
        result: Ok(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: st.lock().unwrap().packages.get(&key).unwrap().attempts,
            state: PackageState::Published,
            started_at,
            finished_at,
            duration_ms,
            evidence: types::PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        }),
    }
}

/// Helper function to update state while holding the lock
fn update_state_locked(
    st: &mut ExecutionState,
    key: &str,
    new_state: PackageState,
) {
    let pr = st.packages.get_mut(key).expect("missing package in state");
    pr.state = new_state;
    pr.last_updated_at = Utc::now();
    st.updated_at = Utc::now();
}

/// Publish packages in a single level in parallel
fn run_publish_level(
    level: &PublishLevel,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    event_log: &Arc<Mutex<events::EventLog>>,
    reporter: &Arc<Mutex<dyn Reporter + Send>>,
) -> Result<Vec<PackageReceipt>> {
    let num_packages = level.packages.len();
    let max_concurrent = opts.parallel.max_concurrent.min(num_packages);

    reporter.lock().unwrap().info(&format!(
        "Level {}: publishing {} packages (max concurrent: {})",
        level.level,
        num_packages,
        max_concurrent
    ));

    let mut receipts: Vec<PackageReceipt> = Vec::new();
    let mut handles: Vec<std::thread::JoinHandle<PackagePublishResult>> = Vec::new();

    // Process packages in batches limited by max_concurrent
    for chunk in level.packages.chunks(max_concurrent) {
        // Start all packages in this chunk
        for p in chunk {
            let p = p.clone();
            let ws_clone = ws.clone();
            let opts_clone = opts.clone();
            let reg_clone = reg.clone();
            let st_clone = Arc::clone(st);
            let event_log_clone = Arc::clone(event_log);
            let reporter_clone = Arc::clone(reporter);

            let handle = thread::spawn(move || {
                publish_package(
                    &p,
                    &ws_clone,
                    &opts_clone,
                    &reg_clone,
                    &st_clone,
                    &event_log_clone,
                    &reporter_clone,
                )
            });

            handles.push(handle);
        }

        // Wait for all packages in this chunk to complete
        for handle in handles.drain(..) {
            let result = handle.join().expect("thread panicked");
            match result.result {
                Ok(receipt) => receipts.push(receipt),
                Err(e) => {
                    // For parallel execution, we collect errors and report at the end
                    // If one package fails, we still continue with others in the same level
                    // but overall publish will fail
                    return Err(e);
                }
            }
        }
    }

    Ok(receipts)
}
