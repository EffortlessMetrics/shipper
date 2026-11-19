use super::*;
use crate::lock;
use anyhow::ensure;

mod fixtures;
mod recovery;
use fixtures::Fixture;

#[derive(Clone, Copy, Debug)]
enum Entry {
    Publish,
    Resume,
    DirectParallel,
}

fn invoke(
    fixture: &Fixture,
    entry: Entry,
    opts: &RuntimeOptions,
    supplied: &mut ExecutionState,
) -> Result<()> {
    // A broken admission or negative-control mutation must still reach only
    // the refusing fake publisher, never the host's real Cargo executable.
    fixture.with_cargo("71", || {
        let mut reporter = CollectingReporter::default();
        match entry {
            Entry::Publish => run_publish(&fixture.ws, opts, &mut reporter).map(drop),
            Entry::Resume => run_resume(&fixture.ws, opts, &mut reporter).map(drop),
            Entry::DirectParallel => {
                let registry = test_registry_client(fixture.ws.plan.registry.clone())?;
                parallel::run_publish_parallel(
                    &fixture.ws,
                    opts,
                    supplied,
                    &fixture.state_dir,
                    &registry,
                    &mut reporter,
                )
                .map(drop)
            }
        }
    })
}

#[test]
#[serial]
fn every_entry_refuses_the_same_corrupted_controlled_stop_without_effects() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let supplied = fixture.seed_stop()?;
        let stopped = fixture.snapshot()?;
        for defect in [
            "missing-marker",
            "reordered-marker",
            "corrupt-reconciliation",
            "mismatched-reconciliation",
            "missing-reconciliation",
            "unexpected-receipt",
            "live-record",
            "unknown-record",
            "exhausted-ceiling",
            "missing-state",
        ] {
            fixture.restore(&stopped)?;
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 2;
            opts.webhook.url = fixture.registry.url.clone();
            match defect {
                "missing-marker" | "reordered-marker" => {
                    let path = events::events_path(&fixture.state_dir);
                    let raw = fs::read_to_string(&path)?;
                    let mut lines: Vec<_> = raw.lines().collect();
                    let marker = lines.pop().context("terminal marker")?;
                    ensure!(marker.contains("execution_stopped"));
                    if defect == "reordered-marker" {
                        lines.insert(1, marker);
                    }
                    fs::write(path, format!("{}\n", lines.join("\n")))?;
                }
                "corrupt-reconciliation" => {
                    fs::write(state::reconciliation_path(&fixture.state_dir), b"{broken")?;
                }
                "mismatched-reconciliation" => {
                    let path = state::reconciliation_path(&fixture.state_dir);
                    let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
                    report["plan_id"] = "unrelated-plan".into();
                    fs::write(path, serde_json::to_vec(&report)?)?;
                }
                "missing-reconciliation" => {
                    fs::remove_file(state::reconciliation_path(&fixture.state_dir))?;
                }
                "unexpected-receipt" => {
                    fs::write(
                        state::receipt_path(&fixture.state_dir),
                        b"retained receipt sentinel",
                    )?;
                }
                "live-record" | "unknown-record" => {
                    let mut record = fixture.released_owner_record()?;
                    record["acquired_at"] =
                        serde_json::to_value(Utc::now() - chrono::Duration::days(30))?;
                    if defect == "unknown-record" {
                        record["hostname"] = "different-host".into();
                    }
                    fs::write(fixture.lock_path(), serde_json::to_vec(&record)?)?;
                    if defect == "live-record" && cfg!(target_os = "linux") {
                        ensure!(matches!(
                            crate::state::run_observation::observe_run(
                                &fixture.state_dir,
                                Some(&fixture.ws.workspace_root)
                            )?,
                            crate::cli_bridge::RunObservation::Stopped {
                                liveness: Some(crate::cli_bridge::RunLiveness::Live),
                                ..
                            }
                        ));
                    }
                }
                "exhausted-ceiling" => opts.max_attempts = 1,
                "missing-state" => fs::remove_file(state::state_path(&fixture.state_dir))?,
                _ => unreachable!(),
            }
            let retained = fixture.snapshot()?;
            for force in [false, true] {
                opts.force = force;
                for entry in [Entry::Publish, Entry::Resume, Entry::DirectParallel] {
                    let mut caller_state = supplied.clone();
                    let caller_before = serde_json::to_value(&caller_state)?;
                    let hits = fixture.registry.hits();
                    let cargo = fixture.cargo_calls();
                    let error = invoke(&fixture, entry, &opts, &mut caller_state)
                        .err()
                        .with_context(|| {
                            format!(
                                "{entry:?} admitted {defect}, parallel={parallel}, force={force}"
                            )
                        })?;
                    ensure!(is_publish_admission_error(&error), "{error:#}");
                    if defect == "exhausted-ceiling" {
                        let budget =
                            classify_resume_retry_budget(&error).context("typed ceiling")?;
                        ensure!(budget.current_attempts == 1 && budget.requested_max_attempts == 1);
                    }
                    ensure!(
                        fixture.snapshot()? == retained,
                        "{entry:?} mutated {defect}"
                    );
                    ensure!(
                        fixture.cargo_calls() == cargo,
                        "{entry:?} dispatched Cargo for {defect}"
                    );
                    ensure!(
                        fixture.registry.hits() == hits,
                        "{entry:?} contacted registry/webhook for {defect}"
                    );
                    ensure!(crate::webhook::active_test_deliveries() == 0);
                    ensure!(serde_json::to_value(&caller_state)? == caller_before);
                }
            }
        }
    }
    Ok(())
}

#[test]
#[serial]
fn final_admission_rechecks_the_ceiling_after_a_competing_run() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        fixture.seed_stop()?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        let required = publish::admission::precheck(&fixture.ws, &opts, &fixture.state_dir, false)?;
        ensure!(required);
        // Advance the retained tuple through the real executor while the first
        // caller has only a stale read-only precheck, not invocation ownership.
        fixture.with_cargo("1", || {
            run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
                .expect_err("second genuine controlled stop");
        });
        ensure!(fixture.cargo_calls() == 2);
        ensure!(fixture.retained()?.packages["demo@0.1.0"].attempts == 2);
        let retained = fixture.snapshot()?;
        let hits = fixture.registry.hits();
        let error = fixture.with_cargo("71", || {
            publish::bootstrap::prepare_publish_run(
                &fixture.ws,
                &opts,
                &mut CollectingReporter::default(),
                required,
            )
            .err()
            .context("final admission must reject the current ceiling")
        })?;
        let budget = classify_resume_retry_budget(&error).context("current typed budget")?;
        ensure!(budget.current_attempts == 2 && budget.requested_max_attempts == 2);
        ensure!(fixture.snapshot()? == retained);
        ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == hits);
    }
    Ok(())
}

