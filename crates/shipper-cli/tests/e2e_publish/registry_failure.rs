//! A failed post-Cargo registry lookup must retain the complete Cargo outcome.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use shipper_core::state::{events::EventLog, execution_state, rebuild};
use shipper_types::{ErrorClass, EventType, PackageState};

use super::*;

struct PhaseRegistry {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<Result<(usize, usize)>>,
}

impl PhaseRegistry {
    fn start(publish_log: PathBuf) -> Result<Self> {
        let server = Server::http("127.0.0.1:0")
            .map_err(|error| anyhow!("start phase registry: {error}"))?;
        let base_url = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || -> Result<(usize, usize)> {
            let deadline = Instant::now() + Duration::from_secs(25);
            let (mut before_cargo, mut after_cargo) = (0, 0);
            while !worker_stop.load(Ordering::Acquire) {
                ensure!(
                    Instant::now() < deadline,
                    "phase registry exceeded its deadline"
                );
                let Some(request) = server.recv_timeout(Duration::from_millis(50))? else {
                    continue;
                };
                ensure!(request.url() == "/api/v1/crates/demo/0.1.0");
                // The existing fake-Cargo proxy writes this marker only when
                // executing publish. Startup requests cannot consume a scripted
                // "second response" and accidentally bypass the target branch.
                let status = if publish_log.is_file() {
                    after_cargo += 1;
                    500
                } else {
                    before_cargo += 1;
                    404
                };
                request
                    .respond(Response::from_string("{}").with_status_code(StatusCode(status)))?;
            }
            Ok((before_cargo, after_cargo))
        });
        Ok(Self {
            base_url,
            stop,
            handle,
        })
    }

    fn finish(self) -> Result<(usize, usize)> {
        self.stop.store(true, Ordering::Release);
        self.handle
            .join()
            .map_err(|_| anyhow!("phase registry thread panicked"))?
    }
}

fn visibility_error_retains_cargo_failure(parallel: bool) -> Result<()> {
    let directory = tempdir()?;
    create_single_crate_workspace(directory.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(directory.path());
    let state_dir = directory.path().join("visibility-error-state");
    let publish_log = directory.path().join("cargo-publish.log");
    let registry = PhaseRegistry::start(publish_log.clone())?;
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .current_dir(directory.path())
        .arg("--manifest-path")
        .arg(directory.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(&state_dir)
        .args([
            "--allow-dirty",
            "--skip-ownership-check",
            "--max-attempts",
            "2",
            "--base-delay",
            "0ms",
        ]);
    if parallel {
        command.args(["--parallel", "--max-concurrent", "2"]);
    }
    let output = command
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "timeout talking to server")
        .output();
    // Always stop/join the server before propagating command errors or making
    // assertions, including when the child reaches its bounded timeout.
    let requests = registry.finish()?;
    let output = output?;
    let stderr = String::from_utf8(output.stderr)?;
    ensure!(output.status.code() == Some(1), "{stderr}");
    ensure!(
        stderr.contains("failed to verify registry visibility after cargo failure"),
        "{stderr}"
    );
    ensure!(
        stderr.contains("unexpected status while checking version existence: 500"),
        "{stderr}"
    );
    ensure!(!stderr.contains("authorizes a controlled resume"));
    ensure!(
        requests.0 > 0 && requests.1 > 0,
        "before/after Cargo requests: {requests:?}"
    );
    let calls = fs::read_to_string(&publish_log)?;
    ensure!(
        calls.lines().count() == 1,
        "unexpected Cargo calls: {calls}"
    );
    ensure!(calls.contains("publish -p demo"));

    let state = execution_state::load_state(&state_dir)?.context("persisted execution state")?;
    let progress = state.packages.get("demo@0.1.0").context("demo progress")?;
    let expected = PackageState::Ambiguous {
        message: "transient failure (retryable)".to_owned(),
    };
    ensure!(
        progress.state == expected,
        "saved projection: {:?}",
        progress.state
    );
    ensure!(progress.attempts == 1);
    ensure!(
        state.attempt_history.len() == 1,
        "attempt history: {:?}",
        state.attempt_history
    );
    let detail = state
        .attempt_history
        .first()
        .context("completed Cargo detail")?;
    ensure!(detail.package == "demo" && detail.version == "0.1.0");
    ensure!(detail.attempt == 1 && detail.max_attempts == 2);
    ensure!(detail.error_class == Some(ErrorClass::Retryable));
    ensure!(detail.redacted_message.as_deref() == Some("transient failure (retryable)"));
    ensure!(detail.next_attempt_at.is_none());

    let event_path = state_dir.join("events.jsonl");
    let log = EventLog::read_from_file(&event_path)?;
    let events = log.all_events();
    let mut unconfirmed = events
        .iter()
        .filter(|event| matches!(event.event_type, EventType::PackageUploadUnconfirmed { .. }));
    let checkpoint = unconfirmed.next().context("unconfirmed Cargo checkpoint")?;
    ensure!(unconfirmed.next().is_none());
    ensure!(checkpoint.package == "demo@0.1.0");
    ensure!(
        matches!(&checkpoint.event_type, EventType::PackageUploadUnconfirmed { class: ErrorClass::Retryable, message } if message == "transient failure (retryable)")
    );
    ensure!(checkpoint.timestamp == detail.ended_at);
    let attempted = events
        .iter()
        .position(|event| matches!(event.event_type, EventType::PackageAttempted { .. }))
        .context("attempt event")?;
    let output_event = events
        .iter()
        .position(|event| matches!(event.event_type, EventType::PackageOutput { .. }))
        .context("Cargo output event")?;
    let checkpoint_position = events
        .iter()
        .position(|event| matches!(event.event_type, EventType::PackageUploadUnconfirmed { .. }))
        .context("checkpoint event position")?;
    ensure!(attempted < output_event && output_event < checkpoint_position);
    ensure!(
        !events.iter().any(|event| matches!(
            event.event_type,
            EventType::PackageFailed { .. }
                | EventType::PublishReconciling { .. }
                | EventType::PublishReconciled { .. }
                | EventType::ExecutionStopped { .. }
                | EventType::RetryScheduled { .. }
                | EventType::RetryBackoffStarted { .. }
                | EventType::PublishWaiting { .. }
                | EventType::StateEventDriftDetected { .. }
        )),
        "visibility error must not invent reconciliation, retry or controlled-stop evidence"
    );
    let rebuilt = rebuild::rebuild_state_from_events(
        &event_path,
        rebuild::StateRebuildOptions::new(state.registry.clone())
            .with_fallback_plan_id(&state.plan_id),
    )?;
    let rebuilt_progress = rebuilt.packages.get("demo@0.1.0").context("rebuilt demo")?;
    ensure!(rebuilt_progress.state == expected && rebuilt_progress.attempts == progress.attempts);
    ensure!(rebuilt.attempt_history == state.attempt_history);
    ensure!(!state_dir.join("reconciliation.json").exists());
    ensure!(!state_dir.join("receipt.json").exists());
    Ok(())
}

#[test]
#[serial_test::serial]
fn sequential_registry_error_retains_unconfirmed_projection_and_attempt() -> Result<()> {
    visibility_error_retains_cargo_failure(false)
}

#[test]
#[serial_test::serial]
fn parallel_registry_error_retains_unconfirmed_projection_and_attempt() -> Result<()> {
    visibility_error_retains_cargo_failure(true)
}
