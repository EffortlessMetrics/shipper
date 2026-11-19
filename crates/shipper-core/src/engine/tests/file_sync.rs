//! File-sync faults must reach the public publish result after actual fake Cargo.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

const INJECTED: &str = "injected required file-sync failure";

struct RegistryFixture {
    base_url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RegistryFixture {
    fn start() -> Self {
        Self::with_first_status(404)
    }

    fn with_first_status(first_status: u16) -> Self {
        let server = Server::http("127.0.0.1:0").expect("loopback registry");
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let Some(request) = server.recv_timeout(Duration::from_millis(20)).unwrap() else {
                    continue;
                };
                assert_eq!(request.url(), "/api/v1/crates/demo/0.1.0");
                let status = if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    first_status
                } else {
                    200
                };
                request
                    .respond(Response::from_string("{}").with_status_code(status))
                    .unwrap();
            }
        });
        Self {
            base_url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn ordered_attempts(alpha_sync_failed: Arc<AtomicBool>) -> Self {
        let server = Server::http("127.0.0.1:0").expect("loopback registry");
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut pending_beta: Option<tiny_http::Request> = None;
            let mut beta_checked = false;
            while !worker_stop.load(Ordering::Acquire) {
                // Beta cannot commit an attempt until alpha's flushed attempt
                // has failed sync. Keep servicing alpha even if beta arrives
                // first; never wait while holding the engine's event mutex.
                if pending_beta.is_some()
                    && (alpha_sync_failed.load(Ordering::Acquire)
                        || started.elapsed() > Duration::from_secs(2))
                {
                    let status = if alpha_sync_failed.load(Ordering::Acquire) {
                        404
                    } else {
                        503
                    };
                    pending_beta
                        .take()
                        .unwrap()
                        .respond(Response::from_string("{}").with_status_code(status))
                        .unwrap();
                }
                let Some(request) = server.recv_timeout(Duration::from_millis(10)).unwrap() else {
                    continue;
                };
                seen.fetch_add(1, Ordering::SeqCst);
                match request.url() {
                    "/api/v1/crates/alpha/0.1.0" => {
                        request
                            .respond(Response::from_string("{}").with_status_code(404))
                            .unwrap();
                    }
                    "/api/v1/crates/beta/0.1.0" if !beta_checked => {
                        beta_checked = true;
                        pending_beta = Some(request);
                    }
                    "/api/v1/crates/beta/0.1.0" => {
                        request
                            .respond(Response::from_string("{}").with_status_code(200))
                            .unwrap();
                    }
                    other => panic!("unexpected registry route: {other}"),
                }
            }
        });
        Self {
            base_url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("registry fixture worker");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    StartedEvent,
    SkippedTransition,
    OutputEvent,
    UploadedEvent,
    UploadedProjection,
    ReadinessPoll,
    ReadinessComplete,
    Receipt,
}