#[test]
#[serial]
fn final_admission_does_not_initialize_state_that_disappeared_after_precheck() -> Result<()> {
    for parallel in [false, true] {
        for remove_all_artifacts in [false, true] {
            let fixture = Fixture::new(parallel, false);
            fixture.seed_stop()?;
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 2;
            let required =
                publish::admission::precheck(&fixture.ws, &opts, &fixture.state_dir, false)?;
            ensure!(required);
            let paths = fixture.snapshot()?;
            for (path, _) in paths {
                if (remove_all_artifacts || path == state::state_path(&fixture.state_dir))
                    && path.exists()
                {
                    fs::remove_file(path)?;
                }
            }
            let retained = fixture.snapshot()?;
            let hits = fixture.registry.hits();
            let error = fixture.with_cargo("71", || {
                publish::bootstrap::prepare_publish_run(
                    &fixture.ws,
                    &opts,
                    &mut CollectingReporter::default(),
                    required,
                )
                .err()
                .context("missing required state must refuse under ownership")
            })?;
            ensure!(is_publish_admission_error(&error));
            ensure!(format!("{error:#}").contains("retained state is required"));
            ensure!(fixture.snapshot()? == retained);
            ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == hits);
        }
    }
    Ok(())
}

#[test]
#[serial]
fn final_admission_uses_the_reserved_predecessor_before_replacing_metadata() -> Result<()> {
    for parallel in [false, true] {
        for unknown in [false, true] {
            let fixture = Fixture::new(parallel, false);
            fixture.seed_stop()?;
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 2;
            opts.force = true;
            let required =
                publish::admission::precheck(&fixture.ws, &opts, &fixture.state_dir, false)?;
            let mut prior = fixture.released_owner_record()?;
            if unknown {
                prior.as_object_mut().unwrap().remove("process_identity");
            }
            // The sidecar is unowned and a recognized protocol record is
            // recoverable at the ops layer. Semantic controlled-stop liveness
            // must still be checked using this retained record, not our own.
            fs::write(fixture.lock_path(), serde_json::to_vec(&prior)?)?;
            let retained = fixture.snapshot()?;
            let hits = fixture.registry.hits();
            let error = fixture.with_cargo("71", || {
                publish::bootstrap::prepare_publish_run(
                    &fixture.ws,
                    &opts,
                    &mut CollectingReporter::default(),
                    required,
                )
                .err()
                .context("changed predecessor must refuse before claim")
            })?;
            ensure!(is_publish_admission_error(&error));
            ensure!(format!("{error:#}").contains("controlled-stop"));
            ensure!(
                fixture.snapshot()? == retained,
                "predecessor JSON must remain exact"
            );
            ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == hits);
        }
    }
    Ok(())
}

#[test]
#[serial]
fn publish_and_resume_raise_the_same_cumulative_ceiling_without_resetting_history() -> Result<()> {
    for parallel in [false, true] {
        for resume in [false, true] {
            let fixture = Fixture::new(parallel, false);
            let prior = fixture.seed_stop()?;
            let events_path = events::events_path(&fixture.state_dir);
            let prior_events = fs::read(&events_path)?;
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 2;
            fixture.registry.respond_with(&[404, 200]);
            let receipt = fixture.with_cargo("0", || {
                if resume {
                    run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
                } else {
                    run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
                }
            })?;
            let after = fixture.retained()?;
            ensure!(receipt.execution_result == ExecutionResult::Success);
            ensure!(matches!(
                after.packages["demo@0.1.0"].state,
                PackageState::Published
            ));
            ensure!(after.packages["demo@0.1.0"].attempts == 2 && fixture.cargo_calls() == 2);
            ensure!(after.attempt_history.len() == 2);
            ensure!(
                serde_json::to_value(&after.attempt_history[..prior.attempt_history.len()])?
                    == serde_json::to_value(&prior.attempt_history)?
            );
            ensure!(fs::read(&events_path)?.starts_with(&prior_events));
            ensure!(fixture.registry.hits() == 4);
            let hits = fixture.registry.hits();
            fixture.with_cargo("71", || {
                run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
            })?;
            ensure!(fixture.cargo_calls() == 2 && fixture.registry.hits() == hits);
        }
    }
    Ok(())
}

#[test]
#[serial]
fn direct_parallel_uses_level_scope_even_when_the_parallel_flag_is_false() -> Result<()> {
    let fixture = Fixture::new(false, true);
    let mut supplied = fixture.seed_stop()?;
    let retained = fixture.snapshot()?;
    let mut opts = fixture.opts.clone();
    opts.parallel.enabled = false;
    opts.resume_from = Some("later-sibling".into());
    let error = invoke(&fixture, Entry::DirectParallel, &opts, &mut supplied)
        .err()
        .context("same-level exhausted package must be admitted together")?;
    let budget = classify_resume_retry_budget(&error).context("parallel level ceiling")?;
    ensure!(budget.package == "demo@0.1.0" && budget.current_attempts == 1);
    ensure!(
        fixture.snapshot()? == retained
            && fixture.cargo_calls() == 1
            && fixture.registry.hits() == 2
    );
    Ok(())
}

#[test]
#[serial]
fn direct_parallel_reloads_durable_state_and_refuses_a_separately_held_lock() -> Result<()> {
    let fixture = Fixture::new(false, false);
    let mut supplied = init_state(&fixture.ws, &fixture.state_dir)?;
    let owner = lock::LockFile::acquire(&fixture.state_dir, Some(&fixture.ws.workspace_root))?;
    owner.set_plan_id(&fixture.ws.plan.plan_id)?;
    let retained = fixture.snapshot()?;
    for force in [false, true] {
        let mut opts = fixture.opts.clone();
        opts.force = force;
        let error = invoke(&fixture, Entry::DirectParallel, &opts, &mut supplied)
            .err()
            .context("the wrapper cannot borrow an unprovided OS handle")?;
        ensure!(is_publish_admission_error(&error));
        ensure!(
            fixture.snapshot()? == retained
                && fixture.cargo_calls() == 0
                && fixture.registry.hits() == 0
        );
    }
    owner.release()?;
    let mut durable = supplied.clone();
    durable.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Skipped {
        reason: "already published".into(),
    };
    state::save_state(&fixture.state_dir, &durable)?;
    invoke(
        &fixture,
        Entry::DirectParallel,
        &fixture.opts,
        &mut supplied,
    )?;
    ensure!(serde_json::to_value(&supplied.packages)? == serde_json::to_value(&durable.packages)?);
    ensure!(fixture.cargo_calls() == 0 && fixture.registry.hits() == 0);
    Ok(())
}

