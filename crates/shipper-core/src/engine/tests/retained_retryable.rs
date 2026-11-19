use super::*;

fn cargo_invocations(path: &Path) -> usize {
    fs::read_to_string(path)
        .expect("fake Cargo log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

#[test]
#[serial]
fn retained_retryable_visible_resume_preserves_attempts_without_cargo() {
    for parallel in [false, true] {
        for ambiguous_then_absent in [false, true] {
            let td = tempdir().expect("tempdir");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = td.path().join("cargo.log");
            let mut env_vars = fake_program_env_vars(&bin);
            env_vars.extend([
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some(if ambiguous_then_absent {
                        String::new()
                    } else {
                        "connection refused".to_string()
                    }),
                ),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().to_string()),
                ),
            ]);
            temp_env::with_vars(env_vars, || {
                // The first invocation reaches Cargo and genuinely retains a
                // Retryable failure. An ambiguous outcome requires only its
                // conclusive reconciliation; an ordinary retryable error also
                // gets the existing final-chance readiness check.
                let mut statuses = vec![404, 404];
                if !ambiguous_then_absent {
                    statuses.push(404);
                }
                statuses.push(200);
                let expected_requests = statuses.len();
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        statuses
                            .into_iter()
                            .map(|status| (status, "{}".to_string()))
                            .collect(),
                    )]),
                    expected_requests,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let state_dir = td.path().join(".shipper");
                let mut opts = default_opts(state_dir.clone());
                opts.parallel.enabled = parallel;
                opts.max_attempts = 1;
                opts.readiness.max_total_wait = Duration::ZERO;

                run_publish(&ws, &opts, &mut CollectingReporter::default())
                    .expect_err("first invocation must retain a retryable failure");
                let before = state::load_state(&state_dir)
                    .expect("load retained state")
                    .expect("retained state");
                assert!(matches!(
                    before.packages["demo@0.1.0"].state,
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        ..
                    }
                ));
                assert_eq!(before.packages["demo@0.1.0"].attempts, 1);
                assert_eq!(cargo_invocations(&cargo_log), 1);
                let events_path = events::events_path(&state_dir);
                let previous_events = fs::read(&events_path).expect("retained events");

                // Raising the cumulative ceiling admits a controlled-stop
                // resume, but must not force a new upload when already visible.
                opts.max_attempts = 2;
                let receipt = run_resume(&ws, &opts, &mut CollectingReporter::default())
                    .expect("visible retained version completes without another upload");
                assert_eq!(
                    cargo_invocations(&cargo_log),
                    1,
                    "resume must check visibility before dispatching Cargo"
                );
                let after = state::load_state(&state_dir)
                    .expect("load final state")
                    .expect("final state");
                assert_eq!(after.packages["demo@0.1.0"].attempts, 1);
                assert_eq!(
                    serde_json::to_value(&after.attempt_history).expect("final attempt history"),
                    serde_json::to_value(&before.attempt_history).expect("prior attempt history"),
                    "skipping must retain the original attempt details"
                );
                assert!(matches!(
                    &after.packages["demo@0.1.0"].state,
                    PackageState::Skipped { reason } if reason == "already published"
                ));
                assert!(matches!(receipt.execution_result, ExecutionResult::Success));
                assert_eq!(receipt.packages[0].attempts, 1);
                assert!(matches!(
                    &receipt.packages[0].state,
                    PackageState::Skipped { reason } if reason == "already published"
                ));
                assert!(state::receipt_path(&state_dir).exists());
                let final_events = fs::read(&events_path).expect("final events");
                assert!(final_events.starts_with(&previous_events));
                let new_events: Vec<crate::types::PublishEvent> =
                    std::str::from_utf8(&final_events[previous_events.len()..])
                        .expect("new events UTF-8")
                        .lines()
                        .map(|line| serde_json::from_str(line).expect("new event"))
                        .collect();
                assert!(
                    !new_events.iter().any(|event| matches!(
                        event.event_type,
                        EventType::PackageAttempted { .. }
                    ))
                );
                assert_eq!(
                    new_events
                        .iter()
                        .filter(|event| matches!(
                            &event.event_type,
                            EventType::PackageSkipped { reason } if reason == "already published"
                        ))
                        .count(),
                    1
                );
                assert_eq!(
                    server.seen.lock().expect("requests").len(),
                    expected_requests
                );
                server.join();
            });
        }
    }
}

