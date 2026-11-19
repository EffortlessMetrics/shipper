use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

struct RegistrySpy {
    base_url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RegistrySpy {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").expect("local registry");
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_requests = Arc::clone(&requests);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                if let Some(request) = server
                    .recv_timeout(Duration::from_millis(20))
                    .expect("registry request")
                {
                    worker_requests.fetch_add(1, Ordering::AcqRel);
                    let _ = request.respond(Response::from_string("{}").with_status_code(404));
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

impl Drop for RegistrySpy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .expect("registry worker")
            .join()
            .expect("join registry");
    }
}

fn state_bytes(directory: &Path) -> Option<BTreeMap<PathBuf, Vec<u8>>> {
    directory.exists().then(|| {
        fs::read_dir(directory)
            .expect("state directory")
            .map(|entry| {
                let entry = entry.expect("state entry");
                (
                    PathBuf::from(entry.file_name()),
                    fs::read(entry.path()).expect("state bytes"),
                )
            })
            .collect()
    })
}

#[test]
#[serial_test::serial]
fn per_error_cli_refuses_class_configuration_before_effects() -> Result<()> {
    let registry = RegistrySpy::start();
    for class in ["retryable", "ambiguous", "permanent"] {
        for parallel in [false, true] {
            for retained in [false, true] {
                let directory = tempdir()?;
                create_single_crate_workspace(directory.path());
                let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(directory.path());
                let config_path = directory.path().join(".shipper.toml");
                fs::write(
                    &config_path,
                    format!("[retry.per_error.{class}]\nmax_attempts = 1\n"),
                )?;
                let state_dir = directory.path().join(".shipper");
                if retained {
                    fs::create_dir(&state_dir)?;
                    for name in [
                        "state.json",
                        "events.jsonl",
                        "receipt.json",
                        "reconciliation.json",
                        "lock",
                    ] {
                        fs::write(state_dir.join(name), format!("retained {name}\n"))?;
                    }
                }
                let before = state_bytes(&state_dir);
                let cargo_log = directory.path().join("cargo-publish.log");
                for command_name in ["publish", "resume"] {
                    let mut command = loopback_shipper_cmd();
                    command
                        .timeout(Duration::from_secs(10))
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
                            "--force",
                            "--force-resume",
                            "--max-attempts",
                            "9",
                        ])
                        .env("PATH", &new_path)
                        .env("REAL_CARGO", &real_cargo)
                        .env("SHIPPER_CARGO_BIN", &fake_cargo)
                        .env("SHIPPER_FAKE_PUBLISH_LOG", &cargo_log);
                    if parallel {
                        command.arg("--parallel");
                    }
                    if command_name == "resume" {
                        command.arg("--config").arg(&config_path);
                    }
                    let output = command.arg(command_name).output()?;
                    let stderr = String::from_utf8(output.stderr)?;
                    ensure!(
                        output.status.code() == Some(1),
                        "{command_name}/{class}: {stderr}"
                    );
                    ensure!(
                        stderr.contains("retry.per_error is not supported"),
                        "{command_name}/{class}: {stderr}"
                    );
                    ensure!(stderr.contains("global [retry] policy"), "{stderr}");
                    ensure!(
                        state_bytes(&state_dir) == before,
                        "{command_name} changed state"
                    );
                    ensure!(!cargo_log.exists(), "{command_name} invoked Cargo publish");
                    ensure!(
                        registry.requests.load(Ordering::Acquire) == 0,
                        "{command_name} made a request"
                    );
                }
                let output = shipper_cmd()
                    .args(["config", "validate", "--path"])
                    .arg(&config_path)
                    .output()?;
                ensure!(!output.status.success());
                ensure!(
                    String::from_utf8(output.stderr)?.contains("retry.per_error is not supported")
                );
            }
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn per_error_omitted_and_empty_keep_global_cli_retry_budget() -> Result<()> {
    for suffix in ["", "[retry.per_error]\n"] {
        for parallel in [false, true] {
            let directory = tempdir()?;
            create_single_crate_workspace(directory.path());
            let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(directory.path());
            fs::write(
                directory.path().join(".shipper.toml"),
                format!(
                    "[retry]\npolicy = \"custom\"\nmax_attempts = 2\nbase_delay = \"1ms\"\nmax_delay = \"1ms\"\njitter = 0.0\n{suffix}"
                ),
            )?;
            let registry = RegistrySpy::start();
            let cargo_log = directory.path().join("cargo-publish.log");
            let state_dir = directory.path().join(".shipper");
            let mut command = loopback_shipper_cmd();
            command
                .timeout(Duration::from_secs(15))
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
                    "--no-readiness",
                    "--verify-mode",
                    "none",
                ])
                .env("PATH", &new_path)
                .env("REAL_CARGO", &real_cargo)
                .env("SHIPPER_CARGO_BIN", &fake_cargo)
                .env("SHIPPER_FAKE_PUBLISH_LOG", &cargo_log)
                .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
                .env("SHIPPER_FAKE_PUBLISH_STDERR", "connection refused");
            if parallel {
                command.arg("--parallel");
            }
            let output = command.arg("publish").output()?;
            let stderr = String::from_utf8(output.stderr)?;
            ensure!(output.status.code() == Some(1), "{stderr}");
            ensure!(
                !stderr.contains("retry.per_error is not supported"),
                "{stderr}"
            );
            let calls = fs::read_to_string(&cargo_log)?;
            ensure!(
                calls.lines().count() == 2,
                "global two-attempt ceiling: {calls}; {stderr}"
            );
            ensure!(calls.lines().all(|line| line.starts_with("publish ")));
            ensure!(registry.requests.load(Ordering::Acquire) > 0);
            let state = shipper_core::state::execution_state::load_state(&state_dir)?
                .context("retained state")?;
            ensure!(state.packages["demo@0.1.0"].attempts == 2);
            ensure!(state.attempt_history.len() == 2);
            ensure!(
                state
                    .attempt_history
                    .iter()
                    .all(|attempt| attempt.max_attempts == 2
                        && attempt.error_class == Some(shipper_types::ErrorClass::Retryable))
            );
        }
    }
    Ok(())
}