#[test]
#[serial]
fn fresh_direct_parallel_requires_unattempted_memory_when_no_state_exists() -> Result<()> {
    let fixture = Fixture::new(false, false);
    let fresh = init_state(&fixture.ws, &fixture.state_dir)?;
    fs::remove_file(state::state_path(&fixture.state_dir))?;
    let mut stale = fresh.clone();
    stale.packages.get_mut("demo@0.1.0").unwrap().attempts = 1;
    let before = fixture.snapshot()?;
    ensure!(invoke(&fixture, Entry::DirectParallel, &fixture.opts, &mut stale).is_err());
    ensure!(
        fixture.snapshot()? == before && fixture.registry.hits() == 0 && fixture.cargo_calls() == 0
    );
    let mut supplied = fresh;
    // Older direct-library runs can have package events without run-level
    // lifecycle markers. Losing their state must not turn them into a new run.
    let mut orphaned = events::EventLog::new();
    orphaned.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: "demo".into(),
            version: "0.1.0".into(),
        },
        package: "demo@0.1.0".into(),
    });
    let event_path = events::events_path(&fixture.state_dir);
    orphaned.write_to_file(&event_path)?;
    let orphaned_bytes = fixture.snapshot()?;
    ensure!(
        invoke(
            &fixture,
            Entry::DirectParallel,
            &fixture.opts,
            &mut supplied
        )
        .is_err()
    );
    ensure!(
        fixture.snapshot()? == orphaned_bytes
            && fixture.registry.hits() == 0
            && fixture.cargo_calls() == 0
    );
    fs::remove_file(event_path)?;
    fixture.registry.respond_with(&[200]);
    invoke(
        &fixture,
        Entry::DirectParallel,
        &fixture.opts,
        &mut supplied,
    )?;
    ensure!(matches!(
        supplied.packages["demo@0.1.0"].state,
        PackageState::Skipped { .. }
    ));
    ensure!(fixture.registry.hits() == 1 && fixture.cargo_calls() == 0);
    Ok(())
}

#[test]
#[serial]
fn preflight_and_rehearsal_history_do_not_prevent_a_fresh_publish() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        fs::create_dir_all(&fixture.state_dir)?;
        let events_path = events::events_path(&fixture.state_dir);
        let mut log = events::EventLog::new();
        for event_type in [
            EventType::PlanCreated {
                plan_id: fixture.ws.plan.plan_id.clone(),
                package_count: 1,
            },
            EventType::PreflightStarted,
            EventType::PreflightComplete {
                finishability: crate::types::Finishability::Proven,
            },
            EventType::RehearsalComplete {
                passed: true,
                registry: "fixture".into(),
                plan_id: fixture.ws.plan.plan_id.clone(),
                summary: "separate rehearsal proof".into(),
            },
        ] {
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type,
                package: "all".into(),
            });
        }
        log.write_to_file(&events_path)?;
        let prefix = fs::read(&events_path)?;
        fixture.registry.respond_with(&[404, 200]);
        let receipt = fixture.with_cargo("0", || {
            run_publish(
                &fixture.ws,
                &fixture.opts,
                &mut CollectingReporter::default(),
            )
        })?;
        ensure!(receipt.execution_result == ExecutionResult::Success);
        ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == 2);
        ensure!(fs::read(&events_path)?.starts_with(&prefix));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum UploadCheckpointFault {
    EventAppend,
    ProjectionSync,
}

