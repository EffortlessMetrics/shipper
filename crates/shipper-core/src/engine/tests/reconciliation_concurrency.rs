//! Schedule real package workers around a valid reconciliation destination.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{TryLockError, mpsc};
use std::time::Instant;

use crate::state::reconciliation::test_hooks::{self, WritePhase};
use crate::types::{ReconciliationOutcome, ReconciliationReport};

use super::*;

const WAIT: Duration = Duration::from_secs(5);
const CHILD: &str = "engine::tests::reconciliation_concurrency::gated_cargo_child";

fn wait_for_file(path: &Path) {
    let started = Instant::now();
    while !path.is_file() {
        assert!(
            started.elapsed() < WAIT,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[serial]
#[ignore = "fake Cargo child invoked by the reconciliation concurrency witness"]
fn gated_cargo_child() {
    let root = std::env::current_dir().expect("fixture workspace");
    let package = std::env::var("SHIPPER_RECONCILIATION_PACKAGE").expect("package argument");
    assert!(matches!(package.as_str(), "alpha" | "beta"));
    fs::write(root.join(format!("cargo-started-{package}")), b"started").expect("Cargo start");
    wait_for_file(&root.join(format!("release-cargo-{package}")));
    fs::write(root.join(format!("cargo-exited-{package}")), b"exiting").expect("Cargo exit");
    eprintln!("ambiguous publish exit");
    std::process::exit(1);
}

fn write_gated_cargo(bin: &Path) {
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = fake_cargo_path(bin);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '-p' ]; then\n    shift\n    export SHIPPER_RECONCILIATION_PACKAGE=\"$1\"\n    exec \"$SHIPPER_RECONCILIATION_TEST_EXE\" --exact {CHILD} --ignored --nocapture --quiet\n  fi\n  shift\ndone\nexit 97\n"
            ),
        )
        .expect("Cargo proxy");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable proxy");
    }
    #[cfg(windows)]
    fs::write(
        fake_cargo_path(bin),
        format!(
            "@echo off\r\n:scan\r\nif \"%~1\"==\"\" exit /b 97\r\nif \"%~1\"==\"-p\" goto run\r\nshift\r\ngoto scan\r\n:run\r\nset \"SHIPPER_RECONCILIATION_PACKAGE=%~2\"\r\n\"%SHIPPER_RECONCILIATION_TEST_EXE%\" --exact {CHILD} --ignored --nocapture --quiet\r\nexit /b %ERRORLEVEL%\r\n"
        ),
    )
    .expect("Cargo proxy");
}

struct AbsentRegistry {
    base_url: String,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AbsentRegistry {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").expect("loopback registry");
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&seen);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                if let Some(request) = server
                    .recv_timeout(Duration::from_millis(20))
                    .expect("registry request")
                {
                    observed
                        .lock()
                        .expect("observed requests")
                        .push(request.url().to_owned());
                    request
                        .respond(Response::from_string("{}").with_status_code(StatusCode(404)))
                        .expect("registry response");
                }
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

impl Drop for AbsentRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("registry worker");
        }
    }
}

#[derive(Default)]
struct SchedulerReporter {
    warnings: Vec<String>,
}

impl crate::engine::parallel::Reporter for SchedulerReporter {
    fn info(&mut self, _message: &str) {}
    fn warn(&mut self, message: &str) {
        self.warnings.push(message.to_owned());
    }
    fn error(&mut self, _message: &str) {}
}