fn assert_publish_file_sync(fault: Fault, parallel: bool) {
    let dir = tempdir().expect("fixture");
    let bin = dir.path().join("bin");
    write_fake_tools(&bin);
    let registry = if fault == Fault::SkippedTransition {
        RegistryFixture::with_first_status(200)
    } else {
        RegistryFixture::start()
    };
    let ws = planned_workspace(dir.path(), registry.base_url.clone());
    let state_dir = dir.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = parallel;
    opts.parallel.per_package_timeout = Duration::from_secs(2);
    let calls = dir.path().join("cargo-calls");
    let mut environment = fake_program_env_vars(&bin);
    environment.extend([
        ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
        ("SHIPPER_CARGO_ARGS_LOG", Some(calls.display().to_string())),
        (
            "CARGO_HOME",
            Some(dir.path().join("cargo-home").display().to_string()),
        ),
        ("CARGO_REGISTRY_TOKEN", None),
        ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
    ]);
    let failures = Arc::new(AtomicUsize::new(0));
    let observed_failures = Arc::clone(&failures);
    let _fault = crate::state::file_sync_test_support::observe(&state_dir, move |path| {
        let selected = match (fault, path.file_name().and_then(|name| name.to_str())) {
            (
                Fault::StartedEvent
                | Fault::SkippedTransition
                | Fault::OutputEvent
                | Fault::ReadinessPoll
                | Fault::ReadinessComplete,
                Some("events.jsonl"),
            ) => events::EventLog::read_from_file(path)
                .expect("actual complete event bytes before sync")
                .all_events()
                .iter()
                .any(|event| {
                    matches!(
                        (fault, &event.event_type),
                        (Fault::StartedEvent, EventType::PackageStarted { .. })
                            | (Fault::SkippedTransition, EventType::PackageSkipped { .. })
                            | (Fault::OutputEvent, EventType::PackageOutput { .. })
                            | (Fault::ReadinessPoll, EventType::ReadinessPoll { .. })
                            | (
                                Fault::ReadinessComplete,
                                EventType::ReadinessComplete { .. }
                            )
                    )
                }),
            (Fault::UploadedEvent, Some("events.jsonl")) => events::EventLog::read_from_file(path)
                .expect("the actual event file is flushed before sync")
                .all_events()
                .iter()
                .any(|event| matches!(event.event_type, EventType::PackageUploaded)),
            (Fault::UploadedProjection, Some("state.tmp")) => {
                let projection: ExecutionState =
                    serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                projection.packages["demo@0.1.0"].state == PackageState::Uploaded
            }
            (Fault::Receipt, Some("receipt.tmp")) => {
                let receipt: Receipt = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                assert_eq!(
                    receipt.execution_result,
                    crate::types::ExecutionResult::Success
                );
                assert_eq!(receipt.packages[0].state, PackageState::Published);
                true
            }
            _ => false,
        };
        if selected {
            observed_failures.fetch_add(1, Ordering::SeqCst);
            return Err(std::io::Error::other(INJECTED));
        }
        Ok(())
    });

    temp_env::with_vars(environment, || {
        let started = std::time::Instant::now();
        let result = run_publish(&ws, &opts, &mut CollectingReporter::default());
        assert!(started.elapsed() < Duration::from_secs(5));
        let complete = matches!(fault, Fault::None | Fault::Receipt);
        let uploaded = !matches!(
            fault,
            Fault::StartedEvent | Fault::SkippedTransition | Fault::OutputEvent
        );
        let cargo_called = !matches!(fault, Fault::StartedEvent | Fault::SkippedTransition);
        let readiness_failed = matches!(fault, Fault::ReadinessPoll | Fault::ReadinessComplete);
        let events_path = events::events_path(&state_dir);
        let log = events::EventLog::read_from_file(&events_path).expect("retained complete events");
        assert_eq!(
            log.all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::PackageUploaded))
                .count(),
            usize::from(uploaded)
        );
        assert_eq!(
            log.all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::PackageAttempted { .. }))
                .count(),
            usize::from(cargo_called)
        );
        assert_eq!(
            log.all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::ExecutionFinished { .. }))
                .count(),
            usize::from(complete)
        );
        let retained_state = state::load_state(&state_dir)
            .expect("read prior state")
            .unwrap();
        assert_eq!(
            retained_state.packages["demo@0.1.0"].attempts,
            u32::from(cargo_called)
        );
        assert_eq!(
            retained_state.packages["demo@0.1.0"].state,
            if complete {
                PackageState::Published
            } else if readiness_failed {
                PackageState::Uploaded
            } else {
                PackageState::Pending
            }
        );
        assert_eq!(
            registry.requests.load(Ordering::SeqCst),
            if complete || readiness_failed {
                2
            } else {
                usize::from(fault != Fault::StartedEvent)
            }
        );
        let actual_calls = fs::read_to_string(&calls).unwrap_or_default();
        assert_eq!(
            actual_calls
                .lines()
                .filter(|line| line.split_whitespace().next() == Some("publish"))
                .count(),
            usize::from(cargo_called)
        );
        if readiness_failed {
            let selected_count = log
                .all_events()
                .iter()
                .filter(|event| {
                    matches!(
                        (fault, &event.event_type),
                        (Fault::ReadinessPoll, EventType::ReadinessPoll { .. })
                            | (
                                Fault::ReadinessComplete,
                                EventType::ReadinessComplete { .. }
                            )
                    )
                })
                .count();
            assert_eq!(
                selected_count, 1,
                "readiness close must not reappend the failed batch"
            );
            assert!(
                !log.all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::ReadinessError { .. })),
                "a held owner cannot append a speculative close event"
            );
        }

        if fault == Fault::None {
            let receipt = result.expect("normal synchronized persistence succeeds");
            assert_eq!(
                receipt.execution_result,
                crate::types::ExecutionResult::Success
            );
            assert!(state::receipt_path(&state_dir).is_file());
            assert_eq!(failures.load(Ordering::SeqCst), 0);
        } else {
            let error =
                result.expect_err("required file-sync failure cannot become a success receipt");
            let diagnostic = format!("{error:#}");
            assert!(
                diagnostic.contains(INJECTED),
                "{fault:?}/{parallel}: {diagnostic}"
            );
            let failed_path = match fault {
                Fault::StartedEvent
                | Fault::SkippedTransition
                | Fault::OutputEvent
                | Fault::UploadedEvent
                | Fault::ReadinessPoll
                | Fault::ReadinessComplete => events_path,
                Fault::UploadedProjection => state::state_path(&state_dir).with_extension("tmp"),
                Fault::Receipt => state::receipt_path(&state_dir).with_extension("tmp"),
                Fault::None => unreachable!(),
            };
            assert!(
                diagnostic.contains(&failed_path.display().to_string()),
                "{diagnostic}"
            );
            assert!(
                error.downcast_ref::<std::io::Error>().is_some(),
                "source I/O error was lost: {diagnostic}"
            );
            assert_eq!(failures.load(Ordering::SeqCst), 1);
            assert!(
                !state::receipt_path(&state_dir).exists(),
                "no completed receipt is installed"
            );
            if fault == Fault::Receipt {
                assert!(
                    failed_path.is_file(),
                    "uncommitted receipt bytes remain for diagnosis"
                );
            }
        }
    });
}