fn assert_upload_checkpoint_split_write_refuses_resume(fault: UploadCheckpointFault) -> Result<()> {
    use crate::state::file_sync_test_support::{observe, observe_event_append};

    const INJECTED: &str = "injected unresolved upload persistence failure";
    let mut failures = Vec::new();
    for parallel in [false, true] {
        let result = (|| -> Result<()> {
            let fixture = Fixture::new(parallel, false);
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 3;
            // The initial precheck finds no version. Any later registry
            // query fails, so an unsafe Pending resume would call Cargo.
            fixture.registry.respond_with(&[404, 503]);
            let injected = match fault {
                UploadCheckpointFault::EventAppend => {
                    observe_event_append(&fixture.state_dir, |path| {
                        let log = events::EventLog::read_from_file(path)
                            .map_err(std::io::Error::other)?;
                        if log.all_events().last().is_some_and(|event| {
                            matches!(event.event_type, EventType::PackageOutput { .. })
                        }) {
                            return Err(std::io::Error::other(INJECTED));
                        }
                        Ok(())
                    })
                }
                UploadCheckpointFault::ProjectionSync => observe(&fixture.state_dir, |path| {
                    if path.file_name().and_then(|name| name.to_str()) == Some("state.tmp") {
                        let projection: ExecutionState = serde_json::from_slice(&fs::read(path)?)
                            .map_err(std::io::Error::other)?;
                        if matches!(
                            projection.packages["demo@0.1.0"].state,
                            PackageState::Ambiguous { .. }
                        ) {
                            return Err(std::io::Error::other(INJECTED));
                        }
                    }
                    Ok(())
                }),
            };
            let first = fixture.with_cargo("42", || {
                run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
            });
            let error = first.expect_err("persisting upload uncertainty must fail");
            ensure!(
                error.chain().any(|cause| cause.to_string() == INJECTED),
                "{error:#}"
            );
            drop(injected);
            ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == 1);

            let retained = fixture.retained()?;
            let progress = &retained.packages["demo@0.1.0"];
            ensure!(progress.state == PackageState::Pending && progress.attempts == 1);
            let events =
                events::EventLog::read_from_file(&events::events_path(&fixture.state_dir))?;
            ensure!(events.all_events().iter().any(|event| matches!(
                event.event_type,
                EventType::PackageAttempted { attempt: 1, .. }
            )));
            ensure!(
                events
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PackageOutput { .. }))
            );
            ensure!(
                events.all_events().iter().any(|event| matches!(
                    event.event_type,
                    EventType::PackageUploadUnconfirmed { .. }
                )) == matches!(fault, UploadCheckpointFault::ProjectionSync)
            );

            let before = fixture.snapshot()?;
            for entry in [Entry::Resume, Entry::Publish, Entry::DirectParallel] {
                let error = invoke(&fixture, entry, &opts, &mut retained.clone())
                    .expect_err("retained uncertainty must refuse admission");
                ensure!(
                    fixture.cargo_calls() == 1,
                    "{entry:?} started another upload after {fault:?}"
                );
                ensure!(
                    fixture.registry.hits() == 1,
                    "{entry:?} reached the registry after {fault:?}"
                );
                ensure!(is_publish_admission_error(&error), "{error:#}");
                ensure!(
                    format!("{error:#}").contains("unresolved Cargo upload"),
                    "{error:#}"
                );
                ensure!(fixture.snapshot()? == before, "{entry:?} changed evidence");
            }
            Ok(())
        })();
        if let Err(error) = result {
            failures.push(format!("parallel={parallel}: {error:#}"));
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
#[serial]
fn failed_upload_checkpoint_append_must_refuse_resume() -> Result<()> {
    assert_upload_checkpoint_split_write_refuses_resume(UploadCheckpointFault::EventAppend)
}

#[test]
#[serial]
fn failed_upload_checkpoint_projection_must_refuse_resume() -> Result<()> {
    assert_upload_checkpoint_split_write_refuses_resume(UploadCheckpointFault::ProjectionSync)
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[test]
#[serial]
fn only_proved_no_launch_can_resume_the_retained_attempt() -> Result<()> {
    for parallel in [false, true] {
        let missing = Fixture::new(parallel, false);
        let cargo = fake_cargo_path(&missing.bin);
        let disabled = cargo.with_extension("disabled");
        fs::rename(&cargo, &disabled)?;
        let error = missing
            .with_cargo("0", || {
                run_publish(
                    &missing.ws,
                    &missing.opts,
                    &mut CollectingReporter::default(),
                )
            })
            .expect_err("Cargo executable is missing");
        ensure!(
            error.is::<crate::ops::process::CommandNotLaunched>(),
            "{error:#}"
        );
        ensure!(missing.cargo_calls() == 0 && missing.registry.hits() == 1);

        let path = events::events_path(&missing.state_dir);
        let log = events::EventLog::read_from_file(&path)?;
        ensure!(log.all_events().iter().any(|event| matches!(
            event.event_type,
            EventType::PackageNotLaunched { attempt: 1, .. }
        )));
        ensure!(log.all_events().iter().all(|event| !matches!(
            event.event_type,
            EventType::PackageOutput { .. } | EventType::PublishReconciling { .. }
        )));
        let retained = missing.retained()?;
        fs::rename(&disabled, &cargo)?;
        missing
            .with_cargo("0", || {
                run_resume(
                    &missing.ws,
                    &missing.opts,
                    &mut CollectingReporter::default(),
                )
            })
            .expect_err("the no-launch attempt consumed the current budget");
        ensure!(missing.cargo_calls() == 0);

        let key = "demo@0.1.0";
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &path,
            crate::state::rebuild::StateRebuildOptions::new(retained.registry.clone()),
        )?;
        assert_eq!(rebuilt.packages[key].state, retained.packages[key].state);
        assert_eq!(rebuilt.packages[key].attempts, 1);
        assert_eq!(rebuilt.attempt_history, retained.attempt_history);
        assert_eq!(
            retained.attempt_history[0].error_class,
            Some(ErrorClass::Retryable)
        );

        let mut opts = missing.opts.clone();
        opts.max_attempts = 2;
        missing.registry.respond_with(&[404, 200]);
        missing.with_cargo("0", || {
            run_resume(&missing.ws, &opts, &mut CollectingReporter::default())
        })?;
        ensure!(missing.cargo_calls() == 1);
        let complete = missing.retained()?;
        assert_eq!(complete.packages[key].state, PackageState::Published);
        assert_eq!(
            complete
                .attempt_history
                .iter()
                .map(|detail| detail.attempt)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &path,
            crate::state::rebuild::StateRebuildOptions::new(complete.registry.clone()),
        )?;
        assert_eq!(rebuilt.packages[key].state, complete.packages[key].state);
        assert_eq!(rebuilt.attempt_history, complete.attempt_history);

        // The same retained attempt prefix can also be left by an invocation
        // that actually launched. A failed output append cannot prove no launch.
        let uncertain = Fixture::new(parallel, false);
        let mut opts = uncertain.opts.clone();
        opts.max_attempts = 2;
        let fault = crate::state::file_sync_test_support::observe_event_append(
            &uncertain.state_dir,
            |path| {
                let log = events::EventLog::read_from_file(path).map_err(std::io::Error::other)?;
                if log.all_events().last().is_some_and(|event| {
                    matches!(event.event_type, EventType::PackageAttempted { .. })
                }) {
                    return Err(std::io::Error::other("withhold Cargo completion evidence"));
                }
                Ok(())
            },
        );
        let error = uncertain
            .with_cargo("0", || {
                run_publish(&uncertain.ws, &opts, &mut CollectingReporter::default())
            })
            .expect_err("post-launch completion must be withheld");
        drop(fault);
        ensure!(format!("{error:#}").contains("withhold Cargo completion evidence"));
        ensure!(uncertain.cargo_calls() == 1 && uncertain.registry.hits() == 1);
        let log = events::EventLog::read_from_file(&events::events_path(&uncertain.state_dir))?;
        ensure!(log.all_events().last().is_some_and(|event| matches!(
            event.event_type,
            EventType::PackageAttempted { attempt: 1, .. }
        )));
        let before = uncertain.snapshot()?;
        let error = invoke(&uncertain, Entry::Resume, &opts, &mut uncertain.retained()?)
            .expect_err("an unresolved launched attempt must refuse resume");
        ensure!(is_publish_admission_error(&error), "{error:#}");
        ensure!(format!("{error:#}").contains("unresolved Cargo upload"));
        ensure!(uncertain.cargo_calls() == 1 && uncertain.registry.hits() == 1);
        ensure!(uncertain.snapshot()? == before);
    }
    Ok(())
}

#[cfg(windows)]
#[test]
#[serial]
fn invalid_windows_cargo_image_can_resume_after_fixing_the_executable() -> Result<()> {
    const ERROR_BAD_EXE_FORMAT: i32 = 193;
    const ERROR_EXE_MACHINE_TYPE_MISMATCH: i32 = 216;
    let key = "demo@0.1.0";
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let invalid = fixture.bin.join("invalid-cargo.exe");
        fs::write(&invalid, b"not a Windows executable")?;
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;

        let mut vars = fake_program_env_vars(&fixture.bin);
        vars.retain(|(name, _)| *name != "SHIPPER_CARGO_BIN");
        vars.extend([
            (
                "SHIPPER_CARGO_BIN",
                Some(invalid.to_string_lossy().into_owned()),
            ),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(fixture.cargo_log.to_string_lossy().into_owned()),
            ),
        ]);
        let error = temp_env::with_vars(vars, || {
            run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
        })
        .expect_err("the Windows loader must reject the invalid image");
        ensure!(
            error.is::<crate::ops::process::CommandNotLaunched>(),
            "{error:#}"
        );
        let image_error = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error);
        ensure!(
            matches!(
                image_error,
                Some(ERROR_BAD_EXE_FORMAT | ERROR_EXE_MACHINE_TYPE_MISMATCH)
            ),
            "{error:#}"
        );
        ensure!(fixture.cargo_calls() == 0 && fixture.registry.hits() == 1);

        let path = events::events_path(&fixture.state_dir);
        let log = events::EventLog::read_from_file(&path)?;
        ensure!(log.all_events().iter().any(|event| matches!(
            event.event_type,
            EventType::PackageNotLaunched { attempt: 1, .. }
        )));
        ensure!(log.all_events().iter().all(|event| !matches!(
            event.event_type,
            EventType::PackageOutput { .. } | EventType::PackageCleanupIncomplete { .. }
        )));
        let retained = fixture.retained()?;
        assert_eq!(retained.packages[key].attempts, 1);
        assert!(matches!(
            retained.packages[key].state,
            PackageState::Failed {
                class: ErrorClass::Retryable,
                ..
            }
        ));

        // Restore the valid fake Cargo override, then resume the same run
        // under its original two-attempt budget.
        fixture.registry.respond_with(&[404, 200]);
        let receipt = fixture.with_cargo("0", || {
            run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
        })?;
        assert_eq!(receipt.packages[0].state, PackageState::Published);
        assert_eq!(receipt.packages[0].attempts, 2);
        assert_eq!(fixture.cargo_calls(), 1);

        let complete = fixture.retained()?;
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &path,
            crate::state::rebuild::StateRebuildOptions::new(complete.registry.clone()),
        )?;
        assert_eq!(rebuilt.packages[key].state, complete.packages[key].state);
        assert_eq!(rebuilt.attempt_history, complete.attempt_history);

        fixture.with_cargo("0", || {
            run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
        })?;
        assert_eq!(fixture.cargo_calls(), 1);
        assert_eq!(
            fixture.retained()?.attempt_history,
            complete.attempt_history
        );
        eprintln!(
            "parallel={parallel} loader_error={image_error:?} fake_uploads={}",
            fixture.cargo_calls()
        );
    }
    Ok(())
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[test]
#[serial]
fn repeated_no_launch_requires_its_matching_attempt_projection() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 3;
        let cargo = fake_cargo_path(&fixture.bin);
        let disabled = cargo.with_extension("disabled");
        fs::rename(&cargo, &disabled)?;
        fixture
            .with_cargo("0", || {
                run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
            })
            .expect_err("the first missing-Cargo attempt records a retryable failure");
        let first = fixture.retained()?;

        let fault = crate::state::file_sync_test_support::observe(&fixture.state_dir, |path| {
            if path.file_name().and_then(|name| name.to_str()) == Some("state.tmp") {
                let projection: ExecutionState =
                    serde_json::from_slice(&fs::read(path)?).map_err(std::io::Error::other)?;
                if projection.attempt_history.len() == 2 {
                    return Err(std::io::Error::other(
                        "withhold the second no-launch projection",
                    ));
                }
            }
            Ok(())
        });
        let error = fixture
            .with_cargo("0", || {
                run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
            })
            .expect_err("the second no-launch event must fail at the state write");
        drop(fault);
        ensure!(format!("{error:#}").contains("withhold the second no-launch projection"));
        let stale = fixture.retained()?;
        assert_eq!(stale.packages["demo@0.1.0"].attempts, 2);
        assert_eq!(
            stale.packages["demo@0.1.0"].state,
            first.packages["demo@0.1.0"].state
        );
        assert_eq!(stale.attempt_history, first.attempt_history);
        let log = events::EventLog::read_from_file(&events::events_path(&fixture.state_dir))?;
        ensure!(log.all_events().last().is_some_and(|event| matches!(
            event.event_type,
            EventType::PackageNotLaunched { attempt: 2, .. }
        )));

        fs::rename(disabled, cargo)?;
        let before = fixture.snapshot()?;
        let hits = fixture.registry.hits();
        let error = invoke(&fixture, Entry::Resume, &opts, &mut stale.clone())
            .expect_err("an earlier attempt's retryable projection cannot stand in for this one");
        ensure!(is_publish_admission_error(&error), "{error:#}");
        ensure!(format!("{error:#}").contains("state.json lacks the matching attempt"));
        ensure!(
            fixture.snapshot()? == before
                && fixture.cargo_calls() == 0
                && fixture.registry.hits() == hits
        );
    }
    Ok(())
}