#[test]
#[serial]
fn retained_retryable_unconfirmed_resume_keeps_existing_retry_policy() {
    for parallel in [false, true] {
        for precheck_status in [404, 500] {
            let td = tempdir().expect("tempdir");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = td.path().join("cargo.log");
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![404, 404, 404, precheck_status, 200]
                        .into_iter()
                        .map(|status| (status, "{}".to_string()))
                        .collect(),
                )]),
                5,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = parallel;
            opts.max_attempts = 1;
            opts.readiness.max_total_wait = Duration::ZERO;
            with_test_env(
                &bin,
                vec![
                    ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                    ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                    (
                        "SHIPPER_CARGO_STDERR",
                        Some("connection refused".to_string()),
                    ),
                    (
                        "SHIPPER_CARGO_ARGS_LOG",
                        Some(cargo_log.to_string_lossy().to_string()),
                    ),
                ],
                || {
                    run_publish(&ws, &opts, &mut CollectingReporter::default())
                        .expect_err("retain first retryable failure");
                },
            );
            let before_resume = state::load_state(&state_dir)
                .expect("load retained state")
                .expect("retained state");
            let [original_attempt] = before_resume.attempt_history.as_slice() else {
                panic!("expected a single retained attempt");
            };
            assert_eq!(original_attempt.attempt, 1);
            assert_eq!(original_attempt.error_class, Some(ErrorClass::Retryable));
            assert!(original_attempt.next_attempt_at.is_none());
            let event_path = crate::state::events::events_path(&state_dir);
            let original_events = fs::read(&event_path).expect("retained events");
            opts.max_attempts = 2;
            with_test_env(
                &bin,
                vec![
                    ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                    ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                    ("SHIPPER_CARGO_STDERR", Some(String::new())),
                    (
                        "SHIPPER_CARGO_ARGS_LOG",
                        Some(cargo_log.to_string_lossy().to_string()),
                    ),
                ],
                || {
                    let mut reporter = CollectingReporter::default();
                    let receipt = run_resume(&ws, &opts, &mut reporter)
                        .expect("known retryable work retains its existing retry policy");
                    assert_eq!(receipt.packages[0].attempts, 2);
                    assert!(matches!(receipt.packages[0].state, PackageState::Published));
                    assert_eq!(cargo_invocations(&cargo_log), 2);
                    let live = state::load_state(&state_dir)
                        .expect("load resumed state")
                        .expect("resumed state");
                    let [first, second] = live.attempt_history.as_slice() else {
                        panic!("expected exactly one detail per Cargo attempt");
                    };
                    assert_eq!(first, original_attempt);
                    assert_eq!(second.attempt, 2);
                    assert!(second.error_class.is_none());
                    assert!(
                        fs::read(&event_path)
                            .expect("resumed events")
                            .starts_with(&original_events)
                    );
                    let replay = crate::state::rebuild::rebuild_state_from_events(
                        &event_path,
                        crate::state::rebuild::StateRebuildOptions::new(live.registry.clone()),
                    )
                    .expect("replay resumed events");
                    assert_eq!(replay.attempt_history, live.attempt_history);
                    assert_eq!(
                        reporter.warns.iter().any(|message| {
                            message.contains("existing-version check failed before publish")
                        }),
                        precheck_status == 500,
                        "an unavailable precheck must be reported honestly"
                    );
                },
            );
            assert_eq!(server.seen.lock().expect("requests").len(), 5);
            server.join();
        }
    }
}

#[test]
#[serial]
fn retained_permanent_failure_does_not_gain_retryable_visibility_shortcut() {
    for parallel in [false, true] {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo.log");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![404, 404, 200]
                    .into_iter()
                    .map(|status| (status, "{}".to_string()))
                    .collect(),
            )]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let state_dir = td.path().join(".shipper");
        let mut opts = default_opts(state_dir.clone());
        opts.parallel.enabled = parallel;
        opts.max_attempts = 3;
        opts.readiness.max_total_wait = Duration::ZERO;
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().to_string()),
                ),
            ],
            || {
                run_publish(&ws, &opts, &mut CollectingReporter::default())
                    .expect_err("permanent failure stops without automatic retries");
            },
        );
        assert_eq!(cargo_invocations(&cargo_log), 1);
        let retained = state::load_state(&state_dir)
            .expect("load permanent failure")
            .expect("state");
        assert!(matches!(
            retained.packages["demo@0.1.0"].state,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                ..
            }
        ));

        // An explicit later invocation after the operator fixes the Cargo
        // failure retains its existing path. Permanent failures do not acquire
        // the new pre-attempt visibility shortcut for Retryable state.
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                ("SHIPPER_CARGO_STDERR", Some(String::new())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().to_string()),
                ),
            ],
            || {
                let receipt = run_resume(&ws, &opts, &mut CollectingReporter::default())
                    .expect("explicit invocation after simulated operator repair");
                assert_eq!(receipt.packages[0].attempts, 2);
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                assert_eq!(cargo_invocations(&cargo_log), 2);
            },
        );
        assert_eq!(server.seen.lock().expect("requests").len(), 3);
        server.join();
    }
}
