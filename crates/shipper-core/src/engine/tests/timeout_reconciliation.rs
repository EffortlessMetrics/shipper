//! Actual process timeouts must enter the shared reconciliation state machine.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

const SECRET: &str = "typed-timeout-secret-sentinel";
const CHILD: &str = "engine::tests::timeout_reconciliation::timed_cargo_child";

#[test]
#[serial]
#[ignore = "fake Cargo subprocess invoked by the timeout reconciliation tests"]
fn timed_cargo_child() {
    let root = std::env::current_dir().expect("fixture root");
    let calls_path = root.join("cargo-invocations");
    let previous_calls = fs::read_to_string(&calls_path)
        .map(|calls| calls.lines().count())
        .unwrap_or(0);
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(calls_path)
        .expect("calls file");
    writeln!(calls, "publish").expect("record Cargo invocation");
    calls.flush().expect("flush calls");

    let message = fs::read_to_string(root.join("cargo-message")).expect("fixture message");
    println!("Authorization: Bearer {SECRET}");
    eprintln!("{message}");
    std::io::stdout().flush().expect("flush stdout");
    std::io::stderr().flush().expect("flush stderr");

    if root.join("exit-normally").exists() {
        std::process::exit(42);
    }
    if previous_calls > 0 {
        std::process::exit(0);
    }

    // On Unix the shell proxy execs this process, so the runner kills it.
    // On Windows a command script may leave this helper alive after killing
    // the script; its finite lifetime bounds the fixture's inherited EOF.
    // This is not a new production process-tree cleanup guarantee.
    thread::sleep(Duration::from_secs(2));
    std::process::exit(42);
}

fn write_timed_cargo(bin: &Path) {
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = fake_cargo_path(bin);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ -f silent-cargo ]; then\n  exec \"$SHIPPER_TIMEOUT_FIXTURE_EXE\" --exact {CHILD} --ignored --nocapture --quiet >/dev/null 2>&1\nfi\nexec \"$SHIPPER_TIMEOUT_FIXTURE_EXE\" --exact {CHILD} --ignored --nocapture --quiet\n"
            ),
        )
        .expect("write Cargo proxy");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    #[cfg(windows)]
    fs::write(
        fake_cargo_path(bin),
        format!(
            "@echo off\r\nif exist silent-cargo goto :silent\r\n\"%SHIPPER_TIMEOUT_FIXTURE_EXE%\" --exact {CHILD} --ignored --nocapture --quiet\r\nexit /b %errorlevel%\r\n:silent\r\n\"%SHIPPER_TIMEOUT_FIXTURE_EXE%\" --exact {CHILD} --ignored --nocapture --quiet >nul 2>&1\r\nexit /b %errorlevel%\r\n"
        ),
    )
    .expect("write Cargo proxy");
}