#[test]
#[serial]
fn file_sync_output_failure_retains_path_and_source_in_both_modes() {
    for parallel in [false, true] {
        assert_publish_file_sync(Fault::OutputEvent, parallel);
    }
}

#[test]
#[serial]
fn file_sync_start_and_skip_errors_retain_path_and_source_in_both_modes() {
    for fault in [Fault::StartedEvent, Fault::SkippedTransition] {
        for parallel in [false, true] {
            assert_publish_file_sync(fault, parallel);
        }
    }
}

#[test]
#[serial]
fn file_sync_readiness_close_does_not_reappend_failed_events() {
    for fault in [Fault::ReadinessPoll, Fault::ReadinessComplete] {
        for parallel in [false, true] {
            assert_publish_file_sync(fault, parallel);
        }
    }
}

#[test]
#[serial]
fn file_sync_uploaded_event_failure_stops_both_publish_modes_without_receipt() {
    for parallel in [false, true] {
        assert_publish_file_sync(Fault::UploadedEvent, parallel);
    }
}

#[test]
#[serial]
fn file_sync_uploaded_projection_failure_stops_both_publish_modes_without_receipt() {
    for parallel in [false, true] {
        assert_publish_file_sync(Fault::UploadedProjection, parallel);
    }
}

#[test]
#[serial]
fn file_sync_receipt_failure_is_not_success_in_either_publish_mode() {
    for parallel in [false, true] {
        assert_publish_file_sync(Fault::Receipt, parallel);
    }
}

#[test]
#[serial]
fn file_sync_success_preserves_both_publish_modes() {
    for parallel in [false, true] {
        assert_publish_file_sync(Fault::None, parallel);
    }
}

