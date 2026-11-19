//! Prove that both schedulers use the concurrently drained Cargo process path.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::Value;
use tiny_http::{Response, Server, StatusCode};

const STDOUT_SECRET: &str = "noisy-stdout-secret-sentinel";
const STDERR_SECRET: &str = "noisy-stderr-secret-sentinel";
#[cfg(windows)]
const TIMEOUT_ENV: &str = "SHIPPER_WINDOWS_FIXTURE_TIMEOUT";
#[cfg(windows)]
const EXTERNAL_PARENT_ENV: &str = "SHIPPER_WINDOWS_FIXTURE_EXTERNAL_PARENT";

#[test]
#[serial_test::serial]
#[ignore = "fake Cargo subprocess invoked by the noisy-output CLI tests"]
fn noisy_cargo_child() {
    // The old runner can block this helper behind a full pipe. Its own watchdog
    // bounds a failing control even on platforms where a command script leaves
    // this process running after the script itself is killed.
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(15));
        std::process::exit(99);
    });
    let root = PathBuf::from(std::env::var_os("SHIPPER_NOISY_FIXTURE_ROOT").expect("fixture root"));
    let mut calls = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("publish-calls"))
        .expect("publish calls");
    writeln!(calls, "publish").expect("record publish");
    #[cfg(windows)]
    if std::env::var_os(TIMEOUT_ENV).is_some() {
        let fixture = std::env::current_exe().unwrap();
        let args = ["--ignored", "--exact", "inherited_pipe_holder"];
        let contained_child = if let Ok(parent) = std::env::var(EXTERNAL_PARENT_ENV) {
            // Deliberately put the test pipe holder outside Shipper's job,
            // using our own test harness as its logical parent.
            let parent = windows_spawn::ParentProcess::open(parent.parse().unwrap()).unwrap();
            windows_spawn::Command::new(fixture)
                .args(args)
                .stdin(windows_spawn::Stdio::null())
                .stdout(windows_spawn::Stdio::from_borrowed(&std::io::stdout()).unwrap())
                .stderr(windows_spawn::Stdio::from_borrowed(&std::io::stderr()).unwrap())
                .spawn_with(windows_spawn::SpawnOptions::new().parent_process(&parent))
                .unwrap();
            None
        } else {
            Some(
                std::process::Command::new(fixture)
                    .args(args)
                    .spawn()
                    .unwrap(),
            )
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !root.join("pipe-holder-ready").exists() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        eprintln!("Token={STDERR_SECRET}");
        let Some(mut contained_child) = contained_child else {
            std::process::exit(0);
        };
        fs::write(root.join("unknown-registry"), "").unwrap();
        // The contained child keeps the pipes open while the parent waits.
        // The fixture watchdog still bounds a broken supervision run.
        contained_child.wait().unwrap();
        std::process::exit(98);
    }
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let oversized = std::env::var_os("SHIPPER_NOISY_OVERSIZED").is_some();
    let separator = if oversized { "" } else { "\n" };
    let out = format!("{}{separator}", "O".repeat(4095));
    let err = format!("{}{separator}", "E".repeat(4095));
    if oversized {
        write!(stdout, "Authorization: Bearer ").unwrap();
        write!(stderr, "Token=").unwrap();
    }
    for _ in 0..256 {
        stdout.write_all(out.as_bytes()).unwrap();
        stderr.write_all(err.as_bytes()).unwrap();
    }
    writeln!(stdout, "Authorization: Bearer {STDOUT_SECRET}").unwrap();
    writeln!(stdout, "stdout complete").unwrap();
    writeln!(stderr, "Token={STDERR_SECRET}").unwrap();
    writeln!(stderr, "stderr complete").unwrap();
    stdout.flush().unwrap();
    stderr.flush().unwrap();
    fs::write(root.join("published"), "complete").expect("publish marker");
    std::process::exit(0);
}

#[cfg(windows)]
#[test]
#[serial_test::serial]
#[ignore = "fake Cargo descendant with inherited stdout and stderr"]
fn inherited_pipe_holder() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = PathBuf::from(std::env::var_os("SHIPPER_NOISY_FIXTURE_ROOT").unwrap());
    let _owned = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(0)
        .open(root.join("pipe-holder-handle"))
        .unwrap();
    fs::write(root.join("pipe-holder-ready"), "").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline && !root.join("release-pipe-holder").exists() {
        thread::sleep(Duration::from_millis(15));
    }
}

