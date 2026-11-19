//! Observe progress while the actual fake-Cargo retry is still in flight.

use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

#[test]
#[serial]
#[ignore = "fake Cargo subprocess invoked by live retry tests"]
fn retry_cargo_child() {
    let root = PathBuf::from(std::env::var_os("SHIPPER_414_OBSERVATIONS").unwrap());
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls"))
        .unwrap();
    writeln!(calls, "publish").unwrap();
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("first"))
        .is_ok()
    {
        eprintln!("error: connection reset by peer");
        std::process::exit(101);
    }
    // The test observes ordering through a process-visible marker, not the
    // final captured output or a guessed amount of wall-clock elapsed time.
    fs::write(
        root.join("notice-before-second"),
        root.join("notice").exists().to_string(),
    )
    .unwrap();
    fs::write(root.join("second"), "started").unwrap();
    if std::env::var_os("SHIPPER_414_FINAL_FAILURE").is_some() {
        eprintln!("error: permission denied");
        std::process::exit(101);
    }
    fs::write(root.join("published"), "complete").unwrap();
    std::process::exit(0);
}

struct RegistryFixture {
    base_url: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RegistryFixture {
    fn start(root: &Path) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", server.server_addr());
        let published = root.join("published");
        let stop = Arc::new(AtomicBool::new(false));
        let observed_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !observed_stop.load(Ordering::Acquire) {
                let Some(request) = server.recv_timeout(Duration::from_millis(10)).unwrap() else {
                    continue;
                };
                assert_eq!(request.url(), "/api/v1/crates/demo/0.1.0");
                let status = if published.exists() { 200 } else { 404 };
                request
                    .respond(Response::from_string("{}").with_status_code(status))
                    .unwrap();
            }
        });
        Self {
            base_url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

struct LiveReporter {
    observations: PathBuf,
    messages: Vec<(String, String)>,
    remaining: Vec<Duration>,
    slow_retry_callback: bool,
    disconnected_sink: Option<std::sync::mpsc::Sender<String>>,
    // Embedders are allowed to keep their reporter on its calling thread.
    _not_send: Rc<()>,
}

impl LiveReporter {
    fn record(&mut self, kind: &str, message: &str) {
        self.messages.push((kind.into(), message.into()));
        if let Some(sink) = &self.disconnected_sink {
            assert!(sink.send(message.to_owned()).is_err());
        }
    }
}

impl Reporter for LiveReporter {
    fn info(&mut self, message: &str) {
        self.record("info", message);
    }

    fn warn(&mut self, message: &str) {
        self.record("warn", message);
    }

    fn error(&mut self, message: &str) {
        self.record("error", message);
    }

    fn retry_wait(
        &mut self,
        _package: &str,
        _version: &str,
        _attempt: u32,
        _max_attempts: u32,
        delay: Duration,
        _reason: ErrorClass,
        message: &str,
    ) {
        self.record("retry", message);
        self.remaining.push(delay);
        fs::write(self.observations.join("notice"), "observed").unwrap();
        if self.slow_retry_callback {
            let started = std::time::Instant::now();
            while !self.observations.join("second").exists() {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "the worker must complete its own backoff while reporting is blocked"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn assert_live_retry(
    parallel: bool,
    slow_retry_callback: bool,
    disconnected: bool,
    final_failure: bool,
) {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("bin");
    write_fake_tools(&bin);
    #[cfg(not(windows))]
    fs::write(
        fake_cargo_path(&bin),
        "#!/bin/sh\nexec \"$SHIPPER_414_TEST_BIN\" --ignored --exact engine::tests::live_retry::retry_cargo_child --nocapture\n",
    )
    .unwrap();
    #[cfg(windows)]
    fs::write(
        fake_cargo_path(&bin),
        "@echo off\r\n\"%SHIPPER_414_TEST_BIN%\" --ignored --exact engine::tests::live_retry::retry_cargo_child --nocapture\r\nexit /b %errorlevel%\r\n",
    )
    .unwrap();
    let registry = RegistryFixture::start(dir.path());
    let ws = planned_workspace(dir.path(), registry.base_url.clone());
    let state_dir = dir.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = parallel;
    opts.max_attempts = 2;
    opts.base_delay = Duration::from_millis(500);
    opts.max_delay = Duration::from_millis(500);
    opts.retry_jitter = 0.0;
    opts.parallel.per_package_timeout = Duration::from_secs(5);
    let mut environment = fake_program_env_vars(&bin);
    environment.extend([
        (
            "SHIPPER_414_TEST_BIN",
            Some(std::env::current_exe().unwrap().display().to_string()),
        ),
        (
            "SHIPPER_414_OBSERVATIONS",
            Some(dir.path().display().to_string()),
        ),
        (
            "CARGO_HOME",
            Some(dir.path().join("cargo-home").display().to_string()),
        ),
        ("CARGO_REGISTRY_TOKEN", None),
        ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
        (
            "SHIPPER_414_FINAL_FAILURE",
            final_failure.then(|| "1".into()),
        ),
    ]);
    temp_env::with_vars(environment, || {
        let (sink, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        let mut reporter = LiveReporter {
            observations: dir.path().to_owned(),
            messages: Vec::new(),
            remaining: Vec::new(),
            slow_retry_callback,
            disconnected_sink: disconnected.then_some(sink),
            _not_send: Rc::new(()),
        };
        let result = run_publish(&ws, &opts, &mut reporter);
        if final_failure {
            let error = result.expect_err("worker's terminal failure must reach the host");
            assert!(format!("{error:#}").contains("permanent failure"));
            assert!(!state::receipt_path(&state_dir).exists());
        } else {
            let receipt = result.unwrap();
            assert_eq!(receipt.packages[0].state, PackageState::Published);
        }
        assert_eq!(
            fs::read_to_string(dir.path().join("calls"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        let state = state::load_state(&state_dir).unwrap().unwrap();
        if final_failure {
            assert!(matches!(
                state.packages["demo@0.1.0"].state,
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    ..
                }
            ));
        } else {
            assert_eq!(state.packages["demo@0.1.0"].state, PackageState::Published);
        }
        assert_eq!(state.packages["demo@0.1.0"].attempts, 2);
        let log = events::EventLog::read_from_file(&events::events_path(&state_dir)).unwrap();
        let delays: Vec<_> = log
            .all_events()
            .iter()
            .filter_map(|event| match event.event_type {
                EventType::RetryBackoffStarted { delay_ms, .. } => {
                    Some(Duration::from_millis(delay_ms))
                }
                _ => None,
            })
            .collect();
        assert_eq!(delays.len(), 1);
        assert_eq!(reporter.remaining.len(), 1);
        assert!(reporter.remaining[0] <= delays[0]);
        assert!(
            reporter.remaining[0] > Duration::ZERO,
            "notice was delivered after the active retry wait: {:?}",
            reporter.remaining
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("notice-before-second")).unwrap(),
            "true",
            "fake Cargo began its second attempt before the host received the retry notice"
        );
        let failed = reporter
            .messages
            .iter()
            .position(|(kind, message)| kind == "warn" && message.contains("cargo publish failed"))
            .expect("first-attempt failure warning");
        let retry = reporter
            .messages
            .iter()
            .position(|(kind, _)| kind == "retry")
            .unwrap();
        let terminal = if final_failure {
            reporter
                .messages
                .iter()
                .rposition(|(kind, message)| {
                    kind == "warn" && message.contains("cargo publish failed")
                })
                .expect("second-attempt failure warning")
        } else {
            reporter
                .messages
                .iter()
                .position(|(kind, message)| {
                    kind == "info" && message.contains("cargo publish exited successfully")
                })
                .expect("second-attempt success information")
        };
        assert!(
            failed < retry && retry < terminal,
            "causal order lost: {:?}",
            reporter.messages
        );
    });
}

#[test]
#[serial]
fn live_retry_sequential_notice_precedes_second_cargo_attempt() {
    assert_live_retry(false, false, false, false);
}

#[test]
#[serial]
fn live_retry_parallel_notice_precedes_second_cargo_attempt() {
    assert_live_retry(true, false, false, false);
}

#[test]
#[serial]
fn live_retry_slow_host_callback_does_not_own_worker_sleep_or_change_outcome() {
    for parallel in [false, true] {
        assert_live_retry(parallel, true, false, false);
    }
}

#[test]
#[serial]
fn live_retry_disconnected_consumer_does_not_change_success_or_hide_worker_failure() {
    for parallel in [false, true] {
        for final_failure in [false, true] {
            assert_live_retry(parallel, true, true, final_failure);
        }
    }
}
