use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::auth;
use crate::cargo;
use crate::environment;
use crate::events;
use crate::git;
use crate::lock;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::state;
use crate::types::{
    AttemptEvidence, AuthType, ErrorClass, EventType, ExecutionResult, ExecutionState,
    Finishability, PackageProgress, PackageReceipt, PackageState, PreflightPackage,
    PreflightReport, PublishEvent, PublishPolicy, ReadinessEvidence, Receipt, RuntimeOptions,
};

pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

struct PolicyEffects {
    run_dry_run: bool,
    check_ownership: bool,
    strict_ownership: bool,
    readiness_enabled: bool,
}

fn apply_policy(opts: &RuntimeOptions) -> PolicyEffects {
    match opts.policy {
        PublishPolicy::Safe => PolicyEffects {
            run_dry_run: !opts.no_verify,
            check_ownership: !opts.skip_ownership_check,
            strict_ownership: opts.strict_ownership,
            readiness_enabled: opts.readiness.enabled,
        },
        PublishPolicy::Balanced => PolicyEffects {
            run_dry_run: !opts.no_verify,
            check_ownership: false,
            strict_ownership: false,
            readiness_enabled: opts.readiness.enabled,
        },
        PublishPolicy::Fast => PolicyEffects {
            run_dry_run: false,
            check_ownership: false,
            strict_ownership: false,
            readiness_enabled: false,
        },
    }
}