#[test]
#[serial]
fn retry_backoff_checkpoint_replays_the_live_projection() -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};

    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        fixture.registry.respond_with(&[404, 404]);
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        let inspected = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&inspected);
        let event_path = events::events_path(&fixture.state_dir);
        let _fault =
            crate::state::file_sync_test_support::observe(&fixture.state_dir, move |path| {
                if path.file_name().and_then(|name| name.to_str()) != Some("state.tmp") {
                    return Ok(());
                }
                let projection: ExecutionState =
                    serde_json::from_slice(&fs::read(path)?).map_err(std::io::Error::other)?;
                let Some(detail) = projection
                    .attempt_history
                    .last()
                    .filter(|detail| detail.next_attempt_at.is_some())
                else {
                    return Ok(());
                };
                let rebuilt = crate::state::rebuild::rebuild_state_from_events(
                    &event_path,
                    crate::state::rebuild::StateRebuildOptions::new(projection.registry.clone()),
                )
                .map_err(std::io::Error::other)?;
                assert_eq!(
                    projection.packages["demo@0.1.0"].state,
                    PackageState::Pending
                );
                assert_eq!(rebuilt.packages["demo@0.1.0"].state, PackageState::Pending);
                assert_eq!(
                    rebuilt.attempt_history.as_slice(),
                    std::slice::from_ref(detail)
                );
                assert_eq!(detail.error_class, Some(ErrorClass::Retryable));

                // A journal written before the upload checkpoint existed
                // retains its historical projection and retry metadata.
                let log =
                    events::EventLog::read_from_file(&event_path).map_err(std::io::Error::other)?;
                let mut legacy_log = events::EventLog::new();
                for event in log.all_events() {
                    if !matches!(event.event_type, EventType::PackageUploadUnconfirmed { .. }) {
                        legacy_log.record(event.clone());
                    }
                }
                let legacy_path = path.with_file_name("legacy-backoff.jsonl");
                legacy_log
                    .write_to_file(&legacy_path)
                    .map_err(std::io::Error::other)?;
                let legacy = crate::state::rebuild::rebuild_state_from_events(
                    &legacy_path,
                    crate::state::rebuild::StateRebuildOptions::new(projection.registry.clone()),
                )
                .map_err(std::io::Error::other)?;
                assert!(matches!(
                    legacy.packages["demo@0.1.0"].state,
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        ..
                    }
                ));
                assert_eq!(legacy.attempt_history, rebuilt.attempt_history);
                observed.store(true, Ordering::Release);
                Err(std::io::Error::other(
                    "stop at the witnessed backoff boundary",
                ))
            });
        let result = with_test_env(
            &fixture.bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("42".into())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("connection reset by peer".into()),
                ),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(fixture.cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || run_publish(&fixture.ws, &opts, &mut CollectingReporter::default()),
        );
        let error = result.expect_err("the checkpoint observer must stop the run");
        ensure!(format!("{error:#}").contains("stop at the witnessed backoff boundary"));
        ensure!(inspected.load(Ordering::Acquire));
        ensure!(fixture.cargo_calls() == 1 && fixture.registry.hits() == 2);
    }
    Ok(())
}

