use super::*;

fn direct(
    fixture: &Fixture,
    opts: &RuntimeOptions,
    supplied: &mut ExecutionState,
    cargo_exit: &str,
) -> Result<Vec<PackageReceipt>> {
    fixture.with_cargo(cargo_exit, || {
        let registry = test_registry_client(fixture.ws.plan.registry.clone())?;
        parallel::run_publish_parallel(
            &fixture.ws,
            opts,
            supplied,
            &fixture.state_dir,
            &registry,
            &mut CollectingReporter::default(),
        )
    })
}

#[test]
#[serial]
fn missing_marker_and_invalid_report_identity_cannot_hide_the_current_stop() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let supplied = fixture.seed_stop()?;
        let path = events::events_path(&fixture.state_dir);
        let raw = fs::read_to_string(&path)?;
        let mut lines: Vec<_> = raw.lines().collect();
        ensure!(
            lines
                .pop()
                .context("stop marker")?
                .contains("execution_stopped")
        );
        fs::write(&path, format!("{}\n", lines.join("\n")))?;
        let path = state::reconciliation_path(&fixture.state_dir);
        let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        report["records"][0]["name"] = "forged-package-identity".into();
        fs::write(path, serde_json::to_vec(&report)?)?;
        let retained = fixture.snapshot()?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        for force in [false, true] {
            opts.force = force;
            for entry in [Entry::Publish, Entry::Resume, Entry::DirectParallel] {
                let mut caller = supplied.clone();
                let hits = fixture.registry.hits();
                let error = invoke(&fixture, entry, &opts, &mut caller)
                    .err()
                    .context("combined corruption must refuse")?;
                ensure!(is_publish_admission_error(&error), "{entry:?}: {error:#}");
                ensure!(
                    fixture.snapshot()? == retained,
                    "{entry:?} changed evidence"
                );
                ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == hits);
                ensure!(serde_json::to_value(&caller)? == serde_json::to_value(&supplied)?);
            }
        }
    }
    Ok(())
}

fn assert_informational_start_preserves_admission(execution_segment: bool) -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let supplied = fixture.seed_stop()?;
        let path = events::events_path(&fixture.state_dir);
        let raw = fs::read_to_string(&path)?;
        let mut lines: Vec<_> = raw.lines().collect();
        ensure!(
            lines
                .pop()
                .context("stop marker")?
                .contains("execution_stopped")
        );
        fs::write(&path, format!("{}\n", lines.join("\n")))?;
        let mut log = events::EventLog::new();
        let starts = if execution_segment {
            vec![
                ("all", EventType::ExecutionStarted),
                (
                    "all",
                    EventType::PlanCreated {
                        plan_id: fixture.ws.plan.plan_id.clone(),
                        package_count: 1,
                    },
                ),
            ]
        } else {
            vec![(
                "demo@0.1.0",
                EventType::PackageStarted {
                    name: "demo".into(),
                    version: "0.1.0".into(),
                },
            )]
        };
        for (package, event_type) in starts {
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type,
                package: package.into(),
            });
        }
        log.write_to_file(&path)?;
        let report_path = state::reconciliation_path(&fixture.state_dir);
        let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
        report["records"][0]["name"] = "forged-package-identity".into();
        fs::write(report_path, serde_json::to_vec(&report)?)?;
        let retained = fixture.snapshot()?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        for force in [false, true] {
            opts.force = force;
            for entry in [Entry::Publish, Entry::Resume, Entry::DirectParallel] {
                let mut caller = supplied.clone();
                let error = invoke(&fixture, entry, &opts, &mut caller)
                    .err()
                    .context("a start marker cannot retire the retained attempt")?;
                ensure!(is_publish_admission_error(&error), "{entry:?}: {error:#}");
                ensure!(
                    fixture.snapshot()? == retained,
                    "{entry:?} changed evidence"
                );
                ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == 2);
                ensure!(serde_json::to_value(&caller)? == serde_json::to_value(&supplied)?);
            }
        }
    }
    Ok(())
}

#[test]
#[serial]
fn informational_execution_start_cannot_retire_a_retained_attempt() -> Result<()> {
    assert_informational_start_preserves_admission(true)
}