pub fn run_preflight(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PreflightReport> {
    let workspace_root = &ws.workspace_root;
    let effects = apply_policy(opts);

    if !opts.allow_dirty {
        reporter.info("checking git cleanliness...");
        git::ensure_git_clean(workspace_root)?;
    }

    reporter.info("initializing registry client...");
    let reg = RegistryClient::new(ws.plan.registry.clone())?;

    let token = auth::resolve_token(&ws.plan.registry.name)?;
    let token_detected = token.as_ref().map(|s| !s.is_empty()).unwrap_or(false);

    if effects.strict_ownership && !token_detected {
        bail!(
            "strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)"
        );
    }

    // Determine auth type
    let auth_type = if token_detected {
        Some(AuthType::Token)
    } else {
        None
    };

    // Run dry-run verification based on VerifyMode and policy
    use crate::types::VerifyMode;

    // Workspace-level dry-run result (used for Workspace mode)
    let workspace_dry_run_passed =
        if effects.run_dry_run && opts.verify_mode == VerifyMode::Workspace {
            reporter.info("running workspace dry-run verification...");
            let dry_run_result = cargo::cargo_publish_dry_run_workspace(
                workspace_root,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.output_lines,
            );
            match &dry_run_result {
                Ok(output) => output.exit_code == 0,
                Err(_) => false,
            }
        } else if !effects.run_dry_run || opts.verify_mode == VerifyMode::None {
            reporter.info("skipping dry-run (policy, --no-verify, or verify_mode=none)");
            true
        } else {
            // Package mode — handled per-package below
            true
        };

    // Per-package dry-run results (used for Package mode)
    let per_package_dry_run: std::collections::BTreeMap<String, bool> =
        if effects.run_dry_run && opts.verify_mode == VerifyMode::Package {
            reporter.info("running per-package dry-run verification...");
            let mut results = std::collections::BTreeMap::new();
            for p in &ws.plan.packages {
                let result = cargo::cargo_publish_dry_run_package(
                    workspace_root,
                    &p.name,
                    &ws.plan.registry.name,
                    opts.allow_dirty,
                    opts.output_lines,
                );
                let passed = match &result {
                    Ok(output) => output.exit_code == 0,
                    Err(_) => false,
                };
                if !passed {
                    reporter.warn(&format!("{}@{}: dry-run failed", p.name, p.version));
                }
                results.insert(p.name.clone(), passed);
            }
            results
        } else {
            std::collections::BTreeMap::new()
        };

    // Check each package
    reporter.info("checking packages against registry...");
    let mut packages: Vec<PreflightPackage> = Vec::new();
    let mut any_ownership_unverified = false;

    for p in &ws.plan.packages {
        let already_published = reg.version_exists(&p.name, &p.version)?;
        let is_new_crate = reg.check_new_crate(&p.name)?;

        // Determine dry-run result for this package
        let dry_run_passed = if opts.verify_mode == VerifyMode::Package {
            *per_package_dry_run.get(&p.name).unwrap_or(&true)
        } else {
            workspace_dry_run_passed
        };

        // Ownership verification (best-effort), gated by policy
        let ownership_verified = if token_detected && effects.check_ownership {
            if effects.strict_ownership {
                // In strict mode, ownership errors are fatal
                reg.list_owners(&p.name, token.as_deref().unwrap())?;
                true
            } else {
                let result = reg
                    .verify_ownership(&p.name, token.as_deref().unwrap())
                    .unwrap_or_default();
                if !result {
                    reporter.warn(&format!(
                        "owners preflight failed for {}; continuing (non-strict mode)",
                        p.name
                    ));
                }
                result
            }
        } else {
            // No token, ownership check skipped, or policy disabled it
            false
        };

        if !ownership_verified {
            any_ownership_unverified = true;
        }

        packages.push(PreflightPackage {
            name: p.name.clone(),
            version: p.version.clone(),
            already_published,
            is_new_crate,
            auth_type: auth_type.clone(),
            ownership_verified,
            dry_run_passed,
        });
    }

    // For finishability: all packages must pass dry-run
    let all_dry_run_passed = packages.iter().all(|p| p.dry_run_passed);

    // Determine finishability
    let finishability = if !all_dry_run_passed {
        Finishability::Failed
    } else if any_ownership_unverified {
        Finishability::NotProven
    } else {
        Finishability::Proven
    };

    Ok(PreflightReport {
        plan_id: ws.plan.plan_id.clone(),
        token_detected,
        finishability,
        packages,
        timestamp: Utc::now(),
    })
}

pub fn run_publish(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);
    let effects = apply_policy(opts);

    // Acquire lock
    let lock_timeout = if opts.force {
        Duration::ZERO
    } else {
        opts.lock_timeout
    };
    let _lock = lock::LockFile::acquire_with_timeout(&state_dir, lock_timeout)
        .context("failed to acquire publish lock")?;
    _lock.set_plan_id(&ws.plan.plan_id)?;

    // Collect git context and environment fingerprint at start of execution
    let git_context = git::collect_git_context();
    let environment = environment::collect_environment_fingerprint();

    if !opts.allow_dirty {
        git::ensure_git_clean(workspace_root)?;
    }

    let reg = RegistryClient::new(ws.plan.registry.clone())?;

    // Initialize event log
    let events_path = events::events_path(&state_dir);
    let mut event_log = events::EventLog::new();

    // Load existing state (if any), or initialize.
    let mut st = match state::load_state(&state_dir)? {
        Some(existing) => {
            if existing.plan_id != ws.plan.plan_id {
                if !opts.force_resume {
                    bail!(
                        "existing state plan_id {} does not match current plan_id {}; delete state or use --force-resume",
                        existing.plan_id,
                        ws.plan.plan_id
                    );
                }
                reporter.warn("forcing resume with mismatched plan_id (unsafe)");
            }
            existing
        }
        None => init_state(ws, &state_dir)?,
    };

    reporter.info(&format!("state dir: {}", state_dir.as_path().display()));

    let mut receipts: Vec<PackageReceipt> = Vec::new();
    let run_started = Utc::now();

    // Event: ExecutionStarted
    event_log.record(PublishEvent {
        timestamp: run_started,
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });
    // Event: PlanCreated
    event_log.record(PublishEvent {
        timestamp: run_started,
        event_type: EventType::PlanCreated {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;
    event_log.clear();

    // Ensure we have entries for all packages in plan.
    for p in &ws.plan.packages {
        let key = pkg_key(&p.name, &p.version);
        st.packages.entry(key).or_insert_with(|| PackageProgress {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        });
    }
    st.updated_at = Utc::now();
    state::save_state(&state_dir, &st)?;

    // Check for parallel mode
    if opts.parallel.enabled {
        let parallel_receipts = crate::engine_parallel::run_publish_parallel(
            ws, opts, &mut st, &state_dir, &reg, reporter,
        )?;

        // Event: ExecutionFinished
        let exec_result = if parallel_receipts.iter().all(|r| {
            matches!(
                r.state,
                PackageState::Published | PackageState::Skipped { .. }
            )
        }) {
            ExecutionResult::Success
        } else {
            ExecutionResult::PartialFailure
        };
        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ExecutionFinished {
                result: exec_result,
            },
            package: "all".to_string(),
        });
        event_log.write_to_file(&events_path)?;

        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            started_at: run_started,
            finished_at: Utc::now(),
            packages: parallel_receipts,
            event_log_path: state_dir.join("events.jsonl"),
            git_context,
            environment,
        };

        state::write_receipt(&state_dir, &receipt)?;
        return Ok(receipt);
    }

    for p in &ws.plan.packages {
        let key = pkg_key(&p.name, &p.version);
        let pkg_label = format!("{}@{}", p.name, p.version);
        let progress = st
            .packages
            .get(&key)
            .context("missing package progress in state")?
            .clone();

        match progress.state {
            PackageState::Published | PackageState::Skipped { .. } => {
                reporter.info(&format!(
                    "{}@{}: already complete ({})",
                    p.name,
                    p.version,
                    short_state(&progress.state)
                ));
                continue;
            }
            _ => {}
        }

        // Event: PackageStarted
        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: p.name.clone(),
                version: p.version.clone(),
            },
            package: pkg_label.clone(),
        });

        let started_at = Utc::now();
        let start_instant = Instant::now();

        // First, check if the version is already present.
        if reg.version_exists(&p.name, &p.version)? {
            reporter.info(&format!(
                "{}@{}: already published (skipping)",
                p.name, p.version
            ));
            let skipped = PackageState::Skipped {
                reason: "already published".into(),
            };
            update_state(&mut st, &state_dir, &key, skipped)?;

            // Event: PackageSkipped
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageSkipped {
                    reason: "already published".to_string(),
                },
                package: pkg_label.clone(),
            });
            event_log.write_to_file(&events_path)?;
            event_log.clear();

            receipts.push(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: st.packages.get(&key).unwrap().attempts,
                state: st.packages.get(&key).unwrap().state.clone(),
                started_at,
                finished_at: Utc::now(),
                duration_ms: start_instant.elapsed().as_millis(),
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            });
            continue;
        }

        reporter.info(&format!("{}@{}: publishing...", p.name, p.version));

        let mut attempt = st.packages.get(&key).unwrap().attempts;
        let mut last_err: Option<(ErrorClass, String)> = None;
        let mut attempt_evidence: Vec<AttemptEvidence> = Vec::new();
        let mut readiness_evidence: Vec<ReadinessEvidence> = Vec::new();

        while attempt < opts.max_attempts {
            attempt += 1;
            {
                let pr = st.packages.get_mut(&key).unwrap();
                pr.attempts = attempt;
                pr.last_updated_at = Utc::now();
                state::save_state(&state_dir, &st)?;
            }

            let command = format!(
                "cargo publish -p {} --registry {}",
                p.name, ws.plan.registry.name
            );

            reporter.info(&format!(
                "{}@{}: attempt {}/{}",
                p.name, p.version, attempt, opts.max_attempts
            ));

            // Event: PackageAttempted
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageAttempted {
                    attempt,
                    command: command.clone(),
                },
                package: pkg_label.clone(),
            });

            let out = cargo::cargo_publish(
                workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
                opts.output_lines,
            )?;

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
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageOutput {
                    stdout_tail: out.stdout_tail.clone(),
                    stderr_tail: out.stderr_tail.clone(),
                },
                package: pkg_label.clone(),
            });

            let success = out.exit_code == 0;

            if success {
                reporter.info(&format!(
                    "{}@{}: cargo publish exited successfully; verifying...",
                    p.name, p.version
                ));
                let readiness_config = crate::types::ReadinessConfig {
                    enabled: effects.readiness_enabled,
                    ..opts.readiness.clone()
                };
                let (visible, checks) =
                    verify_published(&reg, &p.name, &p.version, &readiness_config, reporter)?;
                readiness_evidence = checks;
                if visible {
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                    last_err = None;

                    // Event: PackagePublished
                    event_log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackagePublished {
                            duration_ms: start_instant.elapsed().as_millis() as u64,
                        },
                        package: pkg_label.clone(),
                    });
                    event_log.write_to_file(&events_path)?;
                    event_log.clear();

                    break;
                } else {
                    // Cargo itself may warn if the index isn't updated yet. Shipper extends the wait,
                    // but if it still doesn't show up we treat this as ambiguous.
                    last_err = Some((ErrorClass::Ambiguous, "publish succeeded locally, but version not observed on registry within timeout".into()));
                }
            } else {
                // Even if cargo fails, the publish may have succeeded (timeouts, network splits).
                // Always check the registry before deciding.
                reporter.warn(&format!(
                    "{}@{}: cargo publish failed (exit={:?}); checking registry...",
                    p.name, p.version, out.exit_code
                ));

                if reg.version_exists(&p.name, &p.version)? {
                    reporter.info(&format!(
                        "{}@{}: version is present on registry; treating as published",
                        p.name, p.version
                    ));
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                    last_err = None;
                    break;
                }

                let (class, msg) = classify_cargo_failure(&out.stderr_tail, &out.stdout_tail);
                last_err = Some((class.clone(), msg.clone()));

                match class {
                    ErrorClass::Permanent => {
                        let failed = PackageState::Failed {
                            class: class.clone(),
                            message: msg.clone(),
                        };
                        update_state(&mut st, &state_dir, &key, failed)?;

                        // Event: PackageFailed
                        event_log.record(PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackageFailed {
                                class,
                                message: msg,
                            },
                            package: pkg_label.clone(),
                        });
                        event_log.write_to_file(&events_path)?;
                        event_log.clear();

                        return Err(anyhow::anyhow!(
                            "{}@{}: permanent failure: {}",
                            p.name,
                            p.version,
                            last_err.unwrap().1
                        ));
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        let delay = backoff_delay(opts.base_delay, opts.max_delay, attempt);
                        reporter.warn(&format!(
                            "{}@{}: retrying in {}",
                            p.name,
                            p.version,
                            humantime::format_duration(delay)
                        ));
                        thread::sleep(delay);
                    }
                }
            }

            // If we got here without breaking, and there are attempts left, loop.
        }

        let finished_at = Utc::now();
        let duration_ms = start_instant.elapsed().as_millis();

        if let Some((class, msg)) = last_err {
            // Final chance: maybe it eventually showed up.
            if reg.version_exists(&p.name, &p.version)? {
                update_state(&mut st, &state_dir, &key, PackageState::Published)?;
            } else {
                let failed = PackageState::Failed {
                    class: class.clone(),
                    message: msg.clone(),
                };
                update_state(&mut st, &state_dir, &key, failed)?;

                // Event: PackageFailed
                event_log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class,
                        message: msg.clone(),
                    },
                    package: pkg_label.clone(),
                });
                event_log.write_to_file(&events_path)?;
                event_log.clear();

                receipts.push(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: st.packages.get(&key).unwrap().attempts,
                    state: st.packages.get(&key).unwrap().state.clone(),
                    started_at,
                    finished_at,
                    duration_ms,
                    evidence: crate::types::PackageEvidence {
                        attempts: attempt_evidence,
                        readiness_checks: readiness_evidence,
                    },
                });
                return Err(anyhow::anyhow!("{}@{}: failed: {}", p.name, p.version, msg));
            }
        }

        receipts.push(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: st.packages.get(&key).unwrap().attempts,
            state: st.packages.get(&key).unwrap().state.clone(),
            started_at,
            finished_at,
            duration_ms,
            evidence: crate::types::PackageEvidence {
                attempts: attempt_evidence,
                readiness_checks: readiness_evidence,
            },
        });
    }

    // Event: ExecutionFinished
    let exec_result = if receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Skipped { .. }
        )
    }) {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    };
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result,
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;

    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        started_at: run_started,
        finished_at: Utc::now(),
        packages: receipts,
        event_log_path: state_dir.join("events.jsonl"),
        git_context,
        environment,
    };

    state::write_receipt(&state_dir, &receipt)?;

    Ok(receipt)
}

