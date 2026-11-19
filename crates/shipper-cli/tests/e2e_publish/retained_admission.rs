use super::*;

#[test]
#[serial_test::serial]
fn retained_admission_reports_refusal_without_consuming_the_checkpoint() -> Result<()> {
    for parallel in [false, true] {
        let workspace = tempdir()?;
        create_single_crate_workspace(workspace.path());
        let (path, real_cargo, fake_cargo) = setup_fake_cargo(workspace.path());
        let state_dir = workspace.path().join(".shipper");
        let cargo_log = workspace.path().join("publish.log");
        let registry = spawn_bounded_registry(vec![404], 2);
        let api_base = registry.base_url.clone();
        let command = |ceiling: &str| {
            let mut command = loopback_shipper_cmd();
            command
                .current_dir(workspace.path())
                .timeout(Duration::from_secs(20))
                .arg("--manifest-path")
                .arg(workspace.path().join("Cargo.toml"))
                .arg("--state-dir")
                .arg(&state_dir)
                .arg("--api-base")
                .arg(&api_base)
                .args([
                    "--allow-dirty",
                    "--skip-ownership-check",
                    "--verify-mode",
                    "none",
                    "--readiness-timeout",
                    "0ms",
                    "--readiness-poll",
                    "0ms",
                    "--verify-timeout",
                    "0ms",
                    "--verify-poll",
                    "0ms",
                    "--base-delay",
                    "0ms",
                    "--max-attempts",
                    ceiling,
                    "--format",
                    "json",
                ])
                .env("PATH", &path)
                .env("REAL_CARGO", &real_cargo)
                .env("SHIPPER_CARGO_BIN", &fake_cargo)
                .env("SHIPPER_FAKE_PUBLISH_LOG", &cargo_log)
                .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
                .env("SHIPPER_FAKE_PUBLISH_STDERR", "ambiguous transport close")
                .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET");
            if parallel {
                command.arg("--parallel");
            }
            command
        };
        let initial = command("1").arg("publish").output()?;
        ensure!(initial.status.code() == Some(1));
        registry.finish(Duration::from_secs(2))?;
        let report: serde_json::Value = serde_json::from_slice(&initial.stderr)?;
        ensure!(report["category"] == "recoverable_stop", "{report}");
        ensure!(read_publish_log(&cargo_log).len() == 1);
        let event_path = state_dir.join("events.jsonl");
        let stopped_events = fs::read_to_string(&event_path)?;
        let mut lines: Vec<_> = stopped_events.lines().collect();
        ensure!(
            lines
                .pop()
                .context("stop marker")?
                .contains("execution_stopped")
        );
        fs::write(&event_path, format!("{}\n", lines.join("\n")))?;
        let artifacts = [
            "state.json",
            "events.jsonl",
            "receipt.json",
            "reconciliation.json",
        ];
        let snapshot = || -> Result<Vec<Option<Vec<u8>>>> {
            artifacts
                .iter()
                .map(|name| {
                    let path = state_dir.join(name);
                    if path.exists() {
                        Ok(Some(fs::read(path)?))
                    } else {
                        Ok(None)
                    }
                })
                .collect()
        };
        let retained = snapshot()?;
        for entry in ["publish", "resume"] {
            let refused = command("2").args(["--force", entry]).output()?;
            ensure!(refused.status.code() == Some(1) && refused.stdout.is_empty());
            if entry == "publish" {
                let report: serde_json::Value = serde_json::from_slice(&refused.stderr)?;
                ensure!(report["schema_version"] == "shipper.publish.error.v1");
                ensure!(report["command"] == "publish" && report["status"] == "failed");
                ensure!(report["category"] == "state_conflict");
                ensure!(report["safe_to_rerun"]["value"] == false);
                ensure!(report["next_action"]["kind"] == "inspect_events");
            } else {
                // Non-budget resume failures retain their diagnostic contract;
                // the existing typed retry-budget JSON is checked below.
                let stderr = String::from_utf8_lossy(&refused.stderr);
                ensure!(stderr.contains("resume was refused before changing retained evidence"));
                ensure!(stderr.contains("lacks the current controlled-stop marker"));
            }
            assert_secret_absent_from_output_and_state(&refused, &state_dir);
            ensure!(snapshot()? == retained);
            ensure!(read_publish_log(&cargo_log).len() == 1);
        }
        fs::write(&event_path, stopped_events)?;
        let coherent = snapshot()?;
        let ceiling = command("1").arg("resume").output()?;
        ensure!(ceiling.status.code() == Some(1) && ceiling.stdout.is_empty());
        let report: serde_json::Value = serde_json::from_slice(&ceiling.stderr)?;
        ensure!(report["schema_version"] == "shipper.resume.error.v1");
        ensure!(report["category"] == "retry_budget_exhausted");
        ensure!(report["current_attempts"] == 1 && report["minimum_max_attempts"] == 2);
        ensure!(report["safe_to_resume"]["value"] == true);
        ensure!(snapshot()? == coherent && read_publish_log(&cargo_log).len() == 1);
    }
    Ok(())
}
