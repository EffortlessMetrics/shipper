//! A real non-timeout Cargo exit with lost diagnostics cannot authorize retries.

use std::io::Write;

use super::timeout_reconciliation::ProbeRegistry;
use super::*;

const SECRET: &str = "bounded-capture-secret-sentinel";
const CHILD: &str = "engine::tests::capture_reconciliation::oversized_publish_child";

#[test]
#[serial]
#[ignore = "fake Cargo subprocess invoked by capture reconciliation tests"]
fn oversized_publish_child() {
    let root = std::env::current_dir().unwrap();
    let calls_path = root.join("capture-cargo-calls");
    let previous = fs::read_to_string(&calls_path)
        .unwrap_or_default()
        .lines()
        .count();
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(calls_path)
        .unwrap();
    writeln!(calls, "publish").unwrap();
    calls.flush().unwrap();
    if previous > 0 {
        std::process::exit(0);
    }
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    write!(out, "Authorization: Bearer ").unwrap();
    for _ in 0..32 {
        out.write_all(&[b'x'; 8192]).unwrap();
    }
    // A byte suffix or resumed capture after the oversized line would expose
    // this continuation. Both streams must be omitted after capture loss.
    write!(out, "{SECRET}\r").unwrap();
    out.write_all(b"\n continuation-").unwrap();
    writeln!(out, "{SECRET}").unwrap();
    writeln!(err, "Token={SECRET}\npermission denied").unwrap();
    out.flush().unwrap();
    err.flush().unwrap();
    std::process::exit(if root.join("capture-success").exists() {
        0
    } else {
        42
    });
}

fn capture_fixture(root: &Path) -> Vec<(&'static str, Option<String>)> {
    let bin = root.join("bin");
    write_fake_tools(&bin);
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = fake_cargo_path(&bin);
        fs::write(&path, format!("#!/bin/sh\nexec \"$SHIPPER_CAPTURE_FIXTURE_EXE\" --exact {CHILD} --ignored --nocapture\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    fs::write(fake_cargo_path(&bin), format!("@echo off\r\n\"%SHIPPER_CAPTURE_FIXTURE_EXE%\" --exact {CHILD} --ignored --nocapture\r\nexit /b\r\n")).unwrap();
    let mut env = fake_program_env_vars(&bin);
    env.push((
        "SHIPPER_CAPTURE_FIXTURE_EXE",
        Some(std::env::current_exe().unwrap().display().to_string()),
    ));
    env
}

fn calls(root: &Path) -> usize {
    fs::read_to_string(root.join("capture-cargo-calls"))
        .unwrap()
        .lines()
        .count()
}

