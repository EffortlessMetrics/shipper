use super::*;
use shipper_core::state::events::{EventLog, events_path};
use shipper_core::types::{EnvironmentFingerprint, PackageEvidence, PackageReceipt, Receipt};

fn held_state() -> PackageState {
    PackageState::CleanupIncomplete {
        message: "owned process cleanup could not be confirmed".into(),
    }
}

fn progress(name: &str, state: PackageState) -> PackageProgress {
    PackageProgress {
        name: name.into(),
        version: "0.1.0".into(),
        attempts: 1,
        state,
        last_updated_at: Utc::now(),
    }
}

fn receipt_for(progress: &[&PackageProgress]) -> Receipt {
    let now = Utc::now();
    Receipt {
        receipt_version: "shipper.receipt.v2".into(),
        plan_id: "cleanup-plan".into(),
        registry: Registry::crates_io(),
        started_at: now,
        finished_at: now,
        packages: progress
            .iter()
            .map(|package| PackageReceipt {
                name: package.name.clone(),
                version: package.version.clone(),
                attempts: package.attempts,
                state: package.state.clone(),
                started_at: now,
                finished_at: now,
                duration_ms: 1,
                evidence: PackageEvidence {
                    attempts: Vec::new(),
                    readiness_checks: Vec::new(),
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            })
            .collect(),
        event_log_path: "events.jsonl".into(),
        git_context: None,
        environment: EnvironmentFingerprint {
            shipper_version: "test".into(),
            cargo_version: None,
            rust_version: None,
            os: "test".into(),
            arch: "test".into(),
        },
        auth_evidence: None,
        execution_result: ExecutionResult::PartialFailure,
    }
}

fn cleanup_event(package: &str) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        package: package.into(),
        event_type: EventType::PackageCleanupIncomplete {
            message: "owned process cleanup could not be confirmed".into(),
        },
    }
}

#[test]
#[serial]
fn cleanup_incomplete_counts_and_json_override_other_package_postures() -> Result<()> {
    let td = tempdir()?;
    let packages = [
        progress("pending", PackageState::Pending),
        progress(
            "ambiguous",
            PackageState::Ambiguous {
                message: "unknown".into(),
            },
        ),
        progress("held", held_state()),
        progress("uploaded", PackageState::Uploaded),
    ];
    let progress: Vec<_> = packages.iter().collect();
    // A historical/incomplete receipt cannot turn the omitted pending package
    // into resume advice while another retained package is held.
    let receipt = receipt_for(&progress[1..]);
    let counts = command_package_counts(&progress, &receipt);
    assert_eq!(counts.cleanup_incomplete, 1);
    assert_eq!(counts.pending, 1);
    assert_eq!(counts.ambiguous, 1);
    assert_eq!(counts.uploaded, 1);
    assert_eq!(counts.failed, 0);
    assert!(counts.next_package.is_none());
    assert!(counts.first_omitted_resumable.is_none());
    assert!(!legacy_safe_to_resume(&counts));

    let publish = build_publish_operator_outcome(&receipt.execution_result, &counts, td.path());
    let publish = build_publish_json_report(&receipt, &progress, td.path(), td.path(), publish)?;
    let resume = build_resume_operator_outcome(&receipt.execution_result, &counts, td.path());
    let resume = build_resume_json_report(&receipt, &progress, td.path(), td.path(), resume)?;
    for (report, safety) in [
        (serde_json::to_value(publish)?, "safe_to_rerun"),
        (serde_json::to_value(resume)?, "safe_to_resume"),
    ] {
        assert_eq!(report["cleanup_incomplete"], 1);
        assert_eq!(report[safety], false);
        assert_eq!(report["outcome"][safety]["value"], false);
        assert_eq!(report["outcome"]["failure_class"], "cleanup_incomplete");
        assert_eq!(
            report["outcome"]["next_action"]["kind"],
            "stop_and_investigate"
        );
        assert!(report["outcome"]["next_action"].get("command").is_none());
        assert_eq!(report["packages"][2]["state"], "cleanup_incomplete");
        assert_eq!(
            report["receipt"]["packages"][1]["state"]["state"],
            "cleanup_incomplete"
        );
    }

    // Keep the legacy envelope stable for pre-existing states.
    let ordinary = [progress[0]];
    let ordinary_receipt = receipt_for(&ordinary);
    let counts = command_package_counts(&ordinary, &ordinary_receipt);
    let outcome =
        build_resume_operator_outcome(&ordinary_receipt.execution_result, &counts, td.path());
    let report =
        build_resume_json_report(&ordinary_receipt, &ordinary, td.path(), td.path(), outcome)?;
    let json = serde_json::to_value(report)?;
    assert!(json.get("cleanup_incomplete").is_none());
    assert_eq!(json["safe_to_resume"], true);
    Ok(())
}

