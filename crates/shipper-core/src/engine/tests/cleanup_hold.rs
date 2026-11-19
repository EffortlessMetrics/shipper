//! A local process hold must survive scheduling, persistence faults and recovery.

use super::*;
use crate::ops::process::CleanupFailure;
use crate::state::rebuild::{StateRebuildOptions, rebuild_state_file_from_events};
use crate::types::AttemptDetail;

const KEY: &str = "demo@0.1.0";

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod real_process;

fn assert_cleanup(error: &anyhow::Error) {
    assert!(
        matches!(
            classify_publish_stop(error),
            Some(PublishStopClassification::CleanupIncomplete)
        ),
        "{error:#}"
    );
}

fn hold_event() -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        package: KEY.into(),
        event_type: EventType::PackageCleanupIncomplete {
            message: "owned process cleanup not confirmed".into(),
        },
    }
}

fn rebuild_options(ws: &PlannedWorkspace) -> StateRebuildOptions {
    StateRebuildOptions::new(ws.plan.registry.clone())
        .with_fallback_plan_id(ws.plan.plan_id.clone())
}

#[test]
#[serial]
fn cleanup_hold_in_supplied_parallel_state_blocks_before_disk_or_registry() {
    let td = tempdir().unwrap();
    let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
    let state_dir = td.path().join(".shipper");
    let mut supplied = init_state(&ws, &state_dir).unwrap();
    supplied.packages.get_mut(KEY).unwrap().state = PackageState::CleanupIncomplete {
        message: "in-memory hold after persistence failure".into(),
    };
    fs::remove_file(state::state_path(&state_dir)).unwrap();
    let reg = test_registry_client(ws.plan.registry.clone()).unwrap();
    let error = parallel::run_publish_parallel(
        &ws,
        &default_opts(state_dir.clone()),
        &mut supplied,
        &state_dir,
        &reg,
        &mut CollectingReporter::default(),
    )
    .unwrap_err();
    assert_cleanup(&error);
    assert!(!state::state_path(&state_dir).exists());
    assert!(!events::events_path(&state_dir).exists());
}

#[test]
#[serial]
fn cleanup_admission_does_not_echo_malformed_event_payload() {
    let td = tempdir().unwrap();
    fs::write(
        events::events_path(td.path()),
        "{\"type\":\"package_cleanup_incomplete\",\"message\":\"malformed-event-secret-sentinel\"}\n",
    )
    .unwrap();
    let error = validate_publish_cleanup_safety(td.path()).unwrap_err();
    assert!(is_publish_admission_error(&error));
    assert!(
        classify_publish_stop(&error).is_none(),
        "a loose tag is not a complete hold event"
    );
    assert!(!format!("{error:#}").contains("secret-sentinel"));
}

#[test]
#[serial]
fn cleanup_admission_reports_state_parse_errors_without_echoing_retained_payloads() {
    let td = tempdir().unwrap();
    let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
    let state_dir = td.path().join(".shipper");
    let state = init_state(&ws, &state_dir).unwrap();
    let mut invalid_variant = serde_json::to_value(&state).unwrap();
    invalid_variant["packages"][KEY]["state"]["state"] =
        serde_json::json!("invalid-state-secret-sentinel");
    let state_path = state::state_path(&state_dir);
    for raw in [
        "NOT JSON state-secret-sentinel".to_string(),
        String::new(),
        "{\"state_version\":\"shipper.state.v1\"".to_string(),
        "[1, 2, 3]".to_string(),
        "{\"unexpected\":\"state-secret-sentinel\"}".to_string(),
        serde_json::to_string(&invalid_variant).unwrap(),
    ] {
        fs::write(&state_path, &raw).unwrap();
        let error = validate_publish_cleanup_safety(&state_dir).unwrap_err();
        assert!(is_publish_admission_error(&error));
        assert!(classify_publish_stop(&error).is_none());
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("failed to parse state JSON"),
            "{rendered}"
        );
        assert!(!rendered.contains("secret-sentinel"), "{rendered}");
        assert_eq!(fs::read(&state_path).unwrap(), raw.as_bytes());
        assert!(!events::events_path(&state_dir).exists());
    }

    // A filesystem read failure must not be mislabeled as malformed JSON.
    fs::remove_file(&state_path).unwrap();
    fs::create_dir(&state_path).unwrap();
    let error = validate_publish_cleanup_safety(&state_dir).unwrap_err();
    assert!(is_publish_admission_error(&error));
    let rendered = format!("{error:#}");
    assert!(rendered.contains("cannot inspect retained publication state"));
    assert!(!rendered.contains("failed to parse state JSON"));
    assert!(state_path.is_dir());
}