#[test]
#[serial]
fn reconciliation_report_keeps_both_workers_after_delayed_snapshot() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    let bin = root.join("bin");
    write_fake_tools(&bin);
    write_gated_cargo(&bin);
    let server = AbsentRegistry::start();
    let ws = multi_package_workspace(
        root,
        server.base_url.clone(),
        vec![("alpha", "0.1.0"), ("beta", "0.1.0")],
    );
    let state_dir = root.join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.max_attempts = 1;
    opts.parallel.enabled = true;
    opts.parallel.max_concurrent = 2;
    opts.parallel.per_package_timeout = Duration::from_secs(10);
    opts.readiness.max_total_wait = Duration::ZERO;
    let state = Arc::new(Mutex::new(
        init_state(&ws, &state_dir).expect("initial state"),
    ));
    let events_path = crate::state::events::events_path(&state_dir);
    let mut initial_log = crate::state::events::EventLog::new();
    for event_type in [
        EventType::PlanCreated {
            plan_id: ws.plan.plan_id.clone(),
            package_count: 2,
        },
        EventType::ExecutionStarted,
    ] {
        initial_log.record(PublishEvent {
            timestamp: Utc::now(),
            package: "all".to_owned(),
            event_type,
        });
    }
    initial_log
        .write_to_file(&events_path)
        .expect("initial events");
    initial_log.clear();
    let event_log = Arc::new(Mutex::new(initial_log));
    let reporter = Arc::new(crate::engine::parallel::SendReporter::default());
    let report_path = state::reconciliation_path(&state_dir);
    assert!(
        state_dir.is_dir() && !report_path.exists(),
        "valid unused report destination"
    );

    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let (newer_written_tx, newer_written_rx) = mpsc::channel();
    let _observer = test_hooks::install(
        &state_dir,
        Arc::new(move |phase, report| {
            let packages: Vec<_> = report
                .records
                .iter()
                .map(|record| record.package.as_str())
                .collect();
            if phase == WritePhase::Snapshot && packages == ["alpha@0.1.0"] {
                snapshot_tx.send(()).expect("alpha snapshot notice");
                release_rx
                    .lock()
                    .expect("snapshot release receiver")
                    .recv_timeout(WAIT)
                    .expect("release delayed alpha snapshot");
            }
            if phase == WritePhase::Replaced && packages.contains(&"beta@0.1.0") {
                let _ = newer_written_tx.send(());
            }
        }),
    );

    with_test_env(
        &bin,
        vec![
            (
                "SHIPPER_RECONCILIATION_TEST_EXE",
                Some(
                    std::env::current_exe()
                        .expect("test binary")
                        .display()
                        .to_string(),
                ),
            ),
            (
                "CARGO_REGISTRY_TOKEN",
                Some("reconciliation-fixture-token".to_owned()),
            ),
        ],
        || {
            let reg = init_registry_client(ws.plan.registry.clone(), root, &opts)
                .expect("registry client");
            let worker_ws = ws.clone();
            let worker_state = Arc::clone(&state);
            let worker_log = Arc::clone(&event_log);
            let worker_reporter = Arc::clone(&reporter);
            let worker_state_dir = state_dir.clone();
            let worker_events_path = events_path.clone();
            let (done_tx, done_rx) = mpsc::channel();
            let scheduler = thread::spawn(move || {
                let level = worker_ws
                    .plan
                    .group_by_levels()
                    .into_iter()
                    .next()
                    .expect("parallel level");
                let mut scheduler_reporter = SchedulerReporter::default();
                let result = crate::engine::parallel::publish::run_publish_level(
                    &level,
                    &worker_ws,
                    &opts,
                    &reg,
                    &worker_state,
                    &worker_state_dir,
                    &worker_log,
                    &worker_events_path,
                    &mut scheduler_reporter,
                    &worker_reporter,
                );
                done_tx
                    .send((result, scheduler_reporter.warnings))
                    .expect("scheduler result");
            });

            // Both actual Cargo children must be executing before either is
            // released; serializing whole package execution cannot pass.
            wait_for_file(&root.join("cargo-started-alpha"));
            wait_for_file(&root.join("cargo-started-beta"));
            fs::write(root.join("release-cargo-alpha"), b"finish alpha").expect("release Cargo");
            snapshot_rx
                .recv_timeout(WAIT)
                .expect("alpha event-derived snapshot");

            // Observe whether the snapshot owns the shared persistence gate.
            // This selects a bounded schedule, not the correctness oracle:
            // without a gate beta finishes first, then alpha overwrites it.
            let snapshot_holds_gate = match event_log.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    false
                }
                Err(TryLockError::WouldBlock) => true,
                Err(TryLockError::Poisoned(_)) => panic!("event log poisoned"),
            };
            fs::write(root.join("release-cargo-beta"), b"finish beta").expect("release peer Cargo");
            wait_for_file(&root.join("cargo-exited-beta"));
            if !snapshot_holds_gate {
                newer_written_rx
                    .recv_timeout(WAIT)
                    .expect("newer beta report must finish first");
            }
            release_tx
                .send(())
                .expect("release alpha report replacement");
            let (result, warnings) = done_rx.recv_timeout(WAIT).expect("workers must finish");
            scheduler.join().expect("scheduler thread");
            let error = result.expect_err("both ambiguous uploads stop after conclusive absence");
            assert!(
                error
                    .downcast_ref::<PublishRecoverableStopError>()
                    .is_some(),
                "{error:#}"
            );

            let persisted = state::load_state(&state_dir)
                .expect("read state")
                .expect("state");
            let mut host = CollectingReporter::default();
            crate::engine::parallel::replay_buffered_messages_to_host(&mut host, &reporter);
            assert!(
                warnings
                    .iter()
                    .chain(&host.warns)
                    .all(|message| !message.contains("failed to write reconciliation report")),
                "valid destination produced a report write error: {warnings:?} {:?}",
                host.warns
            );
            let finalized_error = crate::engine::publish::finalize::record_recoverable_stop(
                error,
                &state_dir,
                &events_path,
                &mut event_log.lock().expect("post-join event log"),
                &persisted,
                &mut host,
            )
            .expect("record controlled stop");
            let coherent = finalized_error
                .downcast_ref::<PublishRecoverableStopError>()
                .expect("controlled stop")
                .evidence_consistent;
            let report: ReconciliationReport =
                serde_json::from_slice(&fs::read(&report_path).expect("normal report file"))
                    .expect("parse normal reconciliation report");
            let packages: Vec<_> = report
                .records
                .iter()
                .map(|record| record.package.as_str())
                .collect();
            let events =
                crate::state::events::EventLog::read_from_file(&events_path).expect("read events");
            let reconciled: Vec<_> = events
                .all_events()
                .iter()
                .filter_map(|event| {
                    matches!(event.event_type, EventType::PublishReconciled { .. })
                        .then_some(event.package.as_str())
                })
                .collect();
            eprintln!(
                "concurrent Cargo=true; snapshot gate={snapshot_holds_gate}; report={packages:?}; events={reconciled:?}; controlled-stop consistent={coherent}"
            );
            assert_eq!(reconciled, ["alpha@0.1.0", "beta@0.1.0"]);
            assert_eq!(
                packages, reconciled,
                "older reconciliation snapshot replaced newer package evidence"
            );
            assert!(
                coherent,
                "coherent worker evidence must survive the post-join consistency consumer"
            );
            assert!(report_path.is_file() && !report_path.with_extension("tmp").exists());
            for record in &report.records {
                assert!(matches!(
                    record.outcome,
                    ReconciliationOutcome::NotPublished { .. }
                ));
                assert_eq!(persisted.packages[&record.package].attempts, 1);
            }
            crate::state::consistency::verify_controlled_stop_consistency(
                &events_path,
                &persisted,
                Some(&report),
            )
            .expect("full event/state/reconciliation consistency");
            let final_report = crate::state::reconciliation::write_report_from_events(
                &state_dir,
                &ws.plan.plan_id,
                &ws.plan.registry,
                &events_path,
            )
            .expect("exclusive post-join report generation")
            .expect("report records");
            assert_eq!(final_report.records, report.records);
        },
    );
    let requests = server.seen.lock().expect("registry observations");
    for package in ["alpha", "beta"] {
        assert_eq!(
            requests
                .iter()
                .filter(|path| *path == &format!("/api/v1/crates/{package}/0.1.0"))
                .count(),
            2
        );
    }
}
