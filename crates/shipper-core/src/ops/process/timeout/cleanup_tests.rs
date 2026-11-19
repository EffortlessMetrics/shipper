//! Real process witnesses with an external watchdog around the supervisor.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitOptions, kill_process, test_kill_process_group, waitpid};

use crate::ops::process::{CleanupFailure, run_command_with_publish_capture};

const FIXTURE: &str = "ops::process::timeout::cleanup_tests::cleanup_process_fixture";
const PROBE: &str = "ops::process::timeout::cleanup_tests::cleanup_supervisor_probe";
const WATCHDOG: Duration = Duration::from_secs(6);
const REPEATS: usize = 12;

fn fixture_command(directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
        .current_dir(directory);
    command
}

#[test]
#[serial_test::serial]
#[ignore = "invocation-owned process/descendant fixture, called by watchdog tests"]
fn cleanup_process_fixture() {
    let role = std::env::var("SHIPPER_TEST_CLEANUP_ROLE").unwrap_or_default();
    let mode = fs::read_to_string("mode").unwrap();
    if role == "descendant" {
        if mode.starts_with("escape") {
            rustix::process::setsid().expect("fixture escapes the invocation group");
        }
        fs::write("descendant-pid", std::process::id().to_string()).unwrap();
        fs::write("descendant-ready", "ready").unwrap();
        // Finite fixture lifetime is a backup to the parent test's watchdog and
        // cleanup. It must substantially exceed the production cleanup bound.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !Path::new("stop-descendant").exists() {
            thread::sleep(Duration::from_millis(10));
        }
        fs::write("descendant-finished", "finished").unwrap();
        std::process::exit(0);
    }

    fs::write("owned-pid", std::process::id().to_string()).unwrap();
    if mode == "signal-exit" {
        println!("direct stdout sentinel");
        eprintln!("direct stderr sentinel");
        std::process::exit(0);
    }
    let mut descendant = fixture_command(&std::env::current_dir().unwrap())
        .env("SHIPPER_TEST_CLEANUP_ROLE", "descendant")
        .stdout(if mode == "closed-pipes" {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .stderr(if mode == "closed-pipes" {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(2);
    while !Path::new("descendant-ready").exists() {
        assert!(
            Instant::now() < ready_deadline,
            "descendant never became ready"
        );
        thread::sleep(Duration::from_millis(5));
    }
    println!("direct stdout sentinel");
    eprintln!("direct stderr sentinel");
    std::io::stdout().flush().unwrap();
    std::io::stderr().flush().unwrap();
    match mode.as_str() {
        "timeout" | "escape-timeout" => {
            let _ = descendant.wait();
            std::process::exit(55);
        }
        "exit0" | "escape0" | "closed-pipes" | "repeated" | "signal-live" => std::process::exit(0),
        "exit42" | "escape" => std::process::exit(42),
        _ => panic!("unknown fixture mode"),
    }
}

struct ChildCleanup(Child);

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_pid(directory: &Path, name: &str) -> Pid {
    let raw = fs::read_to_string(directory.join(name))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert!(raw > 1);
    Pid::from_raw(raw).unwrap()
}

fn cleanup_fixture(directory: &Path) {
    // These PIDs come only from this test's unique directory and controlled
    // finite-lived fixture. Never discover targets by process name or ancestry.
    let _ = fs::write(directory.join("stop-descendant"), "stop");
    for name in ["descendant-pid", "owned-pid", "sibling-pid"] {
        if let Ok(raw) = fs::read_to_string(directory.join(name))
            && let Ok(raw) = raw.parse::<i32>()
            && raw > 1
        {
            let _ = kill_process(Pid::from_raw(raw).unwrap(), Signal::KILL);
        }
    }
}

#[cfg(target_os = "linux")]
fn resource_counts() -> (usize, usize) {
    (
        fs::read_dir("/proc/self/fd").unwrap().count(),
        fs::read_dir("/proc/self/task").unwrap().count(),
    )
}

#[test]
#[serial_test::serial]
#[ignore = "fresh-process supervision witness, called with an external watchdog"]
fn cleanup_supervisor_probe() {
    let root = std::env::current_dir().unwrap();
    let mode = fs::read_to_string(root.join("mode")).unwrap();
    // A separately owned sibling must survive every group cleanup operation.
    let mut sibling = ChildCleanup(
        Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    fs::write(root.join("sibling-pid"), sibling.0.id().to_string()).unwrap();
    #[cfg(target_os = "linux")]
    let before = resource_counts();
    let repeats = if mode == "repeated" { REPEATS } else { 1 };
    for iteration in 0..repeats {
        let directory = root.join(format!("attempt-{iteration}"));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("mode"), &mode).unwrap();
        let _signal_error = mode
            .starts_with("signal-")
            .then(super::unix::inject_group_signal_error);
        let started = Instant::now();
        let result = run_command_with_publish_capture(
            std::env::current_exe().unwrap().to_str().unwrap(),
            &["--exact", FIXTURE, "--ignored", "--nocapture"],
            &directory,
            if mode == "timeout" {
                Some(Duration::from_millis(500))
            } else {
                None
            },
            50,
        )
        .unwrap();
        let elapsed = started.elapsed();
        // Clean the escaped fixture before assertions can unwind this probe.
        if mode == "escape" {
            cleanup_fixture(&directory);
        }
        assert!(
            elapsed < Duration::from_secs(3),
            "{mode} returned in {elapsed:?}"
        );
        assert!(
            sibling.0.try_wait().unwrap().is_none(),
            "unrelated sibling was killed"
        );
        assert!(
            matches!(
                waitpid(Some(read_pid(&directory, "owned-pid")), WaitOptions::NOHANG),
                Err(rustix::io::Errno::CHILD)
            ),
            "the direct child must already be reaped"
        );
        let group_status = test_kill_process_group(read_pid(&directory, "owned-pid"));
        if mode == "signal-live" {
            assert!(
                group_status.is_ok(),
                "the unsignalled descendant is still present"
            );
            // The direct child is already reaped. Stop only this live fixture
            // descendant; do not signal its former group or the cached leader.
            let _ = kill_process(read_pid(&directory, "descendant-pid"), Signal::KILL);
            assert_eq!(result.cleanup_failure, Some(CleanupFailure::GroupSignal));
            assert!(!result.capture.is_complete());
            assert_eq!(result.output.exit_code, 0);
            assert!(!result.output.timed_out);
        } else {
            assert!(
                matches!(group_status, Err(rustix::io::Errno::SRCH)),
                "the invocation group must be empty after completed shutdown"
            );
        }
        if mode == "escape" {
            assert_eq!(result.cleanup_failure, Some(CleanupFailure::InheritedPipe));
            assert!(!result.capture.is_complete());
            assert_eq!(result.output.exit_code, 42);
            assert!(!result.output.timed_out);
        } else if mode != "signal-live" {
            assert_eq!(result.cleanup_failure, None);
            assert!(result.capture.is_complete());
            assert!(!directory.join("descendant-finished").exists());
            assert!(result.output.stdout.contains("direct stdout sentinel"));
            assert!(result.output.stderr.contains("direct stderr sentinel"));
            assert_eq!(result.output.timed_out, mode == "timeout");
            let expected = match mode.as_str() {
                "timeout" => -1,
                "exit42" => 42,
                _ => 0,
            };
            assert_eq!(result.output.exit_code, expected);
        }
        println!(
            "{mode} iteration {iteration}: {elapsed:?}, cleanup={:?}",
            result.cleanup_failure
        );
    }
    #[cfg(target_os = "linux")]
    assert_eq!(
        resource_counts(),
        before,
        "owned pipe descriptors or reader threads leaked"
    );
}

fn external_probe(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("mode"), mode).unwrap();
    let stdout_path = directory.path().join("probe.stdout");
    let stderr_path = directory.path().join("probe.stderr");
    let mut probe = ChildCleanup(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", PROBE, "--ignored", "--nocapture"])
            .current_dir(directory.path())
            .stdout(fs::File::create(&stdout_path).unwrap())
            .stderr(fs::File::create(&stderr_path).unwrap())
            .spawn()
            .unwrap(),
    );
    // The resource witness runs many individually bounded invocations. A
    // valid full cleanup allowance on each one must fit its outer watchdog;
    // the single-invocation regression witnesses keep the tighter six seconds.
    let watchdog = if mode == "repeated" {
        Duration::from_secs(3 * REPEATS as u64 + 3)
    } else {
        WATCHDOG
    };
    let deadline = Instant::now() + watchdog;
    let status = loop {
        if let Some(status) = probe.0.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = probe.0.kill();
            let _ = probe.0.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.is_some_and(|status| status.success()) {
        cleanup_fixture(directory.path());
        for entry in fs::read_dir(directory.path()).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                cleanup_fixture(&entry.path());
            }
        }
    }
    let stdout = fs::read_to_string(stdout_path).unwrap();
    let stderr = fs::read_to_string(stderr_path).unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "{mode}: supervisor failed or exceeded the external {watchdog:?} watchdog\n{stdout}\n{stderr}"
    );
    print!("{stdout}");
}

#[test]
#[serial_test::serial]
fn timeout_kills_owned_descendants_without_waiting_for_inherited_pipes() {
    external_probe("timeout");
}

#[test]
#[serial_test::serial]
fn signal_failure_is_provisional_only_when_final_cleanup_facts_are_proved() {
    external_probe("signal-exit");
    external_probe("signal-live");
}

#[test]
#[serial_test::serial]
fn direct_exit_preserves_zero_and_nonzero_status_with_inherited_pipes() {
    external_probe("exit0");
    external_probe("exit42");
}

#[test]
#[serial_test::serial]
fn escaped_inherited_pipe_returns_typed_cleanup_failure_within_allowance() {
    external_probe("escape");
}

#[test]
#[serial_test::serial]
fn exit_cleanup_contains_descendants_that_do_not_hold_output_pipes() {
    external_probe("closed-pipes");
}

#[test]
#[serial_test::serial]
fn repeated_cleanup_reaps_children_and_releases_pipes_without_reader_threads() {
    external_probe("repeated");
}
