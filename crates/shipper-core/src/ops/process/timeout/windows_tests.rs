//! Native process witnesses run inside a separately watched test process.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{self, Command, Stdio};
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::{Duration, Instant};

use super::super::capture::CaptureMode;
use super::super::interruption::PublishInterruption;
use super::super::{CaptureLoss, CapturedCommandOutput, CleanupFailure};
use super::windows::{CLEANUP_ALLOWANCE, Fault, run_supervised, spawn_in_job, with_fault};

const FIXTURE: &str = "ops::process::timeout::windows_tests::windows_fixture";
const PROBE: &str = "ops::process::timeout::windows_tests::windows_probe";
const CHILD_MODE: &str = "SHIPPER_WINDOWS_SUPERVISOR_TEST_CHILD";
const SIBLING_MODE: &str = "sibling";
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CLEANUP: Duration = Duration::from_secs(3);
const OUTER_WATCHDOG: Duration = Duration::from_secs(25);

fn test_exe() -> std::path::PathBuf {
    std::env::current_exe().expect("test binary path")
}

fn wait_until(duration: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + duration;
    while !condition() {
        assert!(Instant::now() < deadline, "fixture watchdog expired");
        thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_args(name: &str) -> [&str; 4] {
    ["--ignored", "--exact", name, "--nocapture"]
}

fn fixture_command() -> Command {
    let mut command = Command::new(test_exe());
    command.args(fixture_args(FIXTURE));
    command
}

fn fixture(
    dir: &Path,
    mode: &str,
    timeout: Duration,
    capture: CaptureMode,
) -> CapturedCommandOutput {
    fs::write(dir.join("mode"), mode).unwrap();
    run_supervised(
        test_exe().to_str().unwrap(),
        &fixture_args(FIXTURE),
        dir,
        Some(timeout),
        capture,
    )
    .unwrap_or_else(|error| panic!("supervisor fixture {mode}: {error:#}"))
}

fn assert_clean(result: &CapturedCommandOutput, status: i32) {
    assert_eq!(
        result.output.exit_code, status,
        "output: {:?}, capture: {:?}, cleanup: {:?}",
        result.output, result.capture, result.cleanup_failure
    );
    assert!(!result.output.timed_out);
    assert!(!result.interrupted);
    assert_eq!(
        result.cleanup_failure, None,
        "output: {:?}, capture: {:?}",
        result.output, result.capture
    );
    assert!(result.capture.is_complete());
}

fn descendant_is_gone(dir: &Path) -> bool {
    OpenOptions::new()
        .read(true)
        .open(dir.join("descendant-handle"))
        .is_ok()
}

fn process_resources() -> (usize, usize) {
    // Read the resource counts of this probe while it remains alive. The
    // outer watcher also bounds the diagnostic subprocess.
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            "$p = Get-Process -Id {}; Write-Output $p.HandleCount; Write-Output $p.Threads.Count",
            process::id()
        ))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let counts = String::from_utf8(output.stdout).unwrap();
    let mut counts = counts.lines().map(|line| line.trim().parse().unwrap());
    (counts.next().unwrap(), counts.next().unwrap())
}

#[test]
#[serial_test::serial]
#[ignore = "launched only by a native supervision witness"]
fn windows_fixture() {
    if let Some(child_mode) = std::env::var_os(CHILD_MODE) {
        let _exclusive = OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(0)
            .open("descendant-handle")
            .unwrap();
        fs::write("descendant-ready", "").unwrap();
        // The isolation control must outlive every check the watchdog permits.
        let lifetime = if child_mode == SIBLING_MODE {
            OUTER_WATCHDOG + TEST_TIMEOUT
        } else {
            Duration::from_secs(12)
        };
        thread::sleep(lifetime);
        process::exit(0);
    }

    let mode = fs::read_to_string("mode").unwrap();
    let code = match mode.as_str() {
        "quiet0" | "quiet42" => {
            writeln!(io::stdout(), "stdout-marker").unwrap();
            writeln!(io::stderr(), "stderr-marker").unwrap();
            if mode == "quiet0" { 0 } else { 42 }
        }
        "noisy0" | "noisy42" | "noisy-out0" | "noisy-err42" => {
            for _ in 0..128 {
                if mode != "noisy-err42" {
                    io::stdout().write_all(&[b'o'; 8192]).unwrap();
                }
                if mode != "noisy-out0" {
                    io::stderr().write_all(&[b'e'; 8192]).unwrap();
                }
            }
            if mode.ends_with('0') { 0 } else { 42 }
        }
        "descendant0" | "descendant42" | "timeout" => {
            let _descendant = fixture_command().env(CHILD_MODE, "1").spawn().unwrap();
            wait_until(TEST_TIMEOUT, || Path::new("descendant-ready").exists());
            if mode == "timeout" {
                thread::sleep(Duration::from_secs(12));
            }
            // Exit the fixture here so the supervisor must finish cleanup
            // while the descendant still owns the inherited pipes.
            process::exit(if mode == "descendant42" { 42 } else { 0 });
        }
        _ => panic!("unexpected fixture {mode}"),
    };
    process::exit(code);
}