#[test]
#[serial]
fn cleanup_hold_rehearsal_ownership_precedes_mutation_and_rechecks_retained_hold() {
    let td = tempdir().unwrap();
    let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.rehearsal_registry = Some("rehearsal".into());
    opts.registries.push(Registry {
        name: "rehearsal".into(),
        api_base: "http://127.0.0.1:10".into(),
        index_base: Some("http://127.0.0.1:10".into()),
    });
    let owner = crate::lock::LockFile::acquire(&state_dir, Some(&ws.workspace_root)).unwrap();
    let lock_path = crate::lock::lock_path(&state_dir, Some(&ws.workspace_root));
    let lock_before = fs::read(&lock_path).unwrap();
    for force in [false, true] {
        opts.force = force;
        let error = run_rehearsal(&ws, &opts, &mut CollectingReporter::default()).unwrap_err();
        assert!(is_publish_admission_error(&error), "{error:#}");
        assert!(format!("{error:#}").contains("rehearsal ownership"));
        assert_eq!(fs::read(&lock_path).unwrap(), lock_before);
        assert!(!events::events_path(&state_dir).exists());
        assert!(!state::state_path(&state_dir).exists());
    }
    owner.release().unwrap();
    // A hold written by the preceding owner after the read-only precheck must
    // still refuse the reservation before predecessor metadata is replaced.
    validate_publish_cleanup_safety(&state_dir).unwrap();
    let mut log = events::EventLog::new();
    log.record(hold_event());
    log.write_to_file(&events::events_path(&state_dir)).unwrap();
    let before = fs::read(events::events_path(&state_dir)).unwrap();
    let error = publish::admission::reserve_rehearsal(&ws, &opts, &state_dir)
        .err()
        .unwrap();
    assert_cleanup(&error);
    assert_eq!(fs::read(events::events_path(&state_dir)).unwrap(), before);
    assert!(!state::state_path(&state_dir).exists());
}

#[test]
#[serial]
fn cleanup_hold_is_absorbing_when_rebuilding_later_ordinary_events() {
    let td = tempdir().unwrap();
    let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
    let mut log = events::EventLog::new();
    log.record(hold_event());
    for event_type in [
        EventType::PackageStarted {
            name: "demo".into(),
            version: "0.1.0".into(),
        },
        EventType::PackageAttempted {
            command: "cargo publish".into(),
            attempt: 2,
            max_attempts: 3,
        },
        EventType::PackageUploaded,
        EventType::PublishReconciled {
            outcome: ReconciliationOutcome::Published {
                attempts: 1,
                elapsed_ms: 1,
            },
        },
        EventType::PackagePublished { duration_ms: 1 },
    ] {
        log.record(PublishEvent {
            timestamp: Utc::now(),
            package: KEY.into(),
            event_type,
        });
    }
    log.write_to_file(&events::events_path(td.path())).unwrap();
    let rebuilt = rebuild_state_file_from_events(td.path(), rebuild_options(&ws)).unwrap();
    assert!(matches!(
        rebuilt.packages[KEY].state,
        PackageState::CleanupIncomplete { .. }
    ));
    assert_eq!(
        rebuilt.packages[KEY].attempts, 2,
        "later observations are retained"
    );
    assert_cleanup(&validate_publish_cleanup_safety(td.path()).unwrap_err());
}