#[test]
#[serial]
fn file_sync_resume_skip_and_promotion_preserve_original_io_context() {
    for terminal in [false, true] {
        for parallel in [false, true] {
            let dir = tempdir().expect("fixture");
            let bin = dir.path().join("bin");
            write_fake_tools(&bin);
            let registry = RegistryFixture::start();
            let ws = planned_workspace(dir.path(), registry.base_url.clone());
            let state_dir = dir.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = parallel;
            opts.parallel.per_package_timeout = Duration::from_secs(2);
            let calls = dir.path().join("cargo-calls");
            let mut environment = fake_program_env_vars(&bin);
            environment.extend([
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("SHIPPER_CARGO_ARGS_LOG", Some(calls.display().to_string())),
                (
                    "CARGO_HOME",
                    Some(dir.path().join("cargo-home").display().to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
            ]);
            temp_env::with_vars(environment, || {
                // Produce the retained state through the real public path. A
                // completed run supplies the terminal-skip branch. A returned
                // readiness sync error leaves Uploaded for the promotion branch.
                let seed_fault = (!terminal).then(|| {
                    crate::state::file_sync_test_support::observe(&state_dir, |path| {
                        if path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl")
                            && events::EventLog::read_from_file(path)
                                .unwrap()
                                .all_events()
                                .iter()
                                .any(|event| {
                                    matches!(event.event_type, EventType::ReadinessComplete { .. })
                                })
                        {
                            return Err(std::io::Error::other("seed readiness interruption"));
                        }
                        Ok(())
                    })
                });
                let first = run_publish(&ws, &opts, &mut CollectingReporter::default());
                if terminal {
                    first.expect("initial complete publish");
                } else {
                    let error = first.expect_err("retain uploaded state before promotion");
                    assert!(format!("{error:#}").contains("seed readiness interruption"));
                }
                drop(seed_fault);

                let retained = state::load_state(&state_dir).unwrap().unwrap();
                assert_eq!(
                    retained.packages["demo@0.1.0"].state,
                    if terminal {
                        PackageState::Published
                    } else {
                        PackageState::Uploaded
                    }
                );
                let events_path = events::events_path(&state_dir);
                let previous_events = fs::read(&events_path).unwrap();
                let previous_receipt = fs::read(state::receipt_path(&state_dir)).ok();
                let failures = Arc::new(AtomicUsize::new(0));
                let observed_failures = Arc::clone(&failures);
                let _fault =
                    crate::state::file_sync_test_support::observe(&state_dir, move |path| {
                        if path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl")
                            && events::EventLog::read_from_file(path)
                                .unwrap()
                                .all_events()
                                .iter()
                                .any(|event| {
                                    matches!(
                                        (terminal, &event.event_type),
                                        (true, EventType::PackageSkipped { .. })
                                            | (false, EventType::PackagePublished { .. })
                                    )
                                })
                        {
                            observed_failures.fetch_add(1, Ordering::SeqCst);
                            return Err(std::io::Error::other(INJECTED));
                        }
                        Ok(())
                    });
                // The failed-owner hold is deliberately instance-local. This
                // explicit second invocation inspects retained complete history
                // and rechecks Uploaded visibility; it proves no automatic
                // cross-invocation repair or hardware durability guarantee.
                let error = run_resume(&ws, &opts, &mut CollectingReporter::default())
                    .expect_err("resume persistence failure cannot return a success receipt");
                let diagnostic = format!("{error:#}");
                assert!(diagnostic.contains(INJECTED), "{diagnostic}");
                assert!(diagnostic.contains(&events_path.display().to_string()));
                assert!(error.downcast_ref::<std::io::Error>().is_some());
                assert_eq!(failures.load(Ordering::SeqCst), 1);
                assert_eq!(
                    fs::read(state::receipt_path(&state_dir)).ok(),
                    previous_receipt,
                    "failed resume must not replace or install a receipt"
                );
                let after = state::load_state(&state_dir).unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(&after.packages).unwrap(),
                    serde_json::to_value(&retained.packages).unwrap()
                );
                assert_eq!(
                    fs::read_to_string(&calls)
                        .unwrap()
                        .lines()
                        .filter(|line| line.split_whitespace().next() == Some("publish"))
                        .count(),
                    1,
                    "resume must not invoke Cargo for either retained state"
                );
                assert_eq!(
                    registry.requests.load(Ordering::SeqCst),
                    if terminal { 2 } else { 3 }
                );
                let bytes = fs::read(&events_path).unwrap();
                assert!(bytes.starts_with(&previous_events));
                let appended: Vec<PublishEvent> =
                    std::str::from_utf8(&bytes[previous_events.len()..])
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                assert_eq!(
                    appended
                        .iter()
                        .filter(|event| matches!(
                            (terminal, &event.event_type),
                            (true, EventType::PackageSkipped { .. })
                                | (false, EventType::PackagePublished { .. })
                        ))
                        .count(),
                    1
                );
                assert!(!appended.iter().any(|event| matches!(
                    event.event_type,
                    EventType::PackageAttempted { .. } | EventType::ExecutionFinished { .. }
                )));
            });
        }
    }
}

#[test]
#[serial]
fn file_sync_failed_attempt_batch_is_not_reappended_by_next_worker() {
    let dir = tempdir().expect("fixture");
    let bin = dir.path().join("bin");
    write_fake_tools(&bin);
    let fault_seen = Arc::new(AtomicBool::new(false));
    let registry = RegistryFixture::ordered_attempts(Arc::clone(&fault_seen));
    let mut ws = planned_workspace(dir.path(), registry.base_url.clone());
    ws.plan.packages = ["alpha", "beta"]
        .map(|name| PlannedPackage {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            manifest_path: dir.path().join(name).join("Cargo.toml"),
            regime: None,
        })
        .to_vec();
    let state_dir = dir.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = true;
    opts.parallel.max_concurrent = 2;
    opts.parallel.per_package_timeout = Duration::from_secs(2);
    let calls = dir.path().join("cargo-calls");
    let observed_fault = Arc::clone(&fault_seen);
    let _fault = crate::state::file_sync_test_support::observe(&state_dir, move |path| {
        if path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl") {
            let log = events::EventLog::read_from_file(path).expect("flushed complete events");
            if log.all_events().iter().any(|event| {
                event.package == "alpha@0.1.0"
                    && matches!(
                        event.event_type,
                        EventType::PackageAttempted { attempt: 1, .. }
                    )
            }) && !observed_fault.swap(true, Ordering::AcqRel)
            {
                return Err(std::io::Error::other(INJECTED));
            }
        }
        Ok(())
    });
    let mut environment = fake_program_env_vars(&bin);
    environment.extend([
        ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
        ("SHIPPER_CARGO_ARGS_LOG", Some(calls.display().to_string())),
        (
            "CARGO_HOME",
            Some(dir.path().join("cargo-home").display().to_string()),
        ),
        ("CARGO_REGISTRY_TOKEN", None),
        ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
    ]);
    temp_env::with_vars(environment, || {
        let started = std::time::Instant::now();
        let error = run_publish(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("alpha's required sync failure stops finalization");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(fault_seen.load(Ordering::Acquire));
        let events_path = events::events_path(&state_dir);
        let log = events::EventLog::read_from_file(&events_path).expect("read retained events");
        let attempts = log
            .all_events()
            .iter()
            .filter(|event| {
                event.package == "alpha@0.1.0"
                    && matches!(
                        event.event_type,
                        EventType::PackageAttempted { attempt: 1, .. }
                    )
            })
            .count();
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &events_path,
            crate::state::rebuild::StateRebuildOptions::new(ws.plan.registry.clone()),
        )
        .expect("rebuild actual event bytes");
        let histories = rebuilt
            .attempt_history
            .iter()
            .filter(|attempt| attempt.package == "alpha" && attempt.attempt == 1)
            .count();
        let actual_calls = fs::read_to_string(&calls).unwrap_or_default();
        assert_eq!(
            (attempts, histories),
            (1, 1),
            "failed alpha batch was blindly replayed; Cargo calls: {actual_calls}"
        );
        assert!(
            !actual_calls.lines().any(|line| line.contains("alpha")),
            "alpha failed before Cargo admission"
        );
        // Both workers are deliberately before attempt admission at the fault.
        // This is not a cancellation promise for already-running peer Cargo.
        assert_eq!(
            actual_calls
                .lines()
                .filter(|line| line.split_whitespace().next() == Some("publish"))
                .count(),
            0
        );
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains(INJECTED));
        assert!(diagnostic.contains(&events_path.display().to_string()));
        assert!(
            error.downcast_ref::<std::io::Error>().is_some(),
            "parallel aggregation must retain the original I/O cause: {diagnostic}"
        );
        assert!(!state::receipt_path(&state_dir).exists());
    });
}

