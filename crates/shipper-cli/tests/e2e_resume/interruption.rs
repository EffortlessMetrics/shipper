//! Actual process-only CLI signal handling and fresh-process recovery.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

const CARGO_FIXTURE: &str = "interruption::interrupted_cargo_fixture";

#[test]
#[serial]
#[ignore = "finite fake Cargo process invoked only by signal witnesses"]
fn interrupted_cargo_fixture() {
    let package = std::env::var("SHIPPER_SIGNAL_PACKAGE").unwrap();
    if Path::new("complete-cargo").exists() {
        fs::write("cargo-succeeded", "published").unwrap();
        return;
    }
    let descendant_role = std::env::var_os("SHIPPER_SIGNAL_DESCENDANT_ROLE").is_some();
    let mut descendant = if !descendant_role {
        std::env::var("SHIPPER_SIGNAL_DESCENDANT").ok().map(|mode| {
            let mut command = ProcessCommand::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", CARGO_FIXTURE, "--ignored", "--nocapture"])
                .env("SHIPPER_SIGNAL_DESCENDANT_ROLE", "yes")
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            if mode == "escaped" {
                command.process_group(0);
            }
            let child = command.spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !Path::new(&format!("descendant-pid-{package}")).exists() {
                assert!(Instant::now() < deadline, "descendant did not start");
                thread::sleep(Duration::from_millis(5));
            }
            child
        })
    } else {
        None
    };
    fs::write(
        format!(
            "{}-pid-{package}",
            if descendant_role {
                "descendant"
            } else {
                "cargo"
            }
        ),
        std::process::id().to_string(),
    )
    .unwrap();
    println!("interrupted stdout sentinel");
    eprintln!("interrupted stderr sentinel");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !Path::new("stop-cargo").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if let Some(child) = descendant.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct RunningChild {
    child: Option<Child>,
    root: std::path::PathBuf,
}
impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = fs::write(self.root.join("stop-cargo"), "stop");
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Give finite orphan fixtures time to observe their stop file before
        // TempDir removes it, including negative controls where Shipper exits
        // by the default signal action before owning-group cleanup runs.
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let running = fs::read_dir(&self.root)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !name.starts_with("cargo-pid-") && !name.starts_with("descendant-pid-") {
                        return false;
                    }
                    fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|pid| fs::read_to_string(format!("/proc/{pid}/stat")).ok())
                        .is_some_and(|stat| !stat.rsplit_once(") ").unwrap().1.starts_with('Z'))
                });
            if !running {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

struct RegistryFixture {
    address: String,
    published: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl RegistryFixture {
    fn start(root: &Path) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = format!("http://{}", server.server_addr());
        let published = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (visible, stopping) = (published.clone(), stop.clone());
        let cargo_succeeded = root.join("cargo-succeeded");
        let thread = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if let Some(request) = server.recv_timeout(Duration::from_millis(25)).unwrap() {
                    let status = if visible.load(Ordering::SeqCst) || cargo_succeeded.exists() {
                        200
                    } else {
                        404
                    };
                    let _ = request.respond(Response::from_string("{}").with_status_code(status));
                }
            }
        });
        Self {
            address,
            published,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for RegistryFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn executable() -> std::path::PathBuf {
    // The optional override lets the same witness qualify the install facade
    // locally; the ordinary CI route always exercises shipper-cli.
    std::env::var_os("SHIPPER_TEST_INTERRUPT_EXE")
        .map(Into::into)
        .unwrap_or_else(|| assert_cmd::cargo::cargo_bin!("shipper-cli").into())
}

fn command(root: &Path, registry: &str, action: &str, parallel: bool) -> ProcessCommand {
    command_with_budget(root, registry, action, parallel, "1")
}

fn command_with_budget(
    root: &Path,
    registry: &str,
    action: &str,
    parallel: bool,
    max_attempts: &str,
) -> ProcessCommand {
    let mut command = ProcessCommand::new(executable());
    command
        .current_dir(root)
        .args([
            "--allow-loopback",
            "--allow-dirty",
            "--no-verify",
            "--api-base",
            registry,
            "--verify-timeout",
            "0ms",
            "--verify-poll",
            "0ms",
            "--readiness-timeout",
            "0ms",
            "--readiness-poll",
            "0ms",
            "--max-attempts",
            max_attempts,
            "--base-delay",
            "0ms",
            "--format",
            "json",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--state-dir")
        .arg(root.join(".shipper"))
        .env("SHIPPER_CARGO_BIN", root.join("fake-bin/cargo"))
        .env("REAL_CARGO", std::env::var("CARGO").unwrap())
        .env("SHIPPER_SIGNAL_FIXTURE", std::env::current_exe().unwrap())
        .env("CARGO_REGISTRY_TOKEN", "signal-fixture-token")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if parallel {
        command.args(["--max-concurrent", "2"]);
    }
    command.arg(action);
    command
}

fn create_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"alpha\", \"beta\", \"gamma\"]\nresolver = \"2\"\n",
    );
    for package in ["alpha", "beta", "gamma"] {
        write_file(
            &root.join(format!("{package}/Cargo.toml")),
            &format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write_file(
            &root.join(format!("{package}/src/lib.rs")),
            "pub fn sample() {}\n",
        );
    }
    let cargo = root.join("fake-bin/cargo");
    write_file(
        &cargo,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = publish ]; then\n  printf '%s\\n' \"$3\" >> cargo-calls\n  export SHIPPER_SIGNAL_PACKAGE=\"$3\"\n  exec \"$SHIPPER_SIGNAL_FIXTURE\" --exact {CARGO_FIXTURE} --ignored --nocapture\nfi\nif [ \"$1\" = metadata ] && [ \"$SHIPPER_SIGNAL_BLOCK_METADATA\" = yes ]; then\n  export SHIPPER_SIGNAL_PACKAGE=metadata\n  exec \"$SHIPPER_SIGNAL_FIXTURE\" --exact {CARGO_FIXTURE} --ignored --nocapture\nfi\nexec \"$REAL_CARGO\" \"$@\"\n"
        ),
    );
    fs::set_permissions(cargo, fs::Permissions::from_mode(0o755)).unwrap();
}