#[test]
#[serial]
fn cleanup_hold_persistence_failures_keep_memory_and_any_recorded_evidence_closed() {
    for (fail_event, fail_state) in [(true, false), (false, true), (true, true)] {
        let td = tempdir().unwrap();
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
        let state_dir = td.path().join(".shipper");
        let events_path = events::events_path(&state_dir);
        let mut retained = init_state(&ws, &state_dir).unwrap();
        let mut log = events::EventLog::new();
        transition::commit_attempt(
            &mut retained,
            &state_dir,
            &mut log,
            &events_path,
            KEY,
            1,
            PublishEvent {
                timestamp: Utc::now(),
                package: KEY.into(),
                event_type: EventType::PackageAttempted {
                    command: "cargo publish".into(),
                    attempt: 1,
                    max_attempts: 3,
                },
            },
        )
        .unwrap();
        let event_prefix = fs::read(&events_path).unwrap();
        let state_prefix = fs::read(state::state_path(&state_dir)).unwrap();
        if fail_event {
            fs::remove_file(&events_path).unwrap();
            fs::create_dir(&events_path).unwrap();
        }
        let temporary_state = state::state_path(&state_dir).with_extension("tmp");
        if fail_state {
            fs::create_dir(&temporary_state).unwrap();
        }
        let shared = Arc::new(Mutex::new(retained));
        let shared_log = Arc::new(Mutex::new(log));
        let now = Utc::now();
        let error = cleanup::stop_publish(
            &shared,
            &state_dir,
            &shared_log,
            &events_path,
            KEY,
            cleanup::message(CleanupFailure::InheritedPipe),
            AttemptDetail {
                package: "demo".into(),
                version: "0.1.0".into(),
                attempt: 1,
                max_attempts: 3,
                started_at: now,
                ended_at: now,
                error_class: Some(ErrorClass::Ambiguous),
                next_attempt_at: None,
                redacted_message: Some("incomplete cleanup".into()),
            },
            &cargo::CargoOutput {
                exit_code: 42,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                timed_out: false,
                duration: Duration::ZERO,
            },
        );
        assert_cleanup(&error);
        {
            let mut state = shared.lock().unwrap();
            assert!(matches!(
                state.packages[KEY].state,
                PackageState::CleanupIncomplete { .. }
            ));
            let mut new_log = events::EventLog::new();
            let refusal = transition::commit_attempt(
                &mut state,
                &state_dir,
                &mut new_log,
                &events_path,
                KEY,
                2,
                PublishEvent {
                    timestamp: now,
                    package: KEY.into(),
                    event_type: EventType::PackageAttempted {
                        command: "cargo publish".into(),
                        attempt: 2,
                        max_attempts: 3,
                    },
                },
            )
            .unwrap_err();
            assert_cleanup(&refusal);
            assert_eq!(state.packages[KEY].attempts, 1);
            assert!(
                new_log.all_events().is_empty(),
                "refused before recording an attempt"
            );
            assert_eq!(state.attempt_history.len(), 1);
        }
        if fail_event {
            fs::remove_dir(&events_path).unwrap();
            fs::write(&events_path, &event_prefix).unwrap();
        }
        if fail_state {
            fs::remove_dir(&temporary_state).unwrap();
            assert_eq!(
                fs::read(state::state_path(&state_dir)).unwrap(),
                state_prefix
            );
        }
        if fail_event && fail_state {
            assert!(format!("{error:#}").contains("no safe rerun is established"));
            // No durability claim when both existing storage paths failed.
        } else {
            assert_cleanup(&validate_publish_cleanup_safety(&state_dir).unwrap_err());
            if fail_event {
                let before = fs::read(state::state_path(&state_dir)).unwrap();
                let error =
                    rebuild_state_file_from_events(&state_dir, rebuild_options(&ws)).unwrap_err();
                assert!(format!("{error:#}").contains("cannot rebuild away retained cleanup hold"));
                assert_eq!(fs::read(state::state_path(&state_dir)).unwrap(), before);
                assert_eq!(
                    fs::read(&events_path).unwrap(),
                    event_prefix,
                    "no invented hold event"
                );
            } else {
                let rebuilt =
                    rebuild_state_file_from_events(&state_dir, rebuild_options(&ws)).unwrap();
                assert!(matches!(
                    rebuilt.packages[KEY].state,
                    PackageState::CleanupIncomplete { .. }
                ));
                assert_eq!(rebuilt.attempt_history.len(), 1);
            }
        }
    }
}