#[test]
#[serial_test::serial]
#[ignore = "the watchdog launches this probe in a separate process"]
fn windows_probe() {
    let root = tempfile::tempdir().unwrap();
    let tail = CaptureMode::PublishTail(8);
    let full = CaptureMode::Full;
    for (mode, status) in [("quiet0", 0), ("quiet42", 42)] {
        let output = fixture(root.path(), mode, TEST_TIMEOUT, tail);
        assert_clean(&output, status);
        assert!(output.output.stdout.contains("stdout-marker"));
        assert!(output.output.stderr.contains("stderr-marker"));
    }
    for (mode, status, noisy_out, noisy_err) in [
        ("noisy0", 0, true, true),
        ("noisy42", 42, true, true),
        ("noisy-out0", 0, true, false),
        ("noisy-err42", 42, false, true),
    ] {
        let output = fixture(root.path(), mode, TEST_TIMEOUT, full);
        assert_clean(&output, status);
        assert_eq!(
            output.output.stdout.contains(&"o".repeat(1024 * 1024)),
            noisy_out
        );
        assert_eq!(
            output.output.stderr.contains(&"e".repeat(1024 * 1024)),
            noisy_err
        );
    }
    let output = fixture(root.path(), "noisy0", TEST_TIMEOUT, tail);
    assert_eq!(output.capture.stdout, Some(CaptureLoss::LineBytes));
    assert_eq!(output.capture.stderr, Some(CaptureLoss::LineBytes));
    assert_eq!(output.cleanup_failure, None);
    assert_eq!(output.output.exit_code, 0);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_clean(&fixture(root.path(), "quiet0", TEST_TIMEOUT, tail), 0);
    });
    let before = process_resources();
    for _ in 0..20 {
        assert_clean(&fixture(root.path(), "quiet0", TEST_TIMEOUT, tail), 0);
    }
    for _ in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        assert_clean(&fixture(dir.path(), "descendant0", TEST_TIMEOUT, tail), 0);
        assert!(descendant_is_gone(dir.path()));
    }
    assert_eq!(process_resources(), before);

    // The sibling holds its own exclusive handle for the duration of every
    // destructive branch. Its process handle remains with this test only.
    let sibling_dir = tempfile::tempdir().unwrap();
    let mut sibling = Sibling(
        fixture_command()
            .env(CHILD_MODE, SIBLING_MODE)
            .current_dir(sibling_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(TEST_TIMEOUT, || {
        sibling_dir.path().join("descendant-ready").exists()
    });

    for (mode, status) in [("descendant0", 0), ("descendant42", 42)] {
        let dir = tempfile::tempdir().unwrap();
        let output = fixture(dir.path(), mode, TEST_TIMEOUT, tail);
        assert_clean(&output, status);
        assert!(output.output.duration < MAX_CLEANUP);
        assert!(descendant_is_gone(dir.path()));
        assert!(sibling.0.try_wait().unwrap().is_none());
    }
    let dir = tempfile::tempdir().unwrap();
    let timeout = Duration::from_millis(1500);
    let output = fixture(dir.path(), "timeout", timeout, tail);
    assert!(output.output.timed_out);
    assert_eq!(output.output.exit_code, -1);
    assert_eq!(output.cleanup_failure, None);
    assert!(output.output.duration < timeout + MAX_CLEANUP);
    assert!(descendant_is_gone(dir.path()));
    assert!(sibling.0.try_wait().unwrap().is_none());

    let dir = tempfile::tempdir().unwrap();
    let output = with_fault(Fault::Terminate, || {
        fixture(dir.path(), "descendant42", TEST_TIMEOUT, tail)
    });
    assert_eq!(output.cleanup_failure, Some(CleanupFailure::JobTermination));
    assert_eq!(output.capture.stdout, Some(CaptureLoss::Cleanup));
    assert_eq!(output.capture.stderr, Some(CaptureLoss::Cleanup));
    assert_eq!(output.output.exit_code, 42);
    assert!(!output.output.timed_out);
    assert!(output.output.duration >= CLEANUP_ALLOWANCE);
    assert!(output.output.duration < MAX_CLEANUP);
    wait_until(TEST_TIMEOUT, || descendant_is_gone(dir.path()));
    assert!(sibling.0.try_wait().unwrap().is_none());

    let output = with_fault(Fault::Accounting, || {
        fixture(root.path(), "quiet0", TEST_TIMEOUT, tail)
    });
    assert_eq!(output.cleanup_failure, Some(CleanupFailure::JobStillActive));
    let output = with_fault(Fault::Reap, || {
        fixture(root.path(), "quiet0", TEST_TIMEOUT, tail)
    });
    assert_eq!(output.cleanup_failure, Some(CleanupFailure::Reap));
    // libtest itself writes to stdout before a fixture runs. Use a silent
    // child so closing either reader does not change the child's exit code.
    let cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join("cmd.exe");
    for (fault, stdout, stderr) in [
        (Fault::ReadStdout, Some(CaptureLoss::Read), None),
        (Fault::ReadStderr, None, Some(CaptureLoss::Read)),
    ] {
        let output = with_fault(fault, || {
            run_supervised(
                cmd.to_str().unwrap(),
                &["/d", "/c", "exit", "/b", "0"],
                root.path(),
                Some(TEST_TIMEOUT),
                tail,
            )
            .unwrap()
        });
        assert_eq!(output.capture.stdout, stdout);
        assert_eq!(output.capture.stderr, stderr);
        assert_eq!(
            output.cleanup_failure,
            Some(CleanupFailure::PipeEofUnconfirmed)
        );
        assert_eq!(output.output.exit_code, 0);
        assert!(output.output.duration < MAX_CLEANUP);
    }
    assert!(sibling.0.try_wait().unwrap().is_none());

    let setup = with_fault(Fault::JobSetup, || {
        run_supervised(
            test_exe().to_str().unwrap(),
            &fixture_args(FIXTURE),
            root.path(),
            Some(TEST_TIMEOUT),
            tail,
        )
    });
    assert!(
        setup
            .err()
            .is_some_and(|error| error.is::<super::CommandNotLaunched>())
    );
    let missing_program = root.path().join("missing-cargo.exe");
    let missing = run_supervised(
        missing_program.to_str().unwrap(),
        &[],
        root.path(),
        Some(TEST_TIMEOUT),
        tail,
    );
    assert!(
        missing
            .err()
            .is_some_and(|error| error.is::<super::CommandNotLaunched>())
    );
    for (fault, stage) in [
        (Fault::Assign, CleanupFailure::JobAssignment),
        (Fault::Resume, CleanupFailure::JobResume),
    ] {
        let output = with_fault(fault, || fixture(root.path(), "quiet0", TEST_TIMEOUT, tail));
        assert_eq!(output.cleanup_failure, Some(stage));
        assert_eq!(output.output.exit_code, -1);
        assert_eq!(output.capture.stdout, Some(CaptureLoss::Cleanup));
        assert_eq!(output.capture.stderr, Some(CaptureLoss::Cleanup));
    }
    assert!(sibling.0.try_wait().unwrap().is_none());

    // Interruption is sticky in this process, so this witness runs last.
    let dir = tempfile::tempdir().unwrap();
    let context = PublishInterruption::process_wide();
    let requested = context.request_flag();
    let default_action = context.default_action_flag();
    let output = thread::scope(|scope| {
        scope.spawn(|| {
            wait_until(TEST_TIMEOUT, || {
                dir.path().join("descendant-ready").exists()
            });
            assert!(!default_action.load(SeqCst));
            requested.store(true, SeqCst);
        });
        fixture(dir.path(), "timeout", TEST_TIMEOUT + TEST_TIMEOUT, tail)
    });
    assert!(output.interrupted);
    assert!(!output.output.timed_out);
    assert_eq!(output.cleanup_failure, None);
    assert!(descendant_is_gone(dir.path()));
    assert!(sibling.0.try_wait().unwrap().is_none());
    assert!(!default_action.load(SeqCst));
    drop(output);
    assert!(default_action.load(SeqCst));

    sibling.stop().expect("stop and reap the control sibling");
    assert!(descendant_is_gone(sibling_dir.path()));
}

struct Sibling(process::Child);

impl Sibling {
    fn stop(&mut self) -> io::Result<()> {
        if self.0.try_wait()?.is_none() {
            self.0.kill()?;
        }
        let deadline = Instant::now() + MAX_CLEANUP;
        while self.0.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "control sibling did not exit after termination",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

impl Drop for Sibling {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[test]
#[serial_test::serial]
fn bounded_windows_supervision_has_an_external_watchdog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.log");
    let log = fs::File::create(&path).unwrap();
    // The watchdog owns a separate outer job so a wedged fixture cannot
    // survive a watchdog failure. This also tests nested-job assignment.
    let outer_job = processkit::ProcessGroup::new().unwrap();
    let mut command = windows_spawn::Command::new(test_exe());
    command
        .args(fixture_args(PROBE))
        .stdin(windows_spawn::Stdio::null())
        .stdout(windows_spawn::Stdio::from(log.try_clone().unwrap()))
        .stderr(windows_spawn::Stdio::from(log));
    let mut child = spawn_in_job(&outer_job, command).unwrap();
    let deadline = Instant::now() + OUTER_WATCHDOG;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{}", fs::read_to_string(&path).unwrap());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Windows supervisor exceeded the external watchdog: {}",
            fs::read_to_string(&path).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(15));
    }
}