#[test]
#[serial]
fn durable_upload_outcomes_refuse_a_stale_projection() -> Result<()> {
    let mut failures = Vec::new();
    for parallel in [false, true] {
        for published in [false, true] {
            for response in [404, 503] {
                let result = (|| -> Result<()> {
                    let fixture = Fixture::new(parallel, false);
                    let mut opts = fixture.opts.clone();
                    opts.max_attempts = 3;
                    if published {
                        fixture.registry.respond_with(&[404, 200, response]);
                    } else {
                        fixture.registry.respond_with(&[404, response]);
                    }
                    let fault = crate::state::file_sync_test_support::observe(
                        &fixture.state_dir,
                        move |path| {
                            if path.file_name().and_then(|name| name.to_str()) == Some("state.tmp")
                            {
                                let snapshot: ExecutionState =
                                    serde_json::from_slice(&fs::read(path)?)
                                        .map_err(std::io::Error::other)?;
                                let expected = if published {
                                    PackageState::Published
                                } else {
                                    PackageState::Uploaded
                                };
                                if snapshot.packages["demo@0.1.0"].state == expected {
                                    return Err(std::io::Error::other("failed outcome snapshot"));
                                }
                            }
                            Ok(())
                        },
                    );
                    let error = fixture
                        .with_cargo(if published { "42" } else { "0" }, || {
                            run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
                        })
                        .expect_err("the outcome snapshot must fail");
                    ensure!(
                        format!("{error:#}").contains("failed outcome snapshot"),
                        "{error:#}"
                    );
                    drop(fault);
                    let disk = fixture.retained()?;
                    let progress = &disk.packages["demo@0.1.0"];
                    ensure!(
                        progress.attempts == 1
                            && disk.attempt_history.len() == usize::from(published)
                    );
                    ensure!(if published {
                        matches!(progress.state, PackageState::Ambiguous { .. })
                    } else {
                        progress.state == PackageState::Pending
                    });
                    let journal = events::events_path(&fixture.state_dir);
                    let rebuilt = crate::state::rebuild::rebuild_state_from_events(
                        &journal,
                        crate::state::rebuild::StateRebuildOptions::new(disk.registry.clone()),
                    )?;
                    let expected = if published {
                        PackageState::Published
                    } else {
                        PackageState::Uploaded
                    };
                    ensure!(rebuilt.packages["demo@0.1.0"].state == expected);
                    ensure!(
                        rebuilt.packages["demo@0.1.0"].attempts == 1
                            && rebuilt.attempt_history.len() == 1
                    );
                    ensure!(rebuilt.attempt_history[0].attempt == 1);
                    ensure!(fixture.cargo_calls() == 1);
                    let hits = fixture.registry.hits();
                    let before = fixture.snapshot()?;
                    for entry in [Entry::Resume, Entry::Publish, Entry::DirectParallel] {
                        let result = invoke(&fixture, entry, &opts, &mut disk.clone());
                        ensure!(fixture.cargo_calls() == 1, "{entry:?} sent a second upload");
                        let error = result.expect_err("stale upload projection must refuse");
                        ensure!(is_publish_admission_error(&error), "{error:#}");
                        ensure!(fixture.registry.hits() == hits && fixture.snapshot()? == before);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    failures.push(format!(
                        "parallel={parallel} published={published} registry={response}: {error:#}"
                    ));
                }
            }
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
#[serial]
fn unavailable_post_cargo_visibility_preserves_hold_until_registry_confirmation() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        fixture.registry.respond_with(&[404, 503]);
        let first = with_test_env(
            &fixture.bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("42".into())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("connection reset by peer".into()),
                ),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(fixture.cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || run_publish(&fixture.ws, &opts, &mut CollectingReporter::default()),
        )
        .expect_err("unavailable post-Cargo registry evidence must stop publishing");
        ensure!(
            format!("{first:#}")
                .contains("failed to verify registry visibility after cargo failure"),
            "parallel={parallel}: {first:#}"
        );
        ensure!(fixture.registry.hits() == 2 && fixture.cargo_calls() == 1);

        let key = "demo@0.1.0";
        let path = events::events_path(&fixture.state_dir);
        let original_bytes = fs::read(&path)?;
        let original_log = events::EventLog::read_from_file(&path)?;
        let original = fixture.retained()?;
        let replay = || {
            crate::state::rebuild::rebuild_state_from_events(
                &path,
                crate::state::rebuild::StateRebuildOptions::new(original.registry.clone()),
            )
        };
        let original_replay = replay()?;

        fixture.registry.respond_with(&[503]);
        let error = fixture
            .with_cargo("71", || {
                run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
            })
            .expect_err("resume must not retry while registry evidence remains unavailable");
        ensure!(
            fixture.cargo_calls() == 1,
            "parallel={parallel}: resume launched Cargo after registry failure"
        );
        ensure!(fixture.registry.hits() == 3);
        ensure!(
            matches!(
                classify_publish_stop(&error),
                Some(PublishStopClassification::StillUnknown { .. })
            ),
            "parallel={parallel}: {error:#}"
        );
        assert!(matches!(
            original.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
        assert_eq!(
            original_replay.packages[key].state,
            original.packages[key].state
        );
        assert_eq!(original_replay.packages[key].attempts, 1);
        assert_eq!(original_replay.attempt_history, original.attempt_history);
        let [detail] = original.attempt_history.as_slice() else {
            anyhow::bail!("expected one durable Cargo attempt");
        };
        assert_eq!(detail.attempt, 1);
        assert_eq!(detail.error_class, Some(ErrorClass::Retryable));
        assert!(detail.next_attempt_at.is_none());
        ensure!(original_log.all_events().iter().any(|event| matches!(
            event.event_type,
            EventType::PackageUploadUnconfirmed {
                class: ErrorClass::Retryable,
                ..
            }
        )));
        ensure!(original_log.all_events().iter().all(|event| !matches!(
            event.event_type,
            EventType::PackageFailed { .. } | EventType::PackagePublished { .. }
        )));

        let held = fixture.retained()?;
        assert!(matches!(
            held.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
        assert_eq!(held.attempt_history, original.attempt_history);
        assert_eq!(replay()?.packages[key].state, held.packages[key].state);
        assert_eq!(replay()?.attempt_history, held.attempt_history);

        fixture.registry.respond_with(&[200]);
        let receipt = fixture.with_cargo("71", || {
            run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
        })?;
        ensure!(fixture.registry.hits() == 4 && fixture.cargo_calls() == 1);
        assert_eq!(receipt.execution_result, ExecutionResult::Success);
        assert_eq!(receipt.packages[0].state, PackageState::Published);
        assert_eq!(receipt.packages[0].attempts, 1);
        let completed = fixture.retained()?;
        assert_eq!(completed.attempt_history, original.attempt_history);
        assert_eq!(replay()?.attempt_history, completed.attempt_history);
        assert_eq!(replay()?.packages[key].state, PackageState::Published);
        assert!(fs::read(&path)?.starts_with(&original_bytes));
    }
    Ok(())
}

#[test]
#[serial]
fn final_visibility_start_keeps_an_unconfirmed_upload_in_replay_and_resume() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        fixture.registry.respond_with(&[404, 404]);
        let events_path = events::events_path(&fixture.state_dir);
        let observed_path = events_path.clone();
        let fault =
            crate::state::file_sync_test_support::observe(&fixture.state_dir, move |path| {
                if path != observed_path {
                    return Ok(());
                }
                let log = events::EventLog::read_from_file(path).map_err(std::io::Error::other)?;
                if log.all_events().last().is_some_and(|event| {
                    matches!(event.event_type, EventType::ReadinessStarted { .. })
                }) {
                    return Err(std::io::Error::other(
                        "stop before the final visibility result",
                    ));
                }
                Ok(())
            });
        let result = with_test_env(
            &fixture.bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("42".into())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("connection reset by peer".into()),
                ),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(fixture.cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || {
                run_publish(
                    &fixture.ws,
                    &fixture.opts,
                    &mut CollectingReporter::default(),
                )
            },
        );
        drop(fault);
        let error =
            result.expect_err("the observer must interrupt before the final registry result");
        ensure!(format!("{error:#}").contains("stop before the final visibility result"));
        ensure!(fixture.registry.hits() == 2 && fixture.cargo_calls() == 1);

        let disk = fixture.retained()?;
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &events_path,
            crate::state::rebuild::StateRebuildOptions::new(disk.registry.clone()),
        )?;
        let key = "demo@0.1.0";
        assert!(matches!(
            disk.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
        assert_eq!(rebuilt.packages[key].state, disk.packages[key].state);
        assert_eq!(rebuilt.packages[key].attempts, disk.packages[key].attempts);
        assert_eq!(rebuilt.attempt_history, disk.attempt_history);
        assert_eq!(
            disk.attempt_history[0].error_class,
            Some(ErrorClass::Retryable)
        );

        fixture.registry.respond_with(&[503]);
        let resume = fixture.with_cargo("71", || {
            run_resume(
                &fixture.ws,
                &fixture.opts,
                &mut CollectingReporter::default(),
            )
        });
        ensure!(resume.is_err() && fixture.registry.hits() == 3 && fixture.cargo_calls() == 1);
        let resumed = fixture.retained()?;
        ensure!(matches!(
            resumed.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
        ensure!(resumed.attempt_history == disk.attempt_history);
    }
    Ok(())
}

#[test]
#[serial]
fn legacy_v04_readiness_evidence_controls_resume() -> Result<()> {
    use crate::types::{AttemptDetail, ReadinessMethod};

    // v0.4.0 (b3ebe6623) saved a successful Cargo attempt and Uploaded
    // before recording readiness. Exhaustion saved Failed(Ambiguous)
    // and recorded PackageFailed without changing the attempt detail.

    let exhausted_state = PackageState::Failed {
        class: ErrorClass::Ambiguous,
        message: "published locally, but version not observed on registry within timeout".into(),
    };
    for parallel in [false, true] {
        for phase in [
            exhausted_state.clone(),
            PackageState::Uploaded,
            PackageState::Published,
        ] {
            let terminal = phase == PackageState::Published;
            let exhausted = phase == exhausted_state;
            let fixture = Fixture::new(parallel, false);
            let mut opts = fixture.opts.clone();
            opts.max_attempts = 1;
            opts.readiness.enabled = true;
            let mut disk = init_state(&fixture.ws, &fixture.state_dir)?;
            let key = "demo@0.1.0";
            let at = Utc::now();
            let ended = at + chrono::Duration::milliseconds(1);
            let output = ended + chrono::Duration::milliseconds(1);
            let readiness = output + chrono::Duration::milliseconds(1);
            let progress = disk.packages.get_mut(key).context("legacy package")?;
            progress.attempts = 1;
            progress.state = phase.clone();
            disk.attempt_history.push(AttemptDetail {
                package: "demo".into(),
                version: "0.1.0".into(),
                attempt: 1,
                max_attempts: 1,
                started_at: at,
                ended_at: ended,
                error_class: None,
                next_attempt_at: None,
                redacted_message: None,
            });
            state::save_state(&fixture.state_dir, &disk)?;
            let path = events::events_path(&fixture.state_dir);
            let mut log = events::EventLog::new();
            log.record(PublishEvent {
                timestamp: at,
                event_type: EventType::PlanCreated {
                    plan_id: disk.plan_id.clone(),
                    package_count: 1,
                },
                package: "all".into(),
            });
            for (timestamp, event_type) in [
                (
                    at,
                    EventType::PackageStarted {
                        name: "demo".into(),
                        version: "0.1.0".into(),
                    },
                ),
                (
                    at,
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".into(),
                        max_attempts: 0,
                    },
                ),
                (
                    output,
                    EventType::PackageOutput {
                        stdout_tail: "Uploading demo".into(),
                        stderr_tail: String::new(),
                    },
                ),
                (
                    readiness,
                    EventType::ReadinessStarted {
                        method: ReadinessMethod::Api,
                    },
                ),
            ] {
                log.record(PublishEvent {
                    timestamp,
                    event_type,
                    package: key.into(),
                });
            }
            let final_at = readiness + chrono::Duration::milliseconds(1);
            if terminal {
                log.record(PublishEvent {
                    timestamp: final_at,
                    event_type: EventType::PackagePublished { duration_ms: 1 },
                    package: key.into(),
                });
            } else if exhausted {
                for (timestamp, event_type) in [
                    (final_at, EventType::ReadinessTimeout { max_wait_ms: 0 }),
                    (
                        final_at + chrono::Duration::milliseconds(1),
                        EventType::PackageFailed {
                            class: ErrorClass::Ambiguous,
                            message: "published locally, but version not observed on registry within timeout".into(),
                        },
                    ),
                ] {
                    log.record(PublishEvent { timestamp, event_type, package: key.into() });
                }
            }
            log.write_to_file(&path)?;
            ensure!(!fs::read_to_string(&path)?.contains("max_attempts"));
            let replay = crate::state::rebuild::rebuild_state_from_events(
                &path,
                crate::state::rebuild::StateRebuildOptions::new(disk.registry.clone()),
            )?;
            ensure!(
                replay.packages[key].state == disk.packages[key].state,
                "legacy phase lost"
            );
            ensure!(replay.packages[key].attempts == 1 && replay.attempt_history.len() == 1);
            let mut expected_detail = disk.attempt_history[0].clone();
            expected_detail.ended_at = readiness;
            assert_eq!(replay.attempt_history, [expected_detail.clone()]);
            if terminal {
                fixture.with_cargo("71", || {
                    run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
                })?;
                ensure!(fixture.cargo_calls() == 0 && fixture.registry.hits() == 0);
                ensure!(fixture.retained()?.attempt_history == disk.attempt_history);
                continue;
            }
            let mut stale = disk.clone();
            stale.packages.get_mut(key).unwrap().state = PackageState::Pending;
            stale.attempt_history.clear();
            state::save_state(&fixture.state_dir, &stale)?;
            let before = fixture.snapshot()?;
            let error = invoke(&fixture, Entry::Resume, &opts, &mut stale)
                .expect_err("the historical readiness witness must block a stale Pending snapshot");
            ensure!(is_publish_admission_error(&error), "{error:#}");
            ensure!(
                fixture.cargo_calls() == 0
                    && fixture.registry.hits() == 0
                    && fixture.snapshot()? == before
            );
            state::save_state(&fixture.state_dir, &disk)?;
            for response in [404, 503] {
                fixture.registry.respond_with(&[response]);
                let hits = fixture.registry.hits();
                let result = fixture.with_cargo("71", || {
                    run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
                });
                ensure!(result.is_err());
                ensure!(fixture.registry.hits() > hits && fixture.cargo_calls() == 0);
                let retained = fixture.retained()?;
                ensure!(retained.packages[key].state == phase);
                ensure!(retained.attempt_history == disk.attempt_history);
                let replay = crate::state::rebuild::rebuild_state_from_events(
                    &path,
                    crate::state::rebuild::StateRebuildOptions::new(disk.registry.clone()),
                )?;
                assert_eq!(replay.packages[key].state, phase);
                assert_eq!(replay.attempt_history, [expected_detail.clone()]);
            }
            fixture.registry.respond_with(&[200]);
            fixture.with_cargo("71", || {
                run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
            })?;
            let published = fixture.retained()?;
            assert_eq!(published.packages[key].state, PackageState::Published);
            assert_eq!(published.attempt_history, disk.attempt_history);
            ensure!(fixture.cargo_calls() == 0);
        }
    }
    Ok(())
}

#[test]
#[serial]
fn accepted_upload_survives_visibility_failure_and_resumes_verification() -> Result<()> {
    for parallel in [false, true] {
        let fixture = Fixture::new(parallel, false);
        let mut opts = fixture.opts.clone();
        opts.max_attempts = 2;
        opts.readiness.enabled = true;
        let first = fixture.with_cargo("0", || {
            run_publish(&fixture.ws, &opts, &mut CollectingReporter::default())
        });
        ensure!(first.is_err() && fixture.cargo_calls() == 1);
        let disk = fixture.retained()?;
        let path = events::events_path(&fixture.state_dir);
        let key = "demo@0.1.0";
        ensure!(disk.packages[key].state == PackageState::Uploaded);
        ensure!(disk.packages[key].attempts == 1 && disk.attempt_history.len() == 1);
        ensure!(
            disk.attempt_history[0].attempt == 1 && disk.attempt_history[0].error_class.is_none()
        );
        let journal = events::EventLog::read_from_file(&path)?;
        let accepted = journal
            .all_events()
            .iter()
            .find(|event| matches!(event.event_type, EventType::PackageUploaded))
            .context("accepted Cargo upload event")?;
        ensure!(disk.attempt_history[0].ended_at == accepted.timestamp);

        for response in [404, 503] {
            fixture.registry.respond_with(&[response]);
            let hits = fixture.registry.hits();
            let result = fixture.with_cargo("71", || {
                run_resume(&fixture.ws, &opts, &mut CollectingReporter::default())
            });
            ensure!(result.is_err() && fixture.registry.hits() > hits);
            ensure!(fixture.cargo_calls() == 1);
            let retained = fixture.retained()?;
            let rebuilt = crate::state::rebuild::rebuild_state_from_events(
                &path,
                crate::state::rebuild::StateRebuildOptions::new(retained.registry.clone()),
            )?;
            ensure!(retained.packages[key].state == PackageState::Uploaded);
            ensure!(rebuilt.packages[key].state == retained.packages[key].state);
            ensure!(retained.packages[key].attempts == 1 && rebuilt.packages[key].attempts == 1);
            ensure!(retained.attempt_history == disk.attempt_history);
            ensure!(rebuilt.attempt_history == retained.attempt_history);
        }
    }
    Ok(())
}