pub fn run_resume(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);
    if state::load_state(&state_dir)?.is_none() {
        bail!(
            "no existing state found in {}; run shipper publish first",
            state_dir.display()
        );
    }
    run_publish(ws, opts, reporter)
}

pub(crate) fn init_state(ws: &PlannedWorkspace, state_dir: &Path) -> Result<ExecutionState> {
    let mut packages: BTreeMap<String, PackageProgress> = BTreeMap::new();
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

    let st = ExecutionState {
        state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };

    state::save_state(state_dir, &st)?;
    Ok(st)
}

fn update_state(
    st: &mut ExecutionState,
    state_dir: &Path,
    key: &str,
    new_state: PackageState,
) -> Result<()> {
    let pr = st
        .packages
        .get_mut(key)
        .context("missing package in state")?;
    pr.state = new_state;
    pr.last_updated_at = Utc::now();
    st.updated_at = Utc::now();
    state::save_state(state_dir, st)
}

pub(crate) fn resolve_state_dir(workspace_root: &Path, state_dir: &PathBuf) -> PathBuf {
    if state_dir.is_absolute() {
        state_dir.clone()
    } else {
        workspace_root.join(state_dir)
    }
}

pub(crate) fn pkg_key(name: &str, version: &str) -> String {
    format!("{}@{}", name, version)
}

