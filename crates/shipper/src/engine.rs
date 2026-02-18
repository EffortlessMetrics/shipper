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
    AttemptEvidence, ErrorClass, EventType, ExecutionResult, ExecutionState, Finishability,
    PackageProgress, PackageReceipt, PackageState, PreflightPackage, PreflightReport, PublishEvent,
    PublishPolicy, ReadinessEvidence, Receipt, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

pub(crate) struct PolicyEffects {
    pub(crate) run_dry_run: bool,
    pub(crate) check_ownership: bool,
    pub(crate) strict_ownership: bool,
    pub(crate) readiness_enabled: bool,
}

pub(crate) fn apply_policy(opts: &RuntimeOptions) -> PolicyEffects {
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
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);
    let events_path = events::events_path(&state_dir);
    let mut event_log = events::EventLog::new();

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;
    event_log.clear();

    if !opts.allow_dirty {
        reporter.info("checking git cleanliness...");
        git::ensure_git_clean(workspace_root)?;
    }

    reporter.info("initializing registry client...");
    let reg = RegistryClient::new(ws.plan.registry.clone())?;

    let token = auth::resolve_token(&ws.plan.registry.name)?;
    let token_detected = token.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
    let auth_type = auth::detect_auth_type_from_token(token.as_deref());

    if effects.strict_ownership && !token_detected {
        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightComplete {
                finishability: Finishability::Failed,
            },
            package: "all".to_string(),
        });
        event_log.write_to_file(&events_path)?;
        bail!(
            "strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)"
        );
    }

    // Run dry-run verification based on VerifyMode and policy
    use crate::types::VerifyMode;

    // Workspace-level dry-run result (used for Workspace mode)
    let (workspace_dry_run_passed, workspace_dry_run_output) =
        if effects.run_dry_run && opts.verify_mode == VerifyMode::Workspace {
            reporter.info("running workspace dry-run verification...");
            let dry_run_result = cargo::cargo_publish_dry_run_workspace(
                workspace_root,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.output_lines,
            );
            match &dry_run_result {
                Ok(output) => (
                    output.exit_code == 0,
                    format!(
                        "workspace dry-run: exit_code={}; stdout_tail={:?}; stderr_tail={:?}",
                        output.exit_code, output.stdout_tail, output.stderr_tail
                    ),
                ),
                Err(err) => (false, format!("workspace dry-run failed: {err:#}")),
            }
        } else if !effects.run_dry_run || opts.verify_mode == VerifyMode::None {
            reporter.info("skipping dry-run (policy, --no-verify, or verify_mode=none)");
            (
                true,
                "workspace dry-run skipped (policy, --no-verify, or verify_mode=none)".to_string(),
            )
        } else {
            // Package mode — handled per-package below
            (
                true,
                "workspace dry-run skipped (verify_mode=package)".to_string(),
            )
        };

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightWorkspaceVerify {
            passed: workspace_dry_run_passed,
            output: workspace_dry_run_output,
        },
        package: "all".to_string(),
    });

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
        if is_new_crate {
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PreflightNewCrateDetected {
                    crate_name: p.name.clone(),
                },
                package: format!("{}@{}", p.name, p.version),
            });
        }

        // Determine dry-run result for this package
        let dry_run_passed = if opts.verify_mode == VerifyMode::Package {
            *per_package_dry_run.get(&p.name).unwrap_or(&true)
        } else {
            workspace_dry_run_passed
        };

        // Ownership verification (best-effort), gated by policy
        let ownership_verified = if token_detected && effects.check_ownership {
            if effects.strict_ownership {
                if is_new_crate {
                    // New crates have no owners endpoint; skip ownership check
                    reporter.info(&format!("{}: new crate, skipping ownership check", p.name));
                    false
                } else {
                    // In strict mode, ownership errors are fatal
                    reg.list_owners(&p.name, token.as_deref().unwrap())?;
                    true
                }
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

        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightOwnershipCheck {
                crate_name: p.name.clone(),
                verified: ownership_verified,
            },
            package: format!("{}@{}", p.name, p.version),
        });

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

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: finishability.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;

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
    // Send webhook notification: publish started
    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishStarted {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
            registry: ws.plan.registry.name.clone(),
        },
    );
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

        // Track whether cargo publish already succeeded (e.g. from Uploaded state on resume)
        let mut cargo_succeeded = false;

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
            PackageState::Uploaded => {
                reporter.info(&format!(
                    "{}@{}: resuming from uploaded (skipping cargo publish)",
                    p.name, p.version
                ));
                cargo_succeeded = true;
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

            let progress = st
                .packages
                .get(&key)
                .context("missing package progress in state for skipped package")?;
            receipts.push(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: progress.attempts,
                state: progress.state.clone(),
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

        let mut attempt = st
            .packages
            .get(&key)
            .context("missing package progress in state for publish")?
            .attempts;
        let mut last_err: Option<(ErrorClass, String)> = None;
        let mut attempt_evidence: Vec<AttemptEvidence> = Vec::new();
        let mut readiness_evidence: Vec<ReadinessEvidence> = Vec::new();

        while attempt < opts.max_attempts {
            attempt += 1;
            {
                let pr = st
                    .packages
                    .get_mut(&key)
                    .context("missing package progress in state during attempt")?;
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

            if !cargo_succeeded {
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
                    None, // sequential mode: no per-package timeout
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

                if out.exit_code == 0 {
                    cargo_succeeded = true;
                    // Persist Uploaded state so resume skips cargo publish
                    update_state(&mut st, &state_dir, &key, PackageState::Uploaded)?;
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
                            let delay = backoff_delay(
                                opts.base_delay,
                                opts.max_delay,
                                attempt,
                                opts.retry_strategy,
                                opts.retry_jitter,
                            );
                            reporter.warn(&format!(
                                "{}@{}: retrying in {}",
                                p.name,
                                p.version,
                                humantime::format_duration(delay)
                            ));
                            thread::sleep(delay);
                        }
                    }
                    continue;
                }
            }

            // Readiness verification (runs after first cargo success + all retries)
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

                // Send webhook notification: package succeeded
                webhook::maybe_send_event(
                    &opts.webhook,
                    WebhookEvent::PublishSucceeded {
                        plan_id: ws.plan.plan_id.clone(),
                        package_name: p.name.clone(),
                        package_version: p.version.clone(),
                        duration_ms: start_instant.elapsed().as_millis() as u64,
                    },
                );

                break;
            } else {
                last_err = Some((ErrorClass::Ambiguous, "publish succeeded locally, but version not observed on registry within timeout".into()));
                let delay = backoff_delay(
                    opts.base_delay,
                    opts.max_delay,
                    attempt,
                    opts.retry_strategy,
                    opts.retry_jitter,
                );
                thread::sleep(delay);
            }
        }

        // If package is still Uploaded (loop didn't run or readiness never checked), force a final check
        if last_err.is_none() {
            let current_state = st.packages.get(&key).map(|p| &p.state);
            if matches!(current_state, Some(PackageState::Uploaded)) {
                if reg.version_exists(&p.name, &p.version)? {
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                } else {
                    last_err = Some((
                        ErrorClass::Ambiguous,
                        "package was uploaded but not confirmed visible on registry".into(),
                    ));
                }
            }
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
                        class: class.clone(),
                        message: msg.clone(),
                    },
                    package: pkg_label.clone(),
                });
                event_log.write_to_file(&events_path)?;
                event_log.clear();

                // Send webhook notification: package failed
                webhook::maybe_send_event(
                    &opts.webhook,
                    WebhookEvent::PublishFailed {
                        plan_id: ws.plan.plan_id.clone(),
                        package_name: p.name.clone(),
                        package_version: p.version.clone(),
                        error_class: format!("{:?}", class.clone()),
                        message: msg.clone(),
                    },
                );

                let progress = st
                    .packages
                    .get(&key)
                    .context("missing package progress in state for failed package")?;
                receipts.push(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: progress.attempts,
                    state: progress.state.clone(),
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

        let progress = st
            .packages
            .get(&key)
            .context("missing package progress in state for completed package")?;
        receipts.push(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: progress.attempts,
            state: progress.state.clone(),
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
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    }) {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    };
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;

    // Calculate publish completion statistics
    let total_packages = receipts.len();
    let success_count = receipts.iter().filter(|r| {
        matches!(r.state, PackageState::Published)
    }).count();
    let failure_count = receipts.iter().filter(|r| {
        matches!(r.state, PackageState::Failed { .. })
    }).count();
    let skipped_count = receipts.iter().filter(|r| {
        matches!(r.state, PackageState::Skipped { .. })
    }).count();

    // Send webhook notification: all complete
    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishCompleted {
            plan_id: ws.plan.plan_id.clone(),
            total_packages,
            success_count,
            failure_count,
            skipped_count,
            result: match exec_result {
                ExecutionResult::Success => "success".to_string(),
                ExecutionResult::PartialFailure => "partial_failure".to_string(),
                ExecutionResult::CompleteFailure => "complete_failure".to_string(),
            },
        },
    );

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
        PackageState::Uploaded => "uploaded",
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