pub(super) struct ProbeRegistry {
    pub(super) base_url: String,
    pub(super) seen: Arc<Mutex<Vec<u16>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProbeRegistry {
    pub(super) fn start(statuses: Vec<u16>) -> Self {
        Self::start_for_packages(statuses, vec!["demo"])
    }

    pub(super) fn start_for_packages(statuses: Vec<u16>, packages: Vec<&'static str>) -> Self {
        let server = Server::http("127.0.0.1:0").expect("loopback registry");
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&seen);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut next = 0;
            while !worker_stop.load(Ordering::Acquire) {
                let Some(request) = server
                    .recv_timeout(Duration::from_millis(20))
                    .expect("registry request")
                else {
                    continue;
                };
                assert!(
                    packages
                        .iter()
                        .any(|name| { request.url() == format!("/api/v1/crates/{name}/0.1.0") }),
                    "unexpected fixture request: {}",
                    request.url()
                );
                let status = statuses[next.min(statuses.len() - 1)];
                next += 1;
                observed.lock().expect("seen").push(status);
                request
                    .respond(Response::from_string("{}").with_status_code(StatusCode(status)))
                    .expect("respond");
            }
        });
        Self {
            base_url,
            seen,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for ProbeRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Observation {
    Published,
    NotPublished,
    StillUnknown,
}

fn assert_timeout_reconciliation(
    parallel: bool,
    message: &str,
    observation: Observation,
    output_lines: usize,
) {
    let td = tempdir().expect("fixture");
    let bin = td.path().join("bin");
    write_fake_tools(&bin);
    write_timed_cargo(&bin);
    fs::write(td.path().join("cargo-message"), message).expect("message");
    if message.is_empty() {
        fs::write(td.path().join("silent-cargo"), "suppress child diagnostics")
            .expect("silent child marker");
    }
    let statuses = match observation {
        Observation::Published => vec![404, 200],
        Observation::NotPublished => vec![404, 404, 200],
        Observation::StillUnknown => vec![404, 503, 503],
    };
    let server = ProbeRegistry::start(statuses.clone());
    let ws = planned_workspace(td.path(), server.base_url.clone());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = parallel;
    opts.parallel.per_package_timeout = Duration::from_secs(1);
    opts.readiness.max_total_wait = Duration::ZERO;
    opts.output_lines = output_lines;
    let mut env = fake_program_env_vars(&bin);
    env.push((
        "SHIPPER_TIMEOUT_FIXTURE_EXE",
        Some(
            std::env::current_exe()
                .expect("test binary")
                .display()
                .to_string(),
        ),
    ));

    temp_env::with_vars(env, || {
        let mut reporter = CollectingReporter::default();
        let result = run_publish(&ws, &opts, &mut reporter);
        let receipt = if matches!(observation, Observation::StillUnknown) {
            let error = result.expect_err("unknown timeout must stop");
            assert!(
                error.downcast_ref::<PublishStillUnknownError>().is_some(),
                "{error:#}"
            );
            assert!(!state::receipt_path(&state_dir).exists());
            let error = run_resume(&ws, &opts, &mut reporter)
                .expect_err("resume must reconcile without another Cargo attempt");
            assert!(
                error.downcast_ref::<PublishStillUnknownError>().is_some(),
                "{error:#}"
            );
            None
        } else {
            Some(result.expect("published or conclusively absent timeout can finish"))
        };

        let expected_calls = if matches!(observation, Observation::NotPublished) {
            2
        } else {
            1
        };
        assert_eq!(
            fs::read_to_string(td.path().join("cargo-invocations"))
                .expect("calls")
                .lines()
                .count(),
            expected_calls,
            "{parallel:?} {message:?} {observation:?}"
        );
        assert_eq!(*server.seen.lock().expect("seen"), statuses);
        let retained = state::load_state(&state_dir).expect("load").expect("state");
        assert_eq!(
            retained.packages["demo@0.1.0"].attempts,
            expected_calls as u32
        );
        assert_eq!(
            retained.attempt_history[0].error_class,
            Some(ErrorClass::Ambiguous)
        );
        let event_path = events::events_path(&state_dir);
        let log = events::EventLog::read_from_file(&event_path).expect("events");
        let timeline = log.all_events();
        assert!(timeline.iter().any(|event| matches!(
            &event.event_type,
            EventType::PackageFailed { class: ErrorClass::Ambiguous, message }
                if message.contains("process timed out")
        )));
        let reconciled = timeline
            .iter()
            .filter_map(|event| match &event.event_type {
                EventType::PublishReconciled { outcome } => Some(outcome),
                _ => None,
            })
            .collect::<Vec<_>>();
        let report: crate::types::ReconciliationReport = serde_json::from_slice(
            &fs::read(state::reconciliation_path(&state_dir)).expect("reconciliation report"),
        )
        .expect("report JSON");
        assert_eq!(report.records.len(), reconciled.len());
        for (record, outcome) in report.records.iter().zip(&reconciled) {
            assert_eq!(
                serde_json::to_value(&record.outcome).unwrap(),
                serde_json::to_value(outcome).unwrap()
            );
        }
        match observation {
            Observation::Published => {
                assert!(matches!(
                    reconciled.as_slice(),
                    [ReconciliationOutcome::Published { attempts: 1, .. }]
                ));
                assert!(matches!(
                    retained.packages["demo@0.1.0"].state,
                    PackageState::Published
                ));
            }
            Observation::NotPublished => {
                assert!(matches!(
                    reconciled.as_slice(),
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
                    .expect("absence event");
                let retry = timeline
                    .iter()
                    .position(|event| {
                        matches!(
                            event.event_type,
                            EventType::PackageAttempted { attempt: 2, .. }
                        )
                    })
                    .expect("second attempt");
                assert!(
                    absent < retry,
                    "absence must be retained before retry starts"
                );
                assert!(matches!(
                    retained.packages["demo@0.1.0"].state,
                    PackageState::Published
                ));
            }
            Observation::StillUnknown => {
                assert_eq!(reconciled.len(), 2);
                assert!(reconciled.iter().all(|outcome| matches!(
                    outcome,
                    ReconciliationOutcome::StillUnknown { attempts: 1, .. }
                )));
                assert!(matches!(
                    retained.packages["demo@0.1.0"].state,
                    PackageState::Ambiguous { .. }
                ));
                assert!(!state::receipt_path(&state_dir).exists());
            }
        }
        let rebuilt = crate::state::rebuild::rebuild_state_from_events(
            &event_path,
            crate::state::rebuild::StateRebuildOptions::new(ws.plan.registry.clone())
                .with_fallback_plan_id(&ws.plan.plan_id),
        )
        .expect("rebuild");
        assert_eq!(
            rebuilt.packages["demo@0.1.0"].attempts,
            expected_calls as u32
        );
        assert_eq!(
            serde_json::to_value(&rebuilt.packages["demo@0.1.0"].state).unwrap(),
            serde_json::to_value(&retained.packages["demo@0.1.0"].state).unwrap()
        );
        if let Some(receipt) = receipt {
            assert!(matches!(receipt.execution_result, ExecutionResult::Success));
            assert_eq!(receipt.packages[0].evidence.attempts[0].exit_code, -1);
        }
        for name in [
            "events.jsonl",
            "state.json",
            "reconciliation.json",
            "receipt.json",
        ] {
            if let Ok(contents) = fs::read_to_string(state_dir.join(name)) {
                assert!(!contents.contains(SECRET), "secret in {name}");
            }
        }
        for messages in [&reporter.infos, &reporter.warns, &reporter.errors] {
            assert!(messages.iter().all(|message| !message.contains(SECRET)));
        }
    });
}

#[test]
#[serial]
fn actual_timeouts_reconcile_misleading_output_in_both_schedulers() {
    for parallel in [false, true] {
        for message in ["connection reset by peer", "permission denied"] {
            for observation in [
                Observation::Published,
                Observation::NotPublished,
                Observation::StillUnknown,
            ] {
                assert_timeout_reconciliation(parallel, message, observation, 100);
            }
        }
    }
}

#[test]
#[serial]
fn actual_timeouts_reconcile_silent_children_and_empty_tails() {
    for parallel in [false, true] {
        for (message, output_lines) in [("permission denied", 0), ("", 100)] {
            assert_timeout_reconciliation(
                parallel,
                message,
                Observation::StillUnknown,
                output_lines,
            );
        }
    }
}

#[test]
#[serial]
fn ordinary_exits_keep_output_classification() {
    for parallel in [false, true] {
        for (message, expected_class) in [
            ("network timeout while uploading", ErrorClass::Retryable),
            ("permission denied", ErrorClass::Permanent),
        ] {
            let td = tempdir().expect("fixture");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            write_timed_cargo(&bin);
            fs::write(td.path().join("cargo-message"), message).expect("message");
            fs::write(td.path().join("exit-normally"), "exit 42").expect("normal exit marker");
            let server = ProbeRegistry::start(vec![404]);
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = parallel;
            opts.parallel.per_package_timeout = Duration::from_secs(5);
            opts.max_attempts = 1;
            opts.readiness.max_total_wait = Duration::ZERO;
            let mut env = fake_program_env_vars(&bin);
            env.push((
                "SHIPPER_TIMEOUT_FIXTURE_EXE",
                Some(
                    std::env::current_exe()
                        .expect("test binary")
                        .display()
                        .to_string(),
                ),
            ));
            temp_env::with_vars(env, || {
                run_publish(&ws, &opts, &mut CollectingReporter::default())
                    .expect_err("ordinary failure");
                let retained = state::load_state(&state_dir).expect("load").expect("state");
                match &retained.packages["demo@0.1.0"].state {
                    PackageState::Failed { class, .. } => assert_eq!(
                        class, &expected_class,
                        "parallel={parallel}, message={message:?}"
                    ),
                    state => panic!("ordinary exit must preserve failure class: {state:?}"),
                }
                assert_eq!(
                    retained.attempt_history[0].error_class.as_ref(),
                    Some(&expected_class)
                );
                let log = events::EventLog::read_from_file(&events::events_path(&state_dir))
                    .expect("events");
                assert!(!log.all_events().iter().any(|event| matches!(
                    event.event_type,
                    EventType::PublishReconciling { .. } | EventType::PublishReconciled { .. }
                )));
                let replay = crate::state::rebuild::rebuild_state_from_events(
                    &events::events_path(&state_dir),
                    crate::state::rebuild::StateRebuildOptions::new(retained.registry.clone()),
                )
                .expect("replay final failure");
                assert_eq!(
                    replay.packages["demo@0.1.0"].state,
                    retained.packages["demo@0.1.0"].state
                );
                assert_eq!(replay.attempt_history, retained.attempt_history);
                // Retryable exhaustion keeps its existing final visibility
                // check; Permanent returns after the initial quick check.
                let requests = if expected_class == ErrorClass::Retryable {
                    vec![404, 404, 404]
                } else {
                    vec![404, 404]
                };
                assert_eq!(*server.seen.lock().expect("seen"), requests);
                assert_eq!(
                    fs::read_to_string(td.path().join("cargo-invocations"))
                        .expect("calls")
                        .lines()
                        .count(),
                    1
                );
            });
        }
    }
}