pub(crate) fn short_state(st: &PackageState) -> &'static str {
    match st {
        PackageState::Pending => "pending",
        PackageState::Published => "published",
        PackageState::Skipped { .. } => "skipped",
        PackageState::Failed { .. } => "failed",
        PackageState::Ambiguous { .. } => "ambiguous",
    }
}

fn verify_published(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &crate::types::ReadinessConfig,
    reporter: &mut dyn Reporter,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    reporter.info(&format!(
        "{}@{}: readiness check ({:?})...",
        crate_name, version, config.method
    ));
    let (visible, evidence) = reg.is_version_visible_with_backoff(crate_name, version, config)?;
    if visible {
        reporter.info(&format!(
            "{}@{}: visible after {} checks",
            crate_name,
            version,
            evidence.len()
        ));
    } else {
        reporter.warn(&format!(
            "{}@{}: not visible after {} checks",
            crate_name,
            version,
            evidence.len()
        ));
    }
    Ok((visible, evidence))
}

pub(crate) fn classify_cargo_failure(stderr: &str, stdout: &str) -> (ErrorClass, String) {
    let hay = format!("{}\n{}", stderr, stdout).to_lowercase();

    // Retryable: backpressure and transient network failures.
    let retryable_patterns = [
        "too many requests",
        "429",
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "connection closed",
        "dns",
        "tls",
        "temporarily unavailable",
        "failed to download",
        "failed to send",
        "server error",
        "502",
        "503",
        "504",
    ];

    if retryable_patterns.iter().any(|p| hay.contains(p)) {
        return (
            ErrorClass::Retryable,
            "transient failure (retryable)".into(),
        );
    }

    // Permanent: manifest / packaging / validation failures.
    let permanent_patterns = [
        "failed to parse manifest",
        "invalid",
        "missing",
        "license",
        "description",
        "readme",
        "repository",
        "could not compile",
        "compilation failed",
        "failed to verify",
        "package is not allowed to be published",
        "publish is disabled",
        "yanked",
        "forbidden",
        "permission denied",
        "not authorized",
        "unauthorized",
    ];

    if permanent_patterns.iter().any(|p| hay.contains(p)) {
        return (
            ErrorClass::Permanent,
            "permanent failure (fix required)".into(),
        );
    }

    // Ambiguous: default. We'll always verify registry before failing.
    (
        ErrorClass::Ambiguous,
        "publish outcome ambiguous; registry did not show version".into(),
    )
}