fn wait_for_files(root: &Path, names: &[&str], child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while names.iter().any(|name| !root.join(name).exists()) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before fixture startup"
        );
        assert!(
            Instant::now() < deadline,
            "fixture startup deadline exceeded"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn bounded_output(running: &mut RunningChild) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while running
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "CLI did not finish bounded interruption"
        );
        thread::sleep(Duration::from_millis(10));
    }
    running.child.take().unwrap().wait_with_output().unwrap()
}

fn run_bounded(mut command: ProcessCommand) -> std::process::Output {
    let mut running = RunningChild {
        root: command.get_current_dir().unwrap().to_path_buf(),
        child: Some(command.spawn().unwrap()),
    };
    bounded_output(&mut running)
}

fn send_signal(running: &RunningChild, signal: &str) {
    assert!(
        ProcessCommand::new("kill")
            .args([
                "-s",
                signal,
                &running.child.as_ref().unwrap().id().to_string()
            ])
            .status()
            .unwrap()
            .success()
    );
}

fn failure_report(output: &std::process::Output) -> serde_json::Value {
    assert!(!output.status.success());
    assert_eq!(
        output.status.signal(),
        None,
        "signal must return through the CLI: {output:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "failed publish cannot fabricate success output"
    );
    serde_json::from_slice(&output.stderr)
        .unwrap_or_else(|error| panic!("invalid failure report: {error}; {output:?}"))
}

