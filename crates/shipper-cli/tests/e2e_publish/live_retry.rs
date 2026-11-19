//! Actual retries retain human stderr ordering and quiet/JSON stream contracts.

use std::path::PathBuf;

use super::*;

#[test]
#[serial_test::serial]
#[ignore = "fake Cargo subprocess invoked by retry output tests"]
fn retry_cargo_child() {
    let root = PathBuf::from(std::env::var_os("SHIPPER_414_ROOT").unwrap());
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
    std::process::exit(0);
}

#[test]
#[serial_test::serial]
fn live_retry_cli_preserves_human_quiet_and_json_output() -> Result<()> {
    for parallel in [false, true] {
        for format in ["human", "quiet", "json"] {
            let scenario = format!("parallel={parallel}, format={format}");
            let directory = tempdir().with_context(|| format!("{scenario}: create fixture"))?;
            let root = directory.path();
            create_single_crate_workspace(root);
            let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(root);
            #[cfg(not(windows))]
            fs::write(
                &fake_cargo,
                "#!/bin/sh\nif [ \"$1\" = publish ]; then\n  exec \"$SHIPPER_414_TEST_BIN\" --ignored --exact live_retry::retry_cargo_child --nocapture\nfi\nexec \"$REAL_CARGO\" \"$@\"\n",
            )
            .with_context(|| format!("{scenario}: write Cargo proxy {fake_cargo}"))?;
            #[cfg(windows)]
            fs::write(
                &fake_cargo,
                "@echo off\r\nif \"%~1\"==\"publish\" (\r\n  \"%SHIPPER_414_TEST_BIN%\" --ignored --exact live_retry::retry_cargo_child --nocapture\r\n  exit /b\r\n)\r\n\"%REAL_CARGO%\" %*\r\nexit /b\r\n",
            )
            .with_context(|| format!("{scenario}: write Cargo proxy {fake_cargo}"))?;
            // Precheck and the failed upload remain absent; only the second
            // Cargo attempt's post-upload check can observe a published version.
            let registry = spawn_bounded_registry(vec![404, 404, 200], 3);
            let state_dir = root.join("state");
            let fixture_exe = std::env::current_exe()
                .with_context(|| format!("{scenario}: locate recursive fake-Cargo executable"))?;
            // The local proof can retain the actual compiled CLI outside a
            // disposable Cargo target. This selector changes only the test.
            let cli_exe = std::env::var_os("SHIPPER_RETRY_TEST_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(assert_cmd::cargo::cargo_bin!("shipper-cli")));
            let mut command = Command::new(&cli_exe);
            command
                .arg("--allow-loopback")
                .timeout(Duration::from_secs(15))
                .current_dir(root)
                .arg("--manifest-path")
                .arg(root.join("Cargo.toml"))
                .arg("--state-dir")
                .arg(&state_dir)
                .arg("--api-base")
                .arg(&registry.base_url)
                .args([
                    "--allow-dirty",
                    "--skip-ownership-check",
                    "--no-verify",
                    "--max-attempts",
                    "2",
                    "--base-delay",
                    "150ms",
                    "--max-delay",
                    "150ms",
                    "--retry-jitter",
                    "0",
                    "--readiness-method",
                    "api",
                    "--readiness-poll",
                    "0ms",
                    "--readiness-timeout",
                    "1s",
                ])
                .env("PATH", new_path)
                .env("REAL_CARGO", real_cargo)
                .env("SHIPPER_CARGO_BIN", fake_cargo)
                .env("SHIPPER_414_TEST_BIN", &fixture_exe)
                .env("SHIPPER_414_ROOT", root)
                .env("CARGO_HOME", root.join("cargo-home"))
                .env_remove("CARGO_REGISTRY_TOKEN")
                .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");
            if parallel {
                command.arg("--parallel");
            }
            match format {
                "quiet" => {
                    command.arg("--quiet");
                }
                "json" => {
                    command.args(["--format", "json"]);
                }
                _ => {}
            }
            let output = command.arg("publish").output().with_context(|| {
                format!(
                    "{scenario}: spawn/wait for CLI {} (exists={}); recursive fake Cargo {} (exists={}); workspace {}",
                    cli_exe.display(),
                    cli_exe.is_file(),
                    fixture_exe.display(),
                    fixture_exe.is_file(),
                    root.display(),
                )
            })?;
            registry
                .finish(Duration::from_secs(2))
                .with_context(|| format!("{scenario}: finish mock registry"))?;
            let stdout = String::from_utf8(output.stdout)
                .with_context(|| format!("{scenario}: decode CLI stdout"))?;
            let stderr = String::from_utf8(output.stderr)
                .with_context(|| format!("{scenario}: decode CLI stderr"))?;
            ensure!(
                output.status.success(),
                "parallel={parallel} format={format}: stdout={stdout}; stderr={stderr}"
            );
            let calls_path = root.join("calls");
            let calls = fs::read_to_string(&calls_path).with_context(|| {
                format!("{scenario}: read fake-Cargo calls {}", calls_path.display())
            })?;
            ensure!(calls == "publish\npublish\n", "{scenario}: calls={calls:?}");
            let state_path = state_dir.join("state.json");
            let state_bytes = fs::read(&state_path).with_context(|| {
                format!("{scenario}: read durable state {}", state_path.display())
            })?;
            let state: serde_json::Value = serde_json::from_slice(&state_bytes)
                .with_context(|| format!("{scenario}: parse durable state"))?;
            ensure!(state["packages"]["demo@0.1.0"]["attempts"] == 2);
            ensure!(state["packages"]["demo@0.1.0"]["state"]["state"] == "published");
            if format == "json" {
                let envelope: serde_json::Value = serde_json::from_str(&stdout)
                    .with_context(|| format!("{scenario}: parse clean CLI JSON stdout"))?;
                ensure!(envelope["schema_version"] == "shipper.publish.v1");
                ensure!(envelope["outcome"]["status"] == "success");
                ensure!(stderr.is_empty(), "JSON stderr must stay empty: {stderr}");
            } else {
                ensure!(!stdout.contains("next attempt in"));
                ensure!(!stdout.contains("cargo publish failed"));
                if format == "quiet" {
                    ensure!(stderr.is_empty(), "quiet stderr must stay empty: {stderr}");
                } else {
                    let failed = stderr
                        .find("cargo publish failed")
                        .context("failure warn")?;
                    let retry = stderr.find("[retry]").context("retry notice")?;
                    let success = stderr
                        .find("cargo publish exited successfully")
                        .context("success info")?;
                    ensure!(failed < retry && retry < success, "{stderr}");
                    ensure!(!stderr.contains("next attempt in 0s"), "{stderr}");
                }
            }
        }
    }
    Ok(())
}
