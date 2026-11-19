use super::*;

#[test]
#[serial_test::serial]
fn publish_guard_contention_is_state_conflict_and_preserves_evidence_even_with_force() -> Result<()>
{
    let workspace = tempdir()?;
    create_single_crate_workspace(workspace.path());
    let (path, real_cargo, fake_cargo) = setup_fake_cargo(workspace.path());
    let state_dir = workspace.path().join(".shipper");
    let cargo_log = workspace.path().join("publish.log");
    let registry = spawn_bounded_registry(vec![200], 1);
    let api_base = registry.base_url.clone();
    let command = || {
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
                "--no-readiness",
                "--policy",
                "fast",
                "--verify-mode",
                "none",
                "--format",
                "json",
            ])
            .env("PATH", &path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_LOG", &cargo_log)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "71")
            .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET");
        command
    };

    // Use the CLI's exact workspace identity and a genuine completed checkpoint.
    let planned = command().arg("plan").output()?;
    ensure!(planned.status.success(), "{:?}", planned.stderr);
    let plan: serde_json::Value = serde_json::from_slice(&planned.stdout)?;
    let workspace_root =
        std::path::PathBuf::from(plan["workspace_root"].as_str().context("workspace root")?);
    let plan_id = plan["plan_id"].as_str().context("plan identity")?;
    let initial = command().arg("publish").output()?;
    ensure!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    ensure!(read_publish_log(&cargo_log).is_empty());
    registry.finish(Duration::from_secs(2))?;

    let owner = shipper_core::lock::LockFile::acquire(&state_dir, Some(&workspace_root))?;
    owner.set_plan_id(plan_id)?;
    let lock_path = shipper_core::lock::lock_path(&state_dir, Some(&workspace_root));
    let mut retained = Vec::new();
    for artifact in [
        "state.json",
        "events.jsonl",
        "receipt.json",
        "reconciliation.json",
    ] {
        let path = state_dir.join(artifact);
        let bytes = if path.exists() {
            Some(fs::read(&path)?)
        } else {
            None
        };
        retained.push((path, bytes));
    }
    let lock_bytes = fs::read(&lock_path)?;
    retained.push((lock_path, Some(lock_bytes)));

    for force in [false, true] {
        let mut publish = command();
        if force {
            publish.arg("--force");
        }
        let refused = publish.arg("publish").output()?;
        ensure!(refused.status.code() == Some(1));
        ensure!(
            refused.stdout.is_empty(),
            "JSON errors must keep stdout empty"
        );
        let report: serde_json::Value = serde_json::from_slice(&refused.stderr)?;
        ensure!(report["schema_version"] == "shipper.publish.error.v1");
        ensure!(report["command"] == "publish" && report["status"] == "failed");
        ensure!(report["category"] == "state_conflict", "{report}");
        ensure!(report["safe_to_rerun"]["value"] == false);
        ensure!(report["next_action"]["kind"] == "inspect_events");
        ensure!(report["next_action"]["command"].is_null());
        assert_secret_absent_from_output_and_state(&refused, &state_dir);
        for (path, bytes) in &retained {
            let after = if path.exists() {
                Some(fs::read(path)?)
            } else {
                None
            };
            ensure!(after == *bytes, "refusal changed {}", path.display());
        }
        ensure!(read_publish_log(&cargo_log).is_empty());
    }

    owner.release()?;
    let completed = command().arg("publish").output()?;
    ensure!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    ensure!(read_publish_log(&cargo_log).is_empty());
    Ok(())
}