#[test]
#[serial]
fn termination_signals_cleanup_stop_and_resume_without_duplicate_upload() {
    for (signal, parallel) in [
        ("INT", false),
        ("TERM", false),
        ("HUP", false),
        ("TERM", true),
    ] {
        let td = tempdir().unwrap();
        create_workspace(td.path());
        let registry = RegistryFixture::start(td.path());
        let mut running = RunningChild {
            child: Some(
                command(td.path(), &registry.address, "publish", parallel)
                    .spawn()
                    .unwrap(),
            ),
            root: td.path().to_path_buf(),
        };
        let names = if parallel {
            vec!["cargo-pid-alpha", "cargo-pid-beta"]
        } else {
            vec!["cargo-pid-alpha"]
        };
        wait_for_files(td.path(), &names, running.child.as_mut().unwrap());
        let start = Instant::now();
        send_signal(&running, signal);
        let output = bounded_output(&mut running);
        assert!(start.elapsed() < Duration::from_secs(4));
        let report = failure_report(&output);
        assert_eq!(report["category"], "interrupted", "{output:?}");
        let calls_before = fs::read(td.path().join("cargo-calls")).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&calls_before).lines().count(),
            names.len()
        );
        for name in names {
            let pid = fs::read_to_string(td.path().join(name)).unwrap();
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "owned child was not reaped"
            );
        }
        let state_dir = td.path().join(".shipper");
        let state = shipper_core::state::execution_state::load_state(&state_dir)
            .unwrap()
            .unwrap();
        for progress in state
            .packages
            .values()
            .filter(|progress| progress.attempts > 0)
        {
            assert!(matches!(
                progress.state,
                shipper_core::types::PackageState::Failed {
                    class: shipper_core::types::ErrorClass::Ambiguous,
                    ..
                }
            ));
        }
        assert!(!state_dir.join("receipt.json").exists());
        let events_before = fs::read(state_dir.join("events.jsonl")).unwrap();
        assert!(String::from_utf8_lossy(&events_before).contains("interrupted stdout sentinel"));
        assert!(
            shipper_core::state::consistency::verify_events_state_consistency(
                &state_dir.join("events.jsonl"),
                &state
            )
            .unwrap()
            .is_consistent()
        );

        registry.published.store(true, Ordering::SeqCst);
        for _ in 0..2 {
            let resumed = run_bounded(command(td.path(), &registry.address, "resume", parallel));
            assert!(resumed.status.success(), "{resumed:?}");
            assert_eq!(
                fs::read(td.path().join("cargo-calls")).unwrap(),
                calls_before
            );
            assert!(
                fs::read(state_dir.join("events.jsonl"))
                    .unwrap()
                    .starts_with(&events_before)
            );
        }
    }
}

#[test]
#[serial]
fn termination_with_descendants_preserves_truthful_cleanup_precedence() {
    use shipper_core::types::PackageState;

    for (signal, mode) in [
        ("INT", "owned"),
        ("TERM", "owned"),
        ("HUP", "owned"),
        ("TERM", "escaped"),
    ] {
        let td = tempdir().unwrap();
        create_workspace(td.path());
        let registry = RegistryFixture::start(td.path());
        let mut running = RunningChild {
            child: Some(
                command(td.path(), &registry.address, "publish", false)
                    .env("SHIPPER_SIGNAL_DESCENDANT", mode)
                    .spawn()
                    .unwrap(),
            ),
            root: td.path().to_path_buf(),
        };
        wait_for_files(
            td.path(),
            &["cargo-pid-alpha"],
            running.child.as_mut().unwrap(),
        );
        let start = Instant::now();
        send_signal(&running, signal);
        if mode == "escaped" {
            // Its inherited pipe necessarily holds the one-second cleanup
            // window open. Repeated signals must not extend that deadline or
            // replace the stronger cleanup-uncertainty result.
            for _ in 0..3 {
                thread::sleep(Duration::from_millis(25));
                send_signal(&running, signal);
            }
        }
        let output = bounded_output(&mut running);
        assert!(start.elapsed() < Duration::from_millis(2500));
        let report = failure_report(&output);
        let state_dir = td.path().join(".shipper");
        let state_bytes = fs::read(state_dir.join("state.json")).unwrap();
        let events_bytes = fs::read(state_dir.join("events.jsonl")).unwrap();
        let state = shipper_core::state::execution_state::load_state(&state_dir)
            .unwrap()
            .unwrap();
        let held = matches!(
            state.packages["alpha@0.1.0"].state,
            PackageState::CleanupIncomplete { .. }
        );
        assert_eq!(
            report["category"],
            if held {
                "cleanup_incomplete"
            } else {
                "interrupted"
            }
        );
        assert_eq!(
            fs::read_to_string(td.path().join("cargo-calls")).unwrap(),
            "alpha\n"
        );
        assert!(!state_dir.join("receipt.json").exists());
        assert!(
            shipper_core::state::consistency::verify_events_state_consistency(
                &state_dir.join("events.jsonl"),
                &state
            )
            .unwrap()
            .is_consistent()
        );

        let leader = fs::read_to_string(td.path().join("cargo-pid-alpha")).unwrap();
        assert!(!Path::new(&format!("/proc/{leader}")).exists());
        let descendant = fs::read_to_string(td.path().join("descendant-pid-alpha")).unwrap();
        if mode == "escaped" {
            assert!(
                held,
                "an inherited pipe is unconfirmed cleanup even after an interrupt"
            );
            assert!(Path::new(&format!("/proc/{descendant}")).exists());
        } else if let Ok(stat) = fs::read_to_string(format!("/proc/{descendant}/stat")) {
            assert!(
                held,
                "a still-visible group member requires a retained hold"
            );
            assert!(
                stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
                "owned descendant still running: {stat}"
            );
        }
        if held {
            for action in ["publish", "resume", "clean"] {
                let refused = run_bounded(command(td.path(), &registry.address, action, false));
                assert!(!refused.status.success(), "{action}: {refused:?}");
                assert!(String::from_utf8_lossy(&refused.stderr).contains("cleanup"));
                assert_eq!(fs::read(state_dir.join("state.json")).unwrap(), state_bytes);
                assert_eq!(
                    fs::read(state_dir.join("events.jsonl")).unwrap(),
                    events_bytes
                );
                assert_eq!(
                    fs::read_to_string(td.path().join("cargo-calls")).unwrap(),
                    "alpha\n"
                );
            }
        }
        fs::write(td.path().join("stop-cargo"), "stop").unwrap();
    }
}