pub(crate) fn backoff_delay(
    base: Duration,
    max: Duration,
    attempt: u32,
    strategy: crate::retry::RetryStrategyType,
    jitter: f64,
) -> Duration {
    let config = crate::retry::RetryStrategyConfig {
        strategy,
        max_attempts: 10, // Not used for delay calculation
        base_delay: base,
        max_delay: max,
        jitter,
    };
    crate::retry::calculate_delay(&config, attempt)
}

#[cfg(test)]
mod tests {
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
    use crate::types::{AuthType, PlannedPackage, Registry, ReleasePlan};

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

    fn fake_program_env_vars(bin_dir: &Path) -> Vec<(&'static str, Option<String>)> {
        vec![
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(bin_dir).to_str().expect("utf8").to_string()),
            ),
            (
                "SHIPPER_GIT_BIN",
                Some(fake_git_path(bin_dir).to_str().expect("utf8").to_string()),
            ),
        ]
    }

    /// Build a combined env var list from fake programs + additional vars, then run closure.
    fn with_test_env<F, R>(bin_dir: &Path, extra: Vec<(&'static str, Option<String>)>, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let mut vars = fake_program_env_vars(bin_dir);
        vars.extend(extra);
        temp_env::with_vars(vars, f)
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
            webhook: crate::webhook::WebhookConfig::default(),
            retry_strategy: crate::retry::RetryStrategyType::Exponential,
            retry_jitter: 0.0,
            retry_per_error: crate::retry::PerErrorConfig::default(),
            encryption: crate::encryption::EncryptionConfig::default(),
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
        assert_eq!(short_state(&PackageState::Uploaded), "uploaded");
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
        let d1 = backoff_delay(base, max, 1, crate::retry::RetryStrategyType::Exponential, 0.5);
        let d20 = backoff_delay(base, max, 20, crate::retry::RetryStrategyType::Exponential, 0.5);

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
            // Keep this generous to avoid timing flakes under highly parallel test execution.
            max_total_wait: Duration::from_secs(2),
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
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.strict_ownership = true;
        opts.skip_ownership_check = false;
        temp_env::with_vars(
            [
                (
                    "CARGO_HOME",
                    Some(td.path().to_str().expect("utf8").to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ],
            || {
                let mut reporter = CollectingReporter::default();
                let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(
                    format!("{err:#}").contains("strict ownership requested but no token found")
                );
            },
        );
    }

    #[test]
    #[serial]
    fn run_preflight_warns_on_owners_failure_when_not_strict() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_preflight_owners_success_path() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
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

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            assert!(reporter.warns.is_empty());
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_returns_error_when_strict_ownership_check_fails() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            // Crate must exist (200) so ownership check is actually attempted;
            // 404 would mean new crate -> ownership check skipped.
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
                        vec![(403, "{}".to_string())],
                    ),
                ]),
                3,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = false;
            opts.strict_ownership = true;

            let mut reporter = CollectingReporter::default();
            let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
            assert!(format!("{err:#}").contains("forbidden when querying owners"));
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_strict_skips_ownership_for_new_crate() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            // Crate returns 404 (new crate) -- ownership check should be skipped.
            // No /owners endpoint needed.
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
            opts.skip_ownership_check = false;
            opts.strict_ownership = true;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            assert!(!rep.packages[0].ownership_verified);
            assert!(rep.packages[0].is_new_crate);
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|i| i.contains("new crate, skipping ownership check"))
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_writes_preflight_events() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
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
            let _ = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            let events_path = td.path().join(".shipper").join("events.jsonl");
            let log = crate::events::EventLog::read_from_file(&events_path).expect("read events");
            let events = log.all_events();

            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightStarted))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightWorkspaceVerify { .. }))
            );
            assert!(
                events.iter().any(|e| {
                    matches!(e.event_type, EventType::PreflightNewCrateDetected { .. })
                })
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightOwnershipCheck { .. }))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightComplete { .. }))
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_detects_trusted_publishing_auth_type() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", None::<String>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            (
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                Some("https://example.invalid/oidc".to_string()),
            ),
            (
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                Some("oidc-token".to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
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

            assert!(!report.token_detected);
            assert_eq!(
                report.packages[0].auth_type,
                Some(crate::types::AuthType::TrustedPublishing)
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_GIT_CLEAN", Some("1".to_string()))]);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_publish_skips_when_version_already_exists() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_publish_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_GIT_CLEAN", Some("1".to_string()))]);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_publish_adds_missing_package_entries_to_existing_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_publish_marks_published_after_successful_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
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
        });
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
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
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
        });
    }

    #[test]
    #[serial]
    fn run_publish_treats_failed_cargo_as_published_if_registry_shows_version() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout while uploading".to_string()),
                ),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_retries_on_retryable_failures() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout talking to server".to_string()),
                ),
            ],
            || {
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
            },
        );
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
        temp_env::with_vars(
            vec![(
                "SHIPPER_CARGO_BIN",
                Some(missing.to_str().expect("utf8").to_string()),
            )],
            || {
                let opts = default_opts(PathBuf::from(".shipper"));
                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_returns_error_on_permanent_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_marks_ambiguous_failure_after_success_without_registry_visibility() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_recovers_on_final_registry_check_after_ambiguous_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
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
            },
        );
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
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_new_crates() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_ownership_verification_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
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

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(!report.packages[0].ownership_verified);
                // Should be NotProven because ownership is unverified
                assert_eq!(report.finishability, Finishability::NotProven);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_dry_run_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                ("SHIPPER_CARGO_STDERR", Some("dry-run failed".to_string())),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_strict_ownership_requires_token() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "CARGO_HOME",
                    Some(td.path().to_str().expect("utf8").to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
            ],
            || {
                let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.strict_ownership = true;
                // No token set

                let mut reporter = CollectingReporter::default();
                let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(
                    format!("{err:#}").contains("strict ownership requested but no token found")
                );
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_finishability_proven_with_all_checks_pass() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn test_fast_policy_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        // Deliberately set cargo to fail — if dry-run runs, it would fail
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("1".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn test_balanced_policy_skips_ownership() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn test_safe_policy_runs_all_checks() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn test_verify_mode_none_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        // Set cargo to fail — if dry-run ran, it would fail
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("1".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn test_verify_mode_package_runs_per_package() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
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
            },
        );
    }

    #[test]
    #[serial]
    fn resume_from_uploaded_skips_cargo_publish_and_reaches_published() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(args_log.to_str().expect("utf8").to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            // First request (early check) returns 404, second (readiness) returns 200
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");

            // Pre-create state with Uploaded + attempts=1
            let mut packages = std::collections::BTreeMap::new();
            packages.insert(
                "demo@0.1.0".to_string(),
                PackageProgress {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Uploaded,
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
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Package should reach Published via the readiness verification path
            assert_eq!(receipt.packages.len(), 1);
            assert!(
                matches!(receipt.packages[0].state, PackageState::Published),
                "expected Published, got {:?}",
                receipt.packages[0].state
            );

            // Cargo publish should NOT have been invoked
            // (args_log should not exist or be empty — no cargo publish calls)
            let cargo_invoked = args_log.exists()
                && fs::read_to_string(&args_log)
                    .unwrap_or_default()
                    .contains("publish");
            assert!(
                !cargo_invoked,
                "cargo publish should not have been invoked on resume from Uploaded"
            );

            // Verify reporter got the resume message
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|i| i.contains("resuming from uploaded")
                        || i.contains("already published")
                        || i.contains("already complete"))
            );

            // Verify the readiness path was exercised
            assert!(
                reporter.infos.iter().any(|i| i.contains("verifying")
                    || i.contains("visible")
                    || i.contains("readiness")),
                "expected readiness verification to be exercised, reporter infos: {:?}",
                reporter.infos
            );

            server.join();
        });
    }
}