pub(crate) fn backoff_delay(base: Duration, max: Duration, attempt: u32) -> Duration {
    let pow = attempt.saturating_sub(1).min(16);
    let mut delay = base.saturating_mul(2_u32.saturating_pow(pow));
    if delay > max {
        delay = max;
    }

    // 0.5x..1.5x jitter
    let jitter: f64 = rand::random::<f64>(); // Random value between 0 and 1
    let jitter = 0.5 + jitter; // Scale to 0.5..1.5
    let millis = (delay.as_millis() as f64 * jitter).round() as u128;
    Duration::from_millis(millis as u64)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use chrono::Utc;
    use serial_test::serial;
    use tempfile::tempdir;
    use tiny_http::{Header, Response, Server, StatusCode};

    use super::*;
    use crate::plan::PlannedWorkspace;
    use crate::types::{PlannedPackage, Registry, ReleasePlan};

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

    #[derive(Clone)]
    struct EnvGuard {
        key: String,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            Self {
                key: key.to_string(),
                old,
            }
        }

        fn unset(key: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::remove_var(key) };
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                unsafe { env::set_var(&self.key, v) };
            } else {
                unsafe { env::remove_var(&self.key) };
            }
        }
    }

    #[cfg(windows)]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo.cmd")
    }

    #[cfg(not(windows))]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo")
    }

    #[cfg(windows)]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git.cmd")
    }

    #[cfg(not(windows))]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git")
    }

    fn configure_fake_programs(bin_dir: &Path) -> (EnvGuard, EnvGuard) {
        let cargo = EnvGuard::set(
            "SHIPPER_CARGO_BIN",
            fake_cargo_path(bin_dir).to_str().expect("utf8"),
        );
        let git = EnvGuard::set(
            "SHIPPER_GIT_BIN",
            fake_git_path(bin_dir).to_str().expect("utf8"),
        );
        (cargo, git)
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

    fn write_fake_git(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("git.cmd"),
                "@echo off\r\nif \"%SHIPPER_GIT_FAIL%\"==\"1\" (\r\n  echo fatal: git failed 1>&2\r\n  exit /b 1\r\n)\r\nif \"%SHIPPER_GIT_CLEAN%\"==\"0\" (\r\n  echo M src/lib.rs\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"${SHIPPER_GIT_FAIL:-0}\" = \"1\" ]; then\n  echo 'fatal: git failed' >&2\n  exit 1\nfi\nif [ \"${SHIPPER_GIT_CLEAN:-1}\" = \"0\" ]; then\n  echo 'M src/lib.rs'\nfi\nexit 0\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod");
        }
    }

    fn write_fake_tools(bin_dir: &Path) {
        fs::create_dir_all(bin_dir).expect("mkdir");
        write_fake_cargo(bin_dir);
        write_fake_git(bin_dir);
    }

    struct TestRegistryServer {
        base_url: String,
        #[allow(clippy::type_complexity)]
        seen: Arc<Mutex<Vec<(String, Option<String>)>>>,
        handle: thread::JoinHandle<()>,
    }

    impl TestRegistryServer {
        fn join(self) {
            self.handle.join().expect("join server");
        }
    }

    fn spawn_registry_server(
        mut routes: std::collections::BTreeMap<String, Vec<(u16, String)>>,
        expected_requests: usize,
    ) -> TestRegistryServer {
        let server = Server::http("127.0.0.1:0").expect("server");
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
        let seen_thread = Arc::clone(&seen);

        let handle = thread::spawn(move || {
            for _ in 0..expected_requests {
                let req = server.recv().expect("request");
                let path = req.url().to_string();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                seen_thread.lock().expect("lock").push((path.clone(), auth));

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

        TestRegistryServer {
            base_url,
            seen,
            handle,
        }
    }

    fn planned_workspace(workspace_root: &Path, api_base: String) -> PlannedWorkspace {
        PlannedWorkspace {
            workspace_root: workspace_root.to_path_buf(),
            plan: ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "plan-demo".to_string(),
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
                dependencies: std::collections::BTreeMap::new(),
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
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            verify_timeout: Duration::from_millis(20),
            verify_poll_interval: Duration::from_millis(1),
            state_dir,
            force_resume: false,
            policy: crate::types::PublishPolicy::default(),
            verify_mode: crate::types::VerifyMode::default(),
            readiness: crate::types::ReadinessConfig {
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
            parallel: crate::types::ParallelConfig::default(),
        }
    }

    #[test]
    fn classify_cargo_failure_covers_retryable_permanent_and_ambiguous() {
        let retryable = classify_cargo_failure("HTTP 429 too many requests", "");
        assert_eq!(retryable.0, ErrorClass::Retryable);

        let permanent = classify_cargo_failure("permission denied", "");
        assert_eq!(permanent.0, ErrorClass::Permanent);

        let ambiguous = classify_cargo_failure("strange output", "");
        assert_eq!(ambiguous.0, ErrorClass::Ambiguous);
    }

    #[test]
    fn collecting_reporter_error_method_records_message() {
        let mut reporter = CollectingReporter::default();
        reporter.error("boom");
        assert_eq!(reporter.errors, vec!["boom".to_string()]);
    }

    #[test]
    fn helper_functions_return_expected_values() {
        let root = PathBuf::from("root");
        let rel = resolve_state_dir(&root, &PathBuf::from(".shipper"));
        assert_eq!(rel, root.join(".shipper"));

        #[cfg(windows)]
        {
            let abs = PathBuf::from(r"C:\x\state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }
        #[cfg(not(windows))]
        {
            let abs = PathBuf::from("/x/state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }

        assert_eq!(pkg_key("a", "1.2.3"), "a@1.2.3");
        assert_eq!(short_state(&PackageState::Pending), "pending");
        assert_eq!(short_state(&PackageState::Published), "published");
        assert_eq!(
            short_state(&PackageState::Skipped {
                reason: "r".to_string()
            }),
            "skipped"
        );
        assert_eq!(
            short_state(&PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "m".to_string()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&PackageState::Ambiguous {
                message: "m".to_string()
            }),
            "ambiguous"
        );
    }

    #[test]
    fn backoff_delay_is_bounded_with_jitter() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);
        let d1 = backoff_delay(base, max, 1);
        let d20 = backoff_delay(base, max, 20);

        assert!(d1 >= Duration::from_millis(50));
        assert!(d1 <= Duration::from_millis(150));

        assert!(d20 >= Duration::from_millis(250));
        assert!(d20 <= Duration::from_millis(750));
    }

    #[test]
    fn verify_published_returns_true_when_registry_visibility_appears() {
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );

        let reg = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: true,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_millis(500),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let (ok, evidence) =
            verify_published(&reg, "demo", "0.1.0", &config, &mut reporter).expect("verify");
        assert!(ok);
        assert!(!reporter.infos.is_empty());
        assert!(!evidence.is_empty());
        server.join();
    }

    #[test]
    fn verify_published_returns_false_on_timeout() {
        let reg = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: "http://127.0.0.1:9".to_string(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: true,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(10),
            max_total_wait: Duration::from_millis(0),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let (ok, _evidence) =
            verify_published(&reg, "demo", "0.1.0", &config, &mut reporter).expect("verify");
        assert!(!ok);
    }

    #[test]
    fn registry_server_helper_returns_404_for_unknown_or_empty_routes() {
        let server_unknown = spawn_registry_server(std::collections::BTreeMap::new(), 1);
        let reg_unknown = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server_unknown.base_url.clone(),
            index_base: None,
        })
        .expect("client");
        let exists_unknown = reg_unknown
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_unknown);
        server_unknown.join();

        let server_empty = spawn_registry_server(
            std::collections::BTreeMap::from([("/api/v1/crates/demo/0.1.0".to_string(), vec![])]),
            1,
        );
        let reg_empty = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server_empty.base_url.clone(),
            index_base: None,
        })
        .expect("client");
        let exists_empty = reg_empty
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_empty);
        server_empty.join();
    }

    #[test]
    #[serial]
    fn run_preflight_errors_in_strict_mode_without_token() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.strict_ownership = true;
        opts.skip_ownership_check = false;

        let mut reporter = CollectingReporter::default();
        let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("strict ownership requested but no token found"));
    }

    #[test]
    #[serial]
    fn run_preflight_warns_on_owners_failure_when_not_strict() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(403, "{}".to_string())],
                ),
            ]),
            3,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = false;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert!(rep.token_detected);
        assert_eq!(rep.packages.len(), 1);
        assert!(!rep.packages[0].already_published);
        assert!(!rep.packages[0].ownership_verified);
        assert!(rep.packages[0].dry_run_passed);
        assert_eq!(rep.finishability, Finishability::NotProven);
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("owners preflight failed"))
        );

        let seen = server.seen.lock().expect("lock");
        assert_eq!(seen.len(), 3);
        drop(seen);
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_owners_success_path() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(200, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(
                        200,
                        r#"{"users":[{"id":1,"login":"alice","name":"Alice"}]}"#.to_string(),
                    )],
                ),
            ]),
            3,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = false;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert_eq!(rep.packages.len(), 1);
        assert!(reporter.warns.is_empty());
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_returns_error_when_strict_ownership_check_fails() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(403, "{}".to_string())],
                ),
            ]),
            3,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = true;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("forbidden when querying owners"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _git_clean = EnvGuard::set("SHIPPER_GIT_CLEAN", "1");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = false;
        opts.skip_ownership_check = true;

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert_eq!(rep.packages.len(), 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_skips_when_version_already_exists() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(
            receipt.packages[0].state,
            PackageState::Skipped { .. }
        ));

        let state_dir = td.path().join(".shipper");
        assert!(state::state_path(&state_dir).exists());
        assert!(state::receipt_path(&state_dir).exists());
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _git_clean = EnvGuard::set("SHIPPER_GIT_CLEAN", "1");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = false;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_adds_missing_package_entries_to_existing_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let state_dir = td.path().join(".shipper");
        let existing = ExecutionState {
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        state::save_state(&state_dir, &existing).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

        let st = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert!(st.packages.contains_key("demo@0.1.0"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_marks_published_after_successful_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.verify_timeout = Duration::from_millis(200);
        opts.verify_poll_interval = Duration::from_millis(1);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_treats_500_as_not_visible_during_readiness() {
        // With the readiness-driven verify, 500 errors are treated as "not visible"
        // (graceful degradation). The publish succeeds via cargo but readiness times out,
        // leading to an ambiguous failure on the final registry check.
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![
                    (404, "{}".to_string()),
                    (500, "{}".to_string()),
                    (404, "{}".to_string()),
                ],
            )]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.readiness.max_total_wait = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_treats_failed_cargo_as_published_if_registry_shows_version() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "timeout while uploading");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert_eq!(receipt.packages[0].attempts, 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_retries_on_retryable_failures() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "timeout talking to server");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
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
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 2;
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert_eq!(receipt.packages[0].attempts, 2);
        assert!(reporter.warns.iter().any(|w| w.contains("retrying in")));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_errors_when_cargo_command_cannot_start() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let missing = td.path().join("no-cargo-here");
        let _cargo_bin = EnvGuard::set("SHIPPER_CARGO_BIN", missing.to_str().expect("utf8"));

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to execute cargo publish"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_returns_error_on_permanent_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "permission denied");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("permanent failure"));

        let st = state::load_state(&td.path().join(".shipper"))
            .expect("load")
            .expect("exists");
        let pkg = st.packages.get("demo@0.1.0").expect("pkg");
        assert!(matches!(
            pkg.state,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                ..
            }
        ));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_marks_ambiguous_failure_after_success_without_registry_visibility() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        // 3 requests: initial version_exists, readiness check, final chance check
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            )]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.readiness.max_total_wait = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed"));

        let st = state::load_state(&td.path().join(".shipper"))
            .expect("load")
            .expect("exists");
        let pkg = st.packages.get("demo@0.1.0").expect("pkg");
        assert!(matches!(
            pkg.state,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                ..
            }
        ));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_recovers_on_final_registry_check_after_ambiguous_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.readiness.max_total_wait = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        server.join();
    }

    #[test]
    fn run_publish_errors_on_plan_mismatch_without_force_resume() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("does not match current plan_id"));
    }

    #[test]
    fn run_publish_allows_forced_resume_with_plan_mismatch() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.force_resume = true;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(receipt.packages.is_empty());
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("forcing resume with mismatched plan_id"))
        );
    }

    #[test]
    fn run_resume_errors_when_state_is_missing() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let err = run_resume(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("no existing state found"));
    }

    #[test]
    fn run_resume_runs_publish_when_state_exists() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let receipt = run_resume(&ws, &opts, &mut reporter).expect("resume");
        assert!(receipt.packages.is_empty());
    }

    // Preflight-specific tests

    #[test]
    fn preflight_report_serializes_correctly() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Proven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: true,
            }],
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let parsed: PreflightReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, report.plan_id);
        assert_eq!(parsed.token_detected, report.token_detected);
        assert_eq!(parsed.finishability, report.finishability);
        assert_eq!(parsed.packages.len(), 1);
    }

    #[test]
    fn finishability_proven_when_all_checks_pass() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Proven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: true,
            }],
            timestamp: Utc::now(),
        };

        assert_eq!(report.finishability, Finishability::Proven);
    }

    #[test]
    fn finishability_not_proven_when_ownership_unverified() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::NotProven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: true,
                auth_type: Some(AuthType::Token),
                ownership_verified: false,
                dry_run_passed: true,
            }],
            timestamp: Utc::now(),
        };

        assert_eq!(report.finishability, Finishability::NotProven);
    }

    #[test]
    fn finishability_failed_when_dry_run_fails() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Failed,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: false,
            }],
            timestamp: Utc::now(),
        };

        assert_eq!(report.finishability, Finishability::Failed);
    }

    #[test]
    fn preflight_package_serializes_correctly() {
        let pkg = PreflightPackage {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            already_published: false,
            is_new_crate: true,
            auth_type: Some(AuthType::Token),
            ownership_verified: true,
            dry_run_passed: true,
        };

        let json = serde_json::to_string(&pkg).expect("serialize");
        let parsed: PreflightPackage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, pkg.name);
        assert_eq!(parsed.version, pkg.version);
        assert_eq!(parsed.already_published, pkg.already_published);
        assert_eq!(parsed.is_new_crate, pkg.is_new_crate);
        assert_eq!(parsed.auth_type, pkg.auth_type);
        assert_eq!(parsed.ownership_verified, pkg.ownership_verified);
        assert_eq!(parsed.dry_run_passed, pkg.dry_run_passed);
    }

    #[test]
    fn auth_type_serializes_correctly() {
        let token_auth = AuthType::Token;
        let tp_auth = AuthType::TrustedPublishing;
        let unknown_auth = AuthType::Unknown;

        let json_token = serde_json::to_string(&token_auth).expect("serialize");
        let parsed_token: AuthType = serde_json::from_str(&json_token).expect("deserialize");
        assert_eq!(parsed_token, AuthType::Token);

        let json_tp = serde_json::to_string(&tp_auth).expect("serialize");
        let parsed_tp: AuthType = serde_json::from_str(&json_tp).expect("deserialize");
        assert_eq!(parsed_tp, AuthType::TrustedPublishing);

        let json_unknown = serde_json::to_string(&unknown_auth).expect("serialize");
        let parsed_unknown: AuthType = serde_json::from_str(&json_unknown).expect("deserialize");
        assert_eq!(parsed_unknown, AuthType::Unknown);
    }

    // Integration tests for preflight scenarios

    #[test]
    #[serial]
    fn preflight_with_all_packages_already_published() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        // Mock registry: version already exists (200)
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(200, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.skip_ownership_check = true;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert_eq!(report.packages.len(), 1);
        assert!(report.packages[0].already_published);
        assert!(!report.packages[0].is_new_crate);
        assert!(report.packages[0].dry_run_passed);
        server.join();
    }

    #[test]
    #[serial]
    fn preflight_with_new_crates() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        // Mock registry: crate doesn't exist (404 for both crate and version)
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.skip_ownership_check = true;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert_eq!(report.packages.len(), 1);
        assert!(!report.packages[0].already_published);
        assert!(report.packages[0].is_new_crate);
        assert!(report.packages[0].dry_run_passed);
        server.join();
    }

    #[test]
    #[serial]
    fn preflight_with_ownership_verification_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        // Mock registry: version doesn't exist, crate exists, ownership check fails with 403
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(200, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(403, "{}".to_string())],
                ),
            ]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.skip_ownership_check = false;
        // Set a fake token
        let _token = EnvGuard::set("CARGO_REGISTRY_TOKEN", "fake-token");

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert_eq!(report.packages.len(), 1);
        assert!(!report.packages[0].ownership_verified);
        // Should be NotProven because ownership is unverified
        assert_eq!(report.finishability, Finishability::NotProven);
        server.join();
    }

    #[test]
    #[serial]
    fn preflight_with_dry_run_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        // Simulate dry-run failure
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "dry-run failed");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.skip_ownership_check = true;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert_eq!(report.packages.len(), 1);
        assert!(!report.packages[0].dry_run_passed);
        // Should be Failed because dry-run failed
        assert_eq!(report.finishability, Finishability::Failed);
        server.join();
    }

    #[test]
    #[serial]
    fn preflight_strict_ownership_requires_token() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.strict_ownership = true;
        // No token set

        let mut reporter = CollectingReporter::default();
        let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("strict ownership requested but no token found"));
    }

    #[test]
    #[serial]
    fn preflight_finishability_proven_with_all_checks_pass() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");
        let _token = EnvGuard::set("CARGO_REGISTRY_TOKEN", "fake-token");

        // Mock registry: version doesn't exist, crate exists, ownership succeeds
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(200, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(200, r#"{"users":[]}"#.to_string())],
                ),
            ]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = true;
        opts.skip_ownership_check = false;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert_eq!(report.packages.len(), 1);
        assert!(report.packages[0].ownership_verified);
        assert!(report.packages[0].dry_run_passed);
        assert_eq!(report.finishability, Finishability::Proven);
        server.join();
    }

    #[test]
    #[serial]
    fn test_fast_policy_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        // Deliberately set cargo to fail — if dry-run runs, it would fail
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");

        // Only need version_exists + check_new_crate
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = crate::types::PublishPolicy::Fast;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        // dry_run_passed should be true (skipped), not false (cargo would have failed)
        assert!(report.packages[0].dry_run_passed);
        // ownership_verified should be false (skipped by Fast policy)
        assert!(!report.packages[0].ownership_verified);
        // Finishability is NotProven because ownership unverified
        assert_eq!(report.finishability, Finishability::NotProven);
        assert!(
            reporter
                .infos
                .iter()
                .any(|i| i.contains("skipping dry-run"))
        );
        server.join();
    }

    #[test]
    #[serial]
    fn test_balanced_policy_skips_ownership() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");
        let _token = EnvGuard::set("CARGO_REGISTRY_TOKEN", "fake-token");

        // Only need version_exists + check_new_crate (no ownership endpoint)
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = crate::types::PublishPolicy::Balanced;
        opts.skip_ownership_check = false; // would check in Safe, but Balanced overrides

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        // ownership_verified false (Balanced skips ownership)
        assert!(!report.packages[0].ownership_verified);
        // dry_run_passed true (Balanced still runs dry-run)
        assert!(report.packages[0].dry_run_passed);
        server.join();
    }

    #[test]
    #[serial]
    fn test_safe_policy_runs_all_checks() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");
        let _token = EnvGuard::set("CARGO_REGISTRY_TOKEN", "fake-token");

        // Need version_exists + check_new_crate + ownership
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(200, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(200, r#"{"users":[]}"#.to_string())],
                ),
            ]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.policy = crate::types::PublishPolicy::Safe;
        opts.skip_ownership_check = false;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        // All checks ran
        assert!(report.packages[0].dry_run_passed);
        assert!(report.packages[0].ownership_verified);
        assert_eq!(report.finishability, Finishability::Proven);
        server.join();
    }

    #[test]
    #[serial]
    fn test_verify_mode_none_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        // Set cargo to fail — if dry-run ran, it would fail
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.verify_mode = crate::types::VerifyMode::None;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        // dry_run_passed is true because verify_mode=None skips it
        assert!(report.packages[0].dry_run_passed);
        assert!(
            reporter
                .infos
                .iter()
                .any(|i| i.contains("skipping dry-run"))
        );
        server.join();
    }

    #[test]
    #[serial]
    fn test_verify_mode_package_runs_per_package() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.verify_mode = crate::types::VerifyMode::Package;

        let mut reporter = CollectingReporter::default();
        let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

        assert!(report.packages[0].dry_run_passed);
        assert!(
            reporter
                .infos
                .iter()
                .any(|i| i.contains("per-package dry-run"))
        );
        server.join();
    }
}