#[test]
#[serial]
fn interrupted_resume_reconciles_not_published_before_a_budgeted_retry() {
    use shipper_core::types::{EventType, ReconciliationOutcome};

    let td = tempdir().unwrap();
    create_workspace(td.path());
    let registry = RegistryFixture::start(td.path());
    let mut running = RunningChild {
        child: Some(
            command(td.path(), &registry.address, "publish", false)
                .spawn()
                .unwrap(),
        ),
        root: td.path().to_path_buf(),
    };
    wait_for_files(
        td.path(),
        &["cargo-pid-alpha"],
        running.child.as_mut().unwrap(),
    );
    send_signal(&running, "TERM");
    assert_eq!(
        failure_report(&bounded_output(&mut running))["category"],
        "interrupted"
    );
    let state_dir = td.path().join(".shipper");
    let interrupted = shipper_core::state::execution_state::load_state(&state_dir)
        .unwrap()
        .unwrap();
    let first_attempt = interrupted.attempt_history[0].clone();

    // The registry remains absent until the finite retry fixture succeeds.
    fs::write(td.path().join("complete-cargo"), "complete").unwrap();
    for _ in 0..2 {
        let resumed = run_bounded(command_with_budget(
            td.path(),
            &registry.address,
            "resume",
            false,
            "2",
        ));
        assert!(resumed.status.success(), "{resumed:?}");
        assert_eq!(
            fs::read_to_string(td.path().join("cargo-calls")).unwrap(),
            "alpha\nalpha\n"
        );
    }
    let state = shipper_core::state::execution_state::load_state(&state_dir)
        .unwrap()
        .unwrap();
    assert_eq!(state.attempt_history.len(), 2);
    assert_eq!(state.attempt_history[0], first_attempt);
    assert_eq!(state.attempt_history[1].attempt, 2);
    let log =
        shipper_core::state::events::EventLog::read_from_file(&state_dir.join("events.jsonl"))
            .unwrap();
    let absent = log
        .all_events()
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
    let retry = log
        .all_events()
        .iter()
        .position(|event| {
            matches!(
                event.event_type,
                EventType::PackageAttempted { attempt: 2, .. }
            )
        })
        .unwrap();
    assert!(
        absent < retry,
        "a fresh invocation must reconcile before another upload"
    );
}

#[test]
#[serial]
fn nonpublish_metadata_keeps_default_termination() {
    let td = tempdir().unwrap();
    create_workspace(td.path());
    let registry = RegistryFixture::start(td.path());
    let mut running = RunningChild {
        child: Some(
            command(td.path(), &registry.address, "plan", false)
                .env("SHIPPER_SIGNAL_BLOCK_METADATA", "yes")
                .env("CARGO", td.path().join("fake-bin/cargo"))
                .spawn()
                .unwrap(),
        ),
        root: td.path().to_path_buf(),
    };
    wait_for_files(
        td.path(),
        &["cargo-pid-metadata"],
        running.child.as_mut().unwrap(),
    );
    send_signal(&running, "TERM");
    let output = bounded_output(&mut running);
    assert_eq!(
        output.status.signal(),
        Some(15),
        "idle signal handling changed default termination: {output:?}"
    );
    assert!(!td.path().join("cargo-calls").exists());
    assert!(!td.path().join(".shipper/state.json").exists());
}