#[test]
#[serial]
fn cleanup_hold_admission_covers_both_artifacts_scope_and_all_publish_entries() {
    for evidence in [
        "both",
        "state",
        "event-stale-state",
        "event-missing-state",
        "event-corrupt-state",
        "event-before-corrupt",
        "event-after-corrupt",
    ] {
        let td = tempdir().unwrap();
        let mut ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
        let state_dir = td.path().join(".shipper");
        let mut supplied = init_state(&ws, &state_dir).unwrap();
        if matches!(evidence, "both" | "state") {
            let mut retained = supplied.clone();
            retained.packages.get_mut(KEY).unwrap().state = PackageState::CleanupIncomplete {
                message: "retained hold".into(),
            };
            state::save_state(&state_dir, &retained).unwrap();
        }
        if evidence != "state" {
            let mut log = events::EventLog::new();
            log.record(hold_event());
            log.write_to_file(&events::events_path(&state_dir)).unwrap();
        }
        if matches!(evidence, "event-before-corrupt" | "event-after-corrupt") {
            let path = events::events_path(&state_dir);
            let raw = fs::read(&path).unwrap();
            let malformed = b"malformed-event-secret-sentinel\xff\n";
            let bytes = if evidence == "event-before-corrupt" {
                [raw.as_slice(), malformed].concat()
            } else {
                [malformed, raw.as_slice()].concat()
            };
            fs::write(path, bytes).unwrap();
        }
        if matches!(
            evidence,
            "event-missing-state" | "event-before-corrupt" | "event-after-corrupt"
        ) {
            fs::remove_file(state::state_path(&state_dir)).unwrap();
        } else if evidence == "event-corrupt-state" {
            fs::write(
                state::state_path(&state_dir),
                "corrupt-state-secret-sentinel",
            )
            .unwrap();
        }
        let state_before = fs::read(state::state_path(&state_dir)).ok();
        let events_before = fs::read(events::events_path(&state_dir)).ok();
        let mut opts = default_opts(state_dir.clone());
        opts.max_attempts = 999;
        opts.force = true;
        opts.force_resume = true;
        opts.resume_from = Some("different".into());
        // An omitted package and new plan cannot narrow this run-wide hold.
        ws.plan.plan_id = "different-plan".into();
        ws.plan.packages[0].name = "different".into();
        let reg = test_registry_client(ws.plan.registry.clone()).unwrap();
        for parallel in [false, true] {
            opts.parallel.enabled = parallel;
            let mut reporter = CollectingReporter::default();
            assert_cleanup(&run_publish(&ws, &opts, &mut reporter).err().unwrap());
            assert_cleanup(&run_resume(&ws, &opts, &mut reporter).err().unwrap());
            assert_cleanup(
                &parallel::run_publish_parallel(
                    &ws,
                    &opts,
                    &mut supplied,
                    &state_dir,
                    &reg,
                    &mut reporter,
                )
                .err()
                .unwrap(),
            );
        }
        assert_cleanup(&validate_publish_cleanup_safety(&state_dir).unwrap_err());
        assert_eq!(
            fs::read(state::state_path(&state_dir)).ok(),
            state_before,
            "{evidence}"
        );
        assert_eq!(
            fs::read(events::events_path(&state_dir)).ok(),
            events_before,
            "{evidence}"
        );
        assert!(!state::receipt_path(&state_dir).exists());
    }
}