fn assert_cleanup_file_sync_failure(rehearsal: bool, owner_already_failed: bool) {
    use crate::ops::process::CleanupFailure;
    use crate::types::AttemptDetail;

    let dir = tempdir().unwrap();
    let ws = planned_workspace(dir.path(), "http://127.0.0.1:9".into());
    let state_dir = dir.path().join(".shipper");
    let events_path = events::events_path(&state_dir);
    let retained = init_state(&ws, &state_dir).unwrap();
    let shared_state = Arc::new(Mutex::new(retained));
    let shared_log = Arc::new(Mutex::new(events::EventLog::new()));
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&sync_calls);
    let fault = crate::state::file_sync_test_support::observe(&state_dir, move |path| {
        if path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl") {
            // Distinguish an appended event from a rejected open or write.
            assert!(!events::EventLog::read_from_file(path).unwrap().is_empty());
            observed_calls.fetch_add(1, Ordering::SeqCst);
            return Err(std::io::Error::other(INJECTED));
        }
        Ok(())
    });

    if owner_already_failed {
        let mut log = shared_log.lock().unwrap();
        log.record(PublishEvent {
            timestamp: Utc::now(),
            package: "demo@0.1.0".into(),
            event_type: EventType::PackageStarted {
                name: "demo".into(),
                version: "0.1.0".into(),
            },
        });
        log.write_to_file(&events_path).unwrap_err();
    }

    let message = cleanup::message(CleanupFailure::InheritedPipe);
    let error = if rehearsal {
        cleanup::stop_rehearsal(
            &mut shared_log.lock().unwrap(),
            &events_path,
            "demo@0.1.0",
            message,
        )
    } else {
        let now = Utc::now();
        cleanup::stop_publish(
            &shared_state,
            &state_dir,
            &shared_log,
            &events_path,
            "demo@0.1.0",
            message,
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
        )
    };
    drop(fault);

    assert!(matches!(
        classify_publish_stop(&error),
        Some(PublishStopClassification::CleanupIncomplete)
    ));
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains(INJECTED), "{diagnostic}");
    assert!(diagnostic.contains(&events_path.display().to_string()));
    assert!(
        error.downcast_ref::<std::io::Error>().is_some(),
        "cleanup classification must retain the underlying I/O cause: {diagnostic}"
    );
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    let retained_bytes = fs::read(&events_path).unwrap();
    let recorded = events::EventLog::read_from_file(&events_path).unwrap();
    assert_eq!(
        recorded
            .all_events()
            .iter()
            .filter(|event| matches!(event.event_type, EventType::PackageCleanupIncomplete { .. }))
            .count(),
        usize::from(!owner_already_failed)
    );
    let mut log = shared_log.lock().unwrap();
    let pending = log.len();
    log.clear();
    assert_eq!(log.len(), pending, "the failed batch stays inspectable");
    log.write_to_file(&events_path).unwrap_err();
    assert_eq!(fs::read(&events_path).unwrap(), retained_bytes);
    drop(log);

    if !rehearsal {
        let state = state::load_state(&state_dir).unwrap().unwrap();
        assert!(matches!(
            state.packages["demo@0.1.0"].state,
            PackageState::CleanupIncomplete { .. }
        ));
        assert_eq!(state.attempt_history.len(), 1);
        assert!(matches!(
            shared_state.lock().unwrap().packages["demo@0.1.0"].state,
            PackageState::CleanupIncomplete { .. }
        ));
    }
    assert!(matches!(
        classify_publish_stop(&validate_publish_cleanup_safety(&state_dir).unwrap_err()),
        Some(PublishStopClassification::CleanupIncomplete)
    ));
    assert!(!state::receipt_path(&state_dir).exists());
}

#[test]
#[serial]
fn file_sync_cleanup_hold_retains_io_cause_and_projection_fallback() {
    for owner_already_failed in [false, true] {
        assert_cleanup_file_sync_failure(false, owner_already_failed);
    }
}

#[test]
#[serial]
fn file_sync_rehearsal_cleanup_retains_io_cause_and_event() {
    assert_cleanup_file_sync_failure(true, false);
}