struct Registry {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<(usize, usize)>>,
}

impl Registry {
    fn start(root: &Path) -> Self {
        let server = Server::http("127.0.0.1:0").expect("loopback registry");
        let base_url = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let published = root.join("published");
        let unknown = root.join("unknown-registry");
        let handle = thread::spawn(move || {
            let (mut missing, mut present) = (0, 0);
            while !worker_stop.load(Ordering::Acquire) {
                let Some(request) = server
                    .recv_timeout(Duration::from_millis(50))
                    .expect("registry request")
                else {
                    continue;
                };
                assert_eq!(request.url(), "/api/v1/crates/demo/0.1.0");
                let status = if unknown.is_file() {
                    503
                } else if published.is_file() {
                    present += 1;
                    200
                } else {
                    missing += 1;
                    404
                };
                request
                    .respond(Response::from_string("{}").with_status_code(StatusCode(status)))
                    .expect("registry response");
            }
            (missing, present)
        });
        Self {
            base_url,
            stop,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> (usize, usize) {
        self.stop.store(true, Ordering::Release);
        self.handle.take().unwrap().join().expect("registry worker")
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn create_fixture(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("demo/src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"demo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        root.join("demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("demo/src/lib.rs"), "pub fn demo() {}\n").unwrap();

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let proxy = root.join("fake-cargo");
        fs::write(
            &proxy,
            "#!/bin/sh\ncase \"$1\" in\n  publish) exec \"$SHIPPER_NOISY_FIXTURE_EXE\" --exact noisy_cargo_child --ignored --nocapture ;;\n  metadata|--version|-V) exec \"$REAL_CARGO\" \"$@\" ;;\n  *) exit 88 ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&proxy, fs::Permissions::from_mode(0o755)).unwrap();
        proxy
    }
    #[cfg(windows)]
    {
        let proxy = root.join("fake-cargo.cmd");
        fs::write(
            &proxy,
            "@echo off\r\nif /i \"%~1\"==\"publish\" goto :publish\r\nif /i \"%~1\"==\"metadata\" goto :cargo\r\nif /i \"%~1\"==\"--version\" goto :cargo\r\nif /i \"%~1\"==\"-V\" goto :cargo\r\nexit /b 88\r\n:publish\r\nstart \"\" /b /wait \"%SHIPPER_NOISY_FIXTURE_EXE%\" --exact noisy_cargo_child --ignored --nocapture\r\nexit /b %errorlevel%\r\n:cargo\r\n\"%REAL_CARGO%\" %*\r\nexit /b %errorlevel%\r\n",
        )
        .unwrap();
        proxy
    }
}

fn cargo_fixture_command(
    root: &Path,
    proxy: &Path,
    registry: &Registry,
    parallel: bool,
    timeout: &str,
    attempts: &str,
) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"));
    command
        .timeout(Duration::from_secs(20))
        .current_dir(root)
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--state-dir")
        .arg(root.join("state"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .args([
            "--allow-loopback",
            "--allow-dirty",
            "--skip-ownership-check",
            "--no-verify",
            "--max-attempts",
            attempts,
            "--output-lines",
            "2",
            "--per-package-timeout",
            timeout,
            "--readiness-method",
            "api",
            "--readiness-poll",
            "0ms",
            "--readiness-timeout",
            "1s",
        ])
        .env("SHIPPER_CARGO_BIN", proxy)
        .env(
            "REAL_CARGO",
            std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        )
        .env(
            "SHIPPER_NOISY_FIXTURE_EXE",
            std::env::current_exe().unwrap(),
        )
        .env("SHIPPER_NOISY_FIXTURE_ROOT", root)
        .env("CARGO_NET_OFFLINE", "true");
    if parallel {
        command.args(["--parallel", "--max-concurrent", "2"]);
    }
    command
}

fn publish_with_noisy_cargo(parallel: bool, oversized: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let proxy = create_fixture(root);
    let registry = Registry::start(root);
    let state_dir = root.join("state");
    let mut command = cargo_fixture_command(root, &proxy, &registry, parallel, "5s", "1");
    if oversized {
        command.env("SHIPPER_NOISY_OVERSIZED", "1");
    } else {
        command.env_remove("SHIPPER_NOISY_OVERSIZED");
    }
    let output = command.arg("publish").output();
    let requests = registry.finish();
    let output = output.unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        requests.0 > 0 && requests.1 > 0,
        "registry phases: {requests:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("publish-calls")).unwrap(),
        "publish\n"
    );

    let receipt_text = fs::read_to_string(state_dir.join("receipt.json")).unwrap();
    let receipt: Value = serde_json::from_str(&receipt_text).unwrap();
    assert_eq!(receipt["execution_result"], "success");
    let package = &receipt["packages"][0];
    assert_eq!(package["state"]["state"], "published");
    let attempts = package["evidence"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    let attempt = &attempts[0];
    assert_eq!(attempt["exit_code"], 0);
    if oversized {
        assert_eq!(attempt["stdout_tail"], "");
        assert_eq!(
            attempt["stderr_tail"],
            "[shipper: Cargo diagnostics omitted after capture loss]"
        );
    } else {
        assert_eq!(
            attempt["stdout_tail"],
            "Authorization: [REDACTED]\nstdout complete"
        );
        assert_eq!(attempt["stderr_tail"], "Token=[REDACTED]\nstderr complete");
    }

    let events_text = fs::read_to_string(state_dir.join("events.jsonl")).unwrap();
    let events: Vec<Value> = events_text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let outputs: Vec<_> = events
        .iter()
        .filter(|event| event["event_type"]["type"] == "package_output")
        .collect();
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0]["event_type"]["stdout_tail"],
        attempt["stdout_tail"]
    );
    assert_eq!(
        outputs[0]["event_type"]["stderr_tail"],
        attempt["stderr_tail"]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"]["type"] == "package_attempted")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"]["type"] == "package_published")
            .count(),
        1
    );

    let state_text = fs::read_to_string(state_dir.join("state.json")).unwrap();
    let state: Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(state["packages"]["demo@0.1.0"]["attempts"], 1);
    assert_eq!(
        state["packages"]["demo@0.1.0"]["state"]["state"],
        "published"
    );
    for text in [
        receipt_text,
        events_text,
        state_text,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ] {
        assert!(!text.contains(STDOUT_SECRET) && !text.contains(STDERR_SECRET));
        assert!(!text.contains("timed out"));
    }
}

#[test]
#[serial_test::serial]
fn sequential_publish_captures_noisy_cargo_without_false_timeout() {
    publish_with_noisy_cargo(false, false);
}

#[test]
#[serial_test::serial]
fn parallel_publish_captures_noisy_cargo_without_false_timeout() {
    publish_with_noisy_cargo(true, false);
}

#[test]
#[serial_test::serial]
fn sequential_publish_omits_oversized_lines_without_changing_success() {
    publish_with_noisy_cargo(false, true);
}

#[test]
#[serial_test::serial]
fn parallel_publish_omits_oversized_lines_without_changing_success() {
    publish_with_noisy_cargo(true, true);
}

#[cfg(windows)]
#[test]
#[serial_test::serial]
fn windows_timeout_with_inherited_pipes_preserves_ambiguous_event_authority() {
    use shipper_core::state::{consistency, execution_state, rebuild};
    use shipper_core::types::{
        ErrorClass, EventType, PackageState, PublishEvent, ReconciliationOutcome,
    };

    for parallel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let proxy = create_fixture(root);
        let registry = Registry::start(root);
        let state_dir = root.join("state");
        let event_path = state_dir.join("events.jsonl");
        let run = |action| {
            cargo_fixture_command(root, &proxy, &registry, parallel, "3s", "3")
                .env(TIMEOUT_ENV, "1")
                .arg(action)
                .output()
                .unwrap()
        };

        let assert_ambiguous_checkpoint = || {
            let state = execution_state::load_state(&state_dir).unwrap().unwrap();
            let key = "demo@0.1.0";
            let progress = &state.packages[key];
            assert!(
                matches!(progress.state, PackageState::Ambiguous { .. }),
                "{progress:?}"
            );
            assert_eq!(progress.attempts, 1);
            let [attempt] = state.attempt_history.as_slice() else {
                panic!(
                    "expected one durable attempt, found {:?}",
                    state.attempt_history
                );
            };
            assert_eq!(
                (&*attempt.package, &*attempt.version, attempt.attempt),
                ("demo", "0.1.0", 1)
            );
            assert_eq!(attempt.error_class, Some(ErrorClass::Ambiguous));
            assert!(attempt.next_attempt_at.is_none());

            let rebuilt = rebuild::rebuild_state_from_events(
                &event_path,
                rebuild::StateRebuildOptions::new(state.registry.clone()),
            )
            .unwrap();
            assert_eq!(rebuilt.packages[key].state, progress.state);
            assert_eq!(rebuilt.packages[key].attempts, progress.attempts);
            assert_eq!(rebuilt.attempt_history, state.attempt_history);
            assert!(
                consistency::verify_events_state_consistency(&event_path, &state)
                    .unwrap()
                    .is_consistent()
            );
            state.attempt_history
        };

        let output = run("publish");
        assert!(!output.status.success(), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains(STDERR_SECRET));
        assert!(fs::File::open(root.join("pipe-holder-handle")).is_ok());
        let initial = fs::read_to_string(&event_path).unwrap();
        assert!(!initial.contains(STDERR_SECRET));
        assert!(initial.contains("timed out"));
        let events = initial
            .lines()
            .map(|line| serde_json::from_str::<PublishEvent>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event.event_type,
            EventType::PublishReconciled {
                outcome: ReconciliationOutcome::StillUnknown { .. }
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event.event_type,
            EventType::PackageCleanupIncomplete { .. } | EventType::PackagePublished { .. }
        )));

        let attempts_after_publish = assert_ambiguous_checkpoint();
        let resumed = run("resume");
        assert!(!resumed.status.success(), "{resumed:?}");
        assert_eq!(
            fs::read_to_string(root.join("publish-calls")).unwrap(),
            "publish\n"
        );
        assert!(
            fs::read_to_string(&event_path)
                .unwrap()
                .starts_with(&initial)
        );
        assert_eq!(assert_ambiguous_checkpoint(), attempts_after_publish);
    }
}

#[cfg(windows)]
#[test]
#[serial_test::serial]
fn windows_open_pipe_records_a_hold_without_a_duplicate_upload() {
    use shipper_core::types::{EventType, PackageState, PublishEvent};

    for parallel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let proxy = create_fixture(root);
        let registry = Registry::start(root);
        let state_dir = root.join("state");
        let event_path = state_dir.join("events.jsonl");
        let run = |action| {
            cargo_fixture_command(root, &proxy, &registry, parallel, "5s", "3")
                .env(TIMEOUT_ENV, "1")
                .env(EXTERNAL_PARENT_ENV, std::process::id().to_string())
                .arg(action)
                .output()
                .unwrap()
        };
        let output = run("publish");
        let holder = root.join("pipe-holder-handle");
        let holder_survived = fs::File::open(&holder).unwrap_err().raw_os_error() == Some(32);
        fs::write(root.join("release-pipe-holder"), "").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while fs::File::open(&holder).is_err() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(15));
        }
        assert!(holder_survived);
        assert!(!output.status.success(), "{output:?}");
        let events = fs::read_to_string(&event_path).unwrap();
        assert!(!events.contains(STDERR_SECRET));
        let records = events
            .lines()
            .map(|line| serde_json::from_str::<PublishEvent>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            records.iter().any(|event| matches!(
                event.event_type,
                EventType::PackageCleanupIncomplete { .. }
            ))
        );
        assert!(!records.iter().any(|event| matches!(
            event.event_type,
            EventType::PackagePublished { .. } | EventType::PublishReconciled { .. }
        )));
        let state = shipper_core::state::execution_state::load_state(&state_dir)
            .unwrap()
            .unwrap();
        assert!(matches!(
            state.packages["demo@0.1.0"].state,
            PackageState::CleanupIncomplete { .. }
        ));
        assert!(
            shipper_core::state::consistency::verify_events_state_consistency(&event_path, &state)
                .unwrap()
                .is_consistent()
        );
        for action in ["publish", "resume"] {
            let refused = run(action);
            assert!(!refused.status.success(), "{refused:?}");
            assert_eq!(fs::read_to_string(&event_path).unwrap(), events);
            assert_eq!(
                fs::read_to_string(root.join("publish-calls")).unwrap(),
                "publish\n"
            );
        }
    }
}