#[test]
#[serial]
fn informational_package_start_cannot_retire_a_retained_attempt() -> Result<()> {
    assert_informational_start_preserves_admission(false)
}

#[test]
#[serial]
fn fresh_direct_stop_can_resume_then_repeat_and_finalize_without_reupload() -> Result<()> {
    for finalize_parallel in [false, true] {
        let fixture = Fixture::new(finalize_parallel, false);
        let mut supplied = init_state(&fixture.ws, &fixture.state_dir)?;
        let error = direct(&fixture, &fixture.opts, &mut supplied, "1")
            .err()
            .context("direct fixture must stop")?;
        ensure!(matches!(
            classify_publish_stop(&error),
            Some(PublishStopClassification::RecoverableNotPublished { .. })
        ));
        ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == 2);
        let prior = fixture.retained()?;
        ensure!(prior.packages["demo@0.1.0"].attempts == 1);
        let prefix = fs::read(events::events_path(&fixture.state_dir))?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        fixture.registry.respond_with(&[404, 200]);
        direct(&fixture, &opts, &mut supplied, "0")
            .context("a direct controlled stop must admit its raised-ceiling continuation")?;
        ensure!(matches!(
            classify_publish_stop(&error),
            Some(PublishStopClassification::RecoverableNotPublished {
                evidence_consistent: true
            })
        ));
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == 4);
        ensure!(matches!(
            supplied.packages["demo@0.1.0"].state,
            PackageState::Published
        ));
        ensure!(supplied.packages["demo@0.1.0"].attempts == 2);
        ensure!(supplied.attempt_history.len() == 2);
        ensure!(
            serde_json::to_value(&supplied.attempt_history[..1])?
                == serde_json::to_value(&prior.attempt_history)?
        );
        ensure!(fs::read(events::events_path(&fixture.state_dir))?.starts_with(&prefix));
        let history = serde_json::to_value(&supplied.attempt_history)?;
        direct(&fixture, &opts, &mut supplied, "71")
            .context("successful direct continuation must be idempotent")?;
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == 4);
        ensure!(serde_json::to_value(&supplied.attempt_history)? == history);
        ensure!(!state::receipt_path(&fixture.state_dir).exists());
        let log = events::EventLog::read_from_file(&events::events_path(&fixture.state_dir))?;
        ensure!(
            !log.all_events()
                .iter()
                .any(|event| matches!(event.event_type, EventType::ExecutionFinished { .. }))
        );
        ensure!(
            log.all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::ExecutionStarted))
                .count()
                == 3
        );
        let outcome = fixture.with_cargo("71", || {
            run_resume_with_state(&fixture.ws, &opts, &mut CollectingReporter::default())
        })?;
        ensure!(outcome.receipt.execution_result == ExecutionResult::Success);
        ensure!(state::receipt_path(&fixture.state_dir).exists());
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == 4);
        ensure!(serde_json::to_value(&outcome.state.attempt_history)? == history);
    }
    Ok(())
}

#[test]
#[serial]
fn direct_continuation_supersedes_the_previous_orchestrator_stop() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let mut supplied = fixture.seed_stop()?;
        let prefix = fs::read(events::events_path(&fixture.state_dir))?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        fixture.registry.respond_with(&[404, 200]);
        direct(&fixture, &opts, &mut supplied, "0")?;
        let history = serde_json::to_value(&supplied.attempt_history)?;
        direct(&fixture, &opts, &mut supplied, "71")
            .context("the previous stop cannot remain current after direct continuation")?;
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == 4);
        ensure!(supplied.packages["demo@0.1.0"].attempts == 2);
        ensure!(serde_json::to_value(&supplied.attempt_history)? == history);
        ensure!(fs::read(events::events_path(&fixture.state_dir))?.starts_with(&prefix));
        ensure!(!state::receipt_path(&fixture.state_dir).exists());
        let receipt = fixture.with_cargo("71", || {
            run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
        })?;
        ensure!(receipt.execution_result == ExecutionResult::Success);
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == 4);
    }
    Ok(())
}