#[test]
#[serial]
fn cleanup_incomplete_errors_survive_admission_context_without_retry_advice() -> Result<()> {
    let td = tempdir()?;
    let mut log = EventLog::new();
    log.record(cleanup_event("retained-outside-plan@0.1.0"));
    log.write_to_file(&events_path(td.path()))?;
    for command in [CleanupCommand::Publish, CleanupCommand::Resume] {
        let error = engine::validate_publish_cleanup_safety(td.path())
            .expect_err("event-only hold must refuse admission");
        assert!(engine::is_publish_admission_error(&error));
        let error = error
            .context("CARGO_REGISTRY_TOKEN=CLEANUP_ERROR_SECRET")
            .context(
                "cleanup evidence could not be recorded reliably; no safe rerun is established",
            );
        let marked = match command {
            CleanupCommand::Publish => {
                mark_publish_engine_error(error, "json", td.path(), td.path())
            }
            CleanupCommand::Resume => mark_resume_engine_error(error, "json", td.path(), td.path()),
        };
        let held = marked
            .downcast_ref::<CleanupIncompleteError>()
            .context("typed CLI cleanup stop")?;
        assert!(!held.rendered_error.contains("CLEANUP_ERROR_SECRET"));
        assert!(!held.rendered_error.contains("shipper publish"));
        assert!(!held.rendered_error.contains("--max-attempts"));
        let json = serde_json::to_value(held.report())?;
        assert_eq!(json["category"], "cleanup_incomplete");
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("no safe rerun is established")
        );
        assert!(
            !json["detail"]
                .as_str()
                .unwrap()
                .contains("CLEANUP_ERROR_SECRET")
        );
        assert_eq!(json["next_action"]["kind"], "stop_and_investigate");
        assert!(json["next_action"].get("command").is_none());
        let (name, safety, absent_safety) = match command {
            CleanupCommand::Publish => ("publish", "safe_to_rerun", "safe_to_resume"),
            CleanupCommand::Resume => ("resume", "safe_to_resume", "safe_to_rerun"),
        };
        assert_eq!(json["command"], name);
        assert_eq!(json[safety]["value"], false);
        assert!(json.get(absent_safety).is_none());
        assert_eq!(
            json["evidence"],
            serde_json::json!([events_path(td.path()).display().to_string()])
        );
    }
    assert!(!td.path().join("state.json").exists());
    Ok(())
}

#[test]
fn cleanup_incomplete_event_clears_schedules_even_before_later_peer_events() {
    let hold = cleanup_event("held@0.1.0");
    assert_eq!(package_state_name(&held_state()), "cleanup_incomplete");
    assert_eq!(package_state_label(&held_state()), "cleanup_incomplete");
    assert_eq!(
        event_type_name(&hold.event_type),
        "package_cleanup_incomplete"
    );
    assert!(summarize_event(&hold).starts_with("cleanup incomplete:"));
    assert!(event_type_clears_next_action(&hold.event_type));
    assert!(!event_type_is_active_progress(&hold.event_type));
    let scheduled = PublishEvent {
        timestamp: Utc::now(),
        package: "peer@0.1.0".into(),
        event_type: EventType::RetryScheduled {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 5_000,
            next_attempt_at: Utc::now() + chrono::Duration::seconds(5),
            reason: ErrorClass::Retryable,
            message: "retryable peer".into(),
        },
    };
    assert!(latest_status_watch_next_action(std::slice::from_ref(&scheduled)).is_some());
    for events in [vec![scheduled.clone(), hold.clone()], vec![hold, scheduled]] {
        assert!(latest_status_watch_next_action(&events).is_none());
        assert!(latest_active_progress_event(&events).is_none());
        assert!(current_status_package(&events, None, &[]).is_none());
    }
}

#[test]
#[serial]
fn cleanup_incomplete_watch_is_global_with_or_without_state_projection() -> Result<()> {
    let td = tempdir()?;
    let now = Utc::now();
    let ws = plan::PlannedWorkspace {
        workspace_root: td.path().into(),
        plan: ReleasePlan {
            plan_version: "shipper.plan.v1".into(),
            plan_id: "cleanup-plan".into(),
            created_at: now,
            registry: Registry::crates_io(),
            packages: vec![PlannedPackage {
                name: "demo".into(),
                version: "0.1.0".into(),
                manifest_path: td.path().join("Cargo.toml"),
                regime: None,
            }],
            dependencies: BTreeMap::new(),
        },
        skipped: Vec::new(),
    };
    for (index, package, projected) in [
        (0, "demo@0.1.0", false),
        (1, "retired@0.1.0", false),
        (2, "retired@0.1.0", true),
    ] {
        let state_dir = td.path().join(index.to_string());
        if projected {
            shipper_core::state::execution_state::save_state(
                &state_dir,
                &ExecutionState {
                    state_version: "shipper.state.v1".into(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    created_at: now,
                    updated_at: now,
                    attempt_history: Vec::new(),
                    packages: BTreeMap::from([(package.into(), progress("retired", held_state()))]),
                },
            )?;
        } else {
            let mut log = EventLog::new();
            log.record(cleanup_event(package));
            log.write_to_file(&events_path(&state_dir))?;
        }
        let report = build_status_watch_report(&ws, &state_dir)?;
        assert_eq!(report.counts.cleanup_incomplete, usize::from(index == 0));
        assert!(report.current_package.is_none());
        assert!(report.next_action.is_none());
        let json = serde_json::to_value(&report)?;
        assert_eq!(json["outcome"]["safe_to_resume"]["value"], false);
        assert_eq!(
            json["outcome"]["next_action"]["kind"],
            "stop_and_investigate"
        );
        assert!(json["outcome"]["next_action"].get("command").is_none());
        let mut rendered = Vec::new();
        write_status_watch_report(&report, "text", &mut rendered)?;
        let rendered = String::from_utf8(rendered)?;
        assert!(rendered.contains("Safe to resume: no"));
        assert!(!rendered.contains("next: retry"));
    }
    Ok(())
}