#[test]
#[serial]
fn incomplete_failed_capture_reconciles_before_any_retry_in_both_modes() {
    for parallel in [false, true] {
        for observed in [200, 404, 503] {
            let td = tempdir().unwrap();
            let env = capture_fixture(td.path());
            let statuses = match observed {
                200 => vec![404, 200],
                404 => vec![404, 404, 200],
                _ => vec![404, 503, 503],
            };
            let server = ProbeRegistry::start(statuses.clone());
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = parallel;
            opts.parallel.per_package_timeout = Duration::from_secs(10);
            opts.readiness.max_total_wait = Duration::ZERO;
            temp_env::with_vars(env, || {
                let mut reporter = CollectingReporter::default();
                let result = run_publish(&ws, &opts, &mut reporter);
                if observed == 503 {
                    assert!(
                        result
                            .unwrap_err()
                            .downcast_ref::<PublishStillUnknownError>()
                            .is_some()
                    );
                    assert!(
                        run_resume(&ws, &opts, &mut reporter)
                            .unwrap_err()
                            .downcast_ref::<PublishStillUnknownError>()
                            .is_some()
                    );
                    assert!(!state::receipt_path(&state_dir).exists());
                } else {
                    let receipt = result.unwrap();
                    assert!(matches!(receipt.execution_result, ExecutionResult::Success));
                    assert_eq!(receipt.packages[0].evidence.attempts[0].exit_code, 42);
                }
                let expected_calls = if observed == 404 { 2 } else { 1 };
                assert_eq!(calls(td.path()), expected_calls);
                assert_eq!(*server.seen.lock().unwrap(), statuses);
                let retained = state::load_state(&state_dir).unwrap().unwrap();
                assert_eq!(
                    retained.packages["demo@0.1.0"].attempts,
                    expected_calls as u32
                );
                assert_eq!(
                    retained.attempt_history[0].error_class,
                    Some(ErrorClass::Ambiguous)
                );
                assert!(
                    retained.attempt_history[0]
                        .redacted_message
                        .as_ref()
                        .unwrap()
                        .contains("capture was incomplete")
                );
                let event_path = events::events_path(&state_dir);
                let log = events::EventLog::read_from_file(&event_path).unwrap();
                let timeline = log.all_events();
                let outcomes: Vec<_> = timeline
                    .iter()
                    .filter_map(|event| match &event.event_type {
                        EventType::PublishReconciled { outcome } => Some(outcome),
                        _ => None,
                    })
                    .collect();
                let report: crate::types::ReconciliationReport = serde_json::from_slice(
                    &fs::read(state::reconciliation_path(&state_dir)).unwrap(),
                )
                .unwrap();
                assert_eq!(report.records.len(), outcomes.len());
                for (record, outcome) in report.records.iter().zip(&outcomes) {
                    assert_eq!(
                        serde_json::to_value(&record.outcome).unwrap(),
                        serde_json::to_value(outcome).unwrap()
                    );
                }
                match observed {
                    200 => assert!(matches!(
                        outcomes.as_slice(),
                        [ReconciliationOutcome::Published { attempts: 1, .. }]
                    )),
                    404 => {
                        assert!(matches!(
                            outcomes.as_slice(),
                            [ReconciliationOutcome::NotPublished { attempts: 1, .. }]
                        ));
                        let absent = timeline
                            .iter()
                            .position(|event| {
                                matches!(
                                    event.event_type,
                                    EventType::PublishReconciled {
                                        outcome: ReconciliationOutcome::NotPublished { .. }
                                    }
                                )
                            })
                            .unwrap();
                        let retry = timeline
                            .iter()
                            .position(|event| {
                                matches!(
                                    event.event_type,
                                    EventType::PackageAttempted { attempt: 2, .. }
                                )
                            })
                            .unwrap();
                        assert!(absent < retry);
                    }
                    _ => {
                        assert_eq!(outcomes.len(), 2);
                        assert!(outcomes.iter().all(|outcome| matches!(
                            outcome,
                            ReconciliationOutcome::StillUnknown { attempts: 1, .. }
                        )));
                        assert!(matches!(
                            retained.packages["demo@0.1.0"].state,
                            PackageState::Ambiguous { .. }
                        ));
                    }
                }
                let first_output = timeline
                    .iter()
                    .find_map(|event| match &event.event_type {
                        EventType::PackageOutput {
                            stdout_tail,
                            stderr_tail,
                        } => Some((stdout_tail, stderr_tail)),
                        _ => None,
                    })
                    .unwrap();
                assert!(first_output.0.is_empty());
                assert_eq!(first_output.1, cargo::PUBLISH_CAPTURE_OMITTED);
                let rebuilt = crate::state::rebuild::rebuild_state_from_events(
                    &event_path,
                    crate::state::rebuild::StateRebuildOptions::new(ws.plan.registry.clone())
                        .with_fallback_plan_id(&ws.plan.plan_id),
                )
                .unwrap();
                assert_eq!(
                    rebuilt.packages["demo@0.1.0"].attempts,
                    expected_calls as u32
                );
                assert_eq!(
                    serde_json::to_value(&rebuilt.packages["demo@0.1.0"].state).unwrap(),
                    serde_json::to_value(&retained.packages["demo@0.1.0"].state).unwrap()
                );
                for name in [
                    "state.json",
                    "events.jsonl",
                    "receipt.json",
                    "reconciliation.json",
                ] {
                    if let Ok(text) = fs::read_to_string(state_dir.join(name)) {
                        assert!(!text.contains(SECRET), "secret in {name}");
                    }
                }
                assert!(
                    reporter
                        .infos
                        .iter()
                        .chain(&reporter.warns)
                        .chain(&reporter.errors)
                        .all(|text| !text.contains(SECRET))
                );
            });
        }
    }
}

#[test]
#[serial]
fn successful_incomplete_capture_still_uses_uploaded_and_readiness() {
    for parallel in [false, true] {
        let td = tempdir().unwrap();
        let env = capture_fixture(td.path());
        fs::write(td.path().join("capture-success"), "exit zero").unwrap();
        let server = ProbeRegistry::start(vec![404, 200]);
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let state_dir = td.path().join(".shipper");
        let mut opts = default_opts(state_dir.clone());
        opts.parallel.enabled = parallel;
        temp_env::with_vars(env, || {
            let receipt = run_publish(&ws, &opts, &mut CollectingReporter::default()).unwrap();
            assert!(matches!(receipt.execution_result, ExecutionResult::Success));
            assert_eq!(calls(td.path()), 1);
            assert_eq!(*server.seen.lock().unwrap(), vec![404, 200]);
            assert_eq!(receipt.packages[0].evidence.attempts[0].exit_code, 0);
            let log = events::EventLog::read_from_file(&events::events_path(&state_dir)).unwrap();
            assert!(
                log.all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PackageUploaded))
            );
            assert!(
                !log.all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PublishReconciling { .. }))
            );
        });
    }
}

#[test]
#[serial]
fn rehearsal_stops_with_truthful_incomplete_capture_classification() {
    let td = tempdir().unwrap();
    let env = capture_fixture(td.path());
    let server = ProbeRegistry::start(vec![200]);
    let ws = planned_workspace(td.path(), "http://127.0.0.1:9".into());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.rehearsal_registry = Some("rehearsal".into());
    opts.registries = vec![Registry {
        name: "rehearsal".into(),
        api_base: server.base_url.clone(),
        index_base: Some(server.base_url.clone()),
    }];
    temp_env::with_vars(env, || {
        let mut reporter = CollectingReporter::default();
        let outcome = run_rehearsal(&ws, &opts, &mut reporter).unwrap();
        assert!(!outcome.passed);
        assert_eq!(calls(td.path()), 1);
        assert!(server.seen.lock().unwrap().is_empty());
        let log = events::EventLog::read_from_file(&events::events_path(&state_dir)).unwrap();
        assert!(log.all_events().iter().any(|event| matches!(&event.event_type, EventType::RehearsalPackageFailed { class: ErrorClass::Ambiguous, message, .. } if message.contains("capture was incomplete"))));
        assert!(
            !fs::read_to_string(events::events_path(&state_dir))
                .unwrap()
                .contains(SECRET)
        );
        assert!(reporter.errors.iter().all(|text| !text.contains(SECRET)));
    });
}
