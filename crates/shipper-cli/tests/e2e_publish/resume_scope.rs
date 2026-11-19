//! Whole-plan completion must include packages outside a resume-from suffix.

use super::*;

struct ScopeFixture {
    workspace: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    cargo_log: std::path::PathBuf,
    api_base: String,
    path: String,
    real_cargo: String,
    fake_cargo: String,
}

impl ScopeFixture {
    fn new(api_base: &str, dependency: bool) -> Result<Self> {
        let workspace = tempdir()?;
        create_independent_workspace(workspace.path());
        if dependency {
            let manifest = workspace.path().join("beta/Cargo.toml");
            let mut contents = fs::read_to_string(&manifest)?;
            contents.push_str(
                "\n[dependencies]\nalpha = { path = \"../alpha\", version = \"0.1.0\" }\n",
            );
            fs::write(manifest, contents)?;
        }
        let (path, real_cargo, fake_cargo) = setup_fake_cargo(workspace.path());
        Ok(Self {
            state_dir: workspace.path().join(".shipper"),
            cargo_log: workspace.path().join("cargo.log"),
            workspace,
            api_base: api_base.into(),
            path,
            real_cargo,
            fake_cargo,
        })
    }

    fn command(&self, parallel: bool, json: bool) -> Command {
        let mut command = loopback_shipper_cmd();
        command
            .current_dir(self.workspace.path())
            .timeout(Duration::from_secs(20))
            .arg("--manifest-path")
            .arg(self.workspace.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&self.state_dir)
            .arg("--api-base")
            .arg(&self.api_base)
            .args([
                "--allow-dirty",
                "--skip-ownership-check",
                "--no-readiness",
                "--policy",
                "fast",
                "--verify-mode",
                "none",
            ])
            .env("PATH", &self.path)
            .env("REAL_CARGO", &self.real_cargo)
            .env("SHIPPER_CARGO_BIN", &self.fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_LOG", &self.cargo_log)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "71")
            .env("CARGO_REGISTRY_TOKEN", "fixture-token");
        if parallel {
            command.arg("--parallel");
        }
        if json {
            command.args(["--format", "json"]);
        }
        command
    }

    fn read(&self, name: &str) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&fs::read(
            self.state_dir.join(name),
        )?)?)
    }

    fn status(&self) -> Result<serde_json::Value> {
        let output = self
            .command(false, true)
            .args(["status", "--durable"])
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn assert_no_publish(&self) {
        assert!(
            read_publish_log(&self.cargo_log).is_empty(),
            "no Cargo publication is authorized by these registry-only fixtures"
        );
    }
    fn seed_prefix_event(
        &self,
        original_events: &str,
        original_receipt: &shipper_core::types::Receipt,
        prior_event: Option<shipper_core::types::EventType>,
    ) -> Result<()> {
        use shipper_core::state::rebuild::{StateRebuildOptions, rebuild_state_from_events};
        let mut events = original_events.to_string();
        if let Some(event_type) = prior_event {
            events.push_str(&serde_json::to_string(
                &shipper_core::types::PublishEvent {
                    timestamp: chrono::Utc::now(),
                    event_type,
                    package: "alpha@0.1.0".into(),
                },
            )?);
            events.push('\n');
        }
        let events_path = self.state_dir.join("events.jsonl");
        fs::write(&events_path, events)?;
        let seeded = rebuild_state_from_events(
            &events_path,
            StateRebuildOptions::new(original_receipt.registry.clone())
                .with_fallback_plan_id(&original_receipt.plan_id),
        )?;
        fs::write(
            self.state_dir.join("state.json"),
            serde_json::to_vec(&seeded)?,
        )?;
        if self.state_dir.join("reconciliation.json").exists() {
            fs::remove_file(self.state_dir.join("reconciliation.json"))?;
        }
        Ok(())
    }
}

#[test]
#[serial_test::serial]
fn resume_scope_pending_prefix_is_partial_in_process_receipt_events_and_status() -> Result<()> {
    for parallel in [false, true] {
        for json in [false, true] {
            let registry = spawn_bounded_registry(vec![200], 1);
            let fixture = ScopeFixture::new(&registry.base_url, true)?;
            let output = fixture
                .command(parallel, json)
                .args(["--resume-from", "beta", "publish"])
                .output()?;
            ensure!(
                output.status.code() == Some(2),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            registry.finish(Duration::from_secs(2))?;

            let state = fixture.read("state.json")?;
            ensure!(state["packages"]["alpha@0.1.0"]["state"]["state"] == "pending");
            ensure!(state["packages"]["alpha@0.1.0"]["attempts"] == 0);
            let receipt = fixture.read("receipt.json")?;
            ensure!(receipt["execution_result"] == "partial_failure");
            ensure!(receipt["packages"].as_array().map(Vec::len) == Some(1));
            ensure!(receipt["packages"][0]["name"] == "beta");
            ensure!(
                receipt["packages"][0]["evidence"]["attempts"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
            );
            let events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
            let events = events
                .lines()
                .map(serde_json::from_str::<serde_json::Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                events.last().context("completion event")?["event_type"]["result"]
                    == "partial_failure"
            );
            ensure!(
                !events
                    .iter()
                    .any(|event| event["event_type"]["type"] == "package_attempted")
            );

            let stdout = String::from_utf8(output.stdout)?;
            if json {
                let report: serde_json::Value = serde_json::from_str(&stdout)?;
                ensure!(report["execution_result"] == "partial_failure");
                ensure!(report["pending"] == 1 && report["skipped"] == 1);
                ensure!(report["packages"].as_array().map(Vec::len) == Some(2));
                ensure!(
                    report["packages"][0]["name"] == "alpha"
                        && report["packages"][0]["state"] == "pending"
                );
                ensure!(report["outcome"]["next_action"]["kind"] == "resume");
                ensure!(
                    report["outcome"]["next_action"]["reason"]
                        .as_str()
                        .context("scope guidance")?
                        .contains("remove --resume-from")
                );
                ensure!(report["outcome"]["safe_to_rerun"]["value"] == false);
            } else {
                ensure!(stdout.contains("Result: partial failure"));
                ensure!(stdout.contains(
                    "alpha@0.1.0: pending (outside this invocation's receipt; durable attempts=0)"
                ));
                ensure!(!stdout.contains("publication is complete"));
                ensure!(human_outcome_value(&stdout, "Next:")?.contains("remove --resume-from"));
                let stderr = String::from_utf8(output.stderr)?;
                ensure!(stderr.contains("Completed 1/2 packages"), "{stderr}");
                ensure!(
                    !stderr.contains("Completed 2/2 packages")
                        && !stderr.contains("Publishing alpha")
                );
            }
            let status = fixture.status()?;
            ensure!(status["outcome"]["execution_result"] == "partial_failure");
            ensure!(status["outcome"]["safe_to_resume"]["value"] == true);
            ensure!(status["outcome"]["next_action"]["kind"] == "resume");
            fixture.assert_no_publish();
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_guidance_correction_reaches_whole_plan_completion() -> Result<()> {
    use shipper_core::types::{ErrorClass, EventType, Receipt};
    const SECRET: &str = "RETRYABLE_PREFIX_SECRET_423";
    for parallel in [false, true] {
        for retryable_prefix in [false, true] {
            // Only beta is checked by the scoped invocation. Removing the
            // scope later checks alpha; terminal beta requires no new query.
            let registry = spawn_bounded_registry(vec![200], 2);
            let fixture = ScopeFixture::new(&registry.base_url, true)?;
            let result = (|| -> Result<()> {
                let initial = fixture
                    .command(parallel, true)
                    .args(["--resume-from", "beta", "publish"])
                    .output()?;
                ensure!(
                    initial.status.code() == Some(2),
                    "{}",
                    String::from_utf8_lossy(&initial.stderr)
                );
                if retryable_prefix {
                    let original_events =
                        fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
                    let receipt: Receipt = serde_json::from_value(fixture.read("receipt.json")?)?;
                    fixture.seed_prefix_event(
                        &original_events,
                        &receipt,
                        Some(EventType::PackageFailed {
                            class: ErrorClass::Retryable,
                            message: SECRET.into(),
                        }),
                    )?;
                }

                let mut guidance = Vec::new();
                for operation in ["publish", "resume"] {
                    for json in [false, true] {
                        let output = fixture
                            .command(parallel, json)
                            .args(["--resume-from", "beta", operation])
                            .output()?;
                        ensure!(
                            output.status.code() == Some(2),
                            "{}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                        let stdout = String::from_utf8(output.stdout)?;
                        let stderr = String::from_utf8(output.stderr)?;
                        ensure!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
                        let reason = if json {
                            let report: serde_json::Value = serde_json::from_str(&stdout)?;
                            ensure!(report["outcome"]["next_action"]["kind"] == "resume");
                            ensure!(report["outcome"]["next_action"].get("command").is_none());
                            ensure!(report["packages"][0]["name"] == "alpha");
                            ensure!(
                                report["packages"][0]["state"]
                                    == if retryable_prefix {
                                        "failed"
                                    } else {
                                        "pending"
                                    }
                            );
                            ensure!(
                                report["receipt"]["packages"].as_array().map(Vec::len) == Some(1)
                            );
                            ensure!(report["receipt"]["packages"][0]["name"] == "beta");
                            report["outcome"]["next_action"]["reason"]
                                .as_str()
                                .context("JSON guidance")?
                                .to_string()
                        } else {
                            human_outcome_value(&stdout, "Next:")?.to_string()
                        };
                        ensure!(
                            reason.contains("remove --resume-from"),
                            "{operation}: {reason}"
                        );
                        ensure!(reason.contains("includes alpha, the first unfinished package omitted by this invocation"), "{reason}");
                        guidance.push(reason);
                    }
                }
                ensure!(
                    guidance.iter().all(|reason| reason == &guidance[0]),
                    "publish/resume human/JSON guidance must agree"
                );

                // Apply the recommended correction: retain the workspace,
                // registry, state and policy, and remove --resume-from.
                let retained_state = fixture.read("state.json")?;
                let retained_events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
                let corrected = fixture.command(parallel, true).arg("resume").output()?;
                ensure!(
                    corrected.status.success(),
                    "{}",
                    String::from_utf8_lossy(&corrected.stderr)
                );
                let report: serde_json::Value = serde_json::from_slice(&corrected.stdout)?;
                let fake_publish_calls = read_publish_log(&fixture.cargo_log);
                let aggregate = serde_json::json!({
                    "parallel": parallel,
                    "retryable_prefix": retryable_prefix,
                    "execution_result": report["execution_result"],
                    "published": report["published"],
                    "skipped": report["skipped"],
                    "pending": report["pending"],
                    "failed": report["failed"],
                    "uploaded": report["uploaded"],
                    "ambiguous": report["ambiguous"],
                    "fake_publish_calls": fake_publish_calls.len(),
                });
                ensure!(
                    report["execution_result"] == "success",
                    "corrected whole-plan aggregates: {aggregate}"
                );
                ensure!(
                    report["published"] == 0
                        && report["skipped"] == 2
                        && report["pending"] == 0
                        && report["failed"] == 0
                        && report["uploaded"] == 0
                        && report["ambiguous"] == 0,
                    "corrected whole-plan aggregates: {aggregate}"
                );
                let packages = report["receipt"]["packages"]
                    .as_array()
                    .context("receipt packages")?;
                ensure!(packages.len() == 2);
                ensure!(
                    packages
                        .iter()
                        .all(|package| package["state"]["state"] == "skipped"),
                    "visible receipt packages must both be skipped: {aggregate}"
                );
                // Both Pending and retained Retryable work must check registry
                // visibility before Cargo. The rejecting fake stays armed to
                // expose any unnecessary upload of the already-visible prefix.
                ensure!(
                    fake_publish_calls.is_empty(),
                    "unexpected fake publication attempts: {aggregate}"
                );
                for package in packages {
                    ensure!(
                        package["evidence"]["attempts"]
                            .as_array()
                            .context("attempt evidence")?
                            .is_empty(),
                        "visible packages must not gain attempt evidence"
                    );
                }
                let final_state = fixture.read("state.json")?;
                ensure!(
                    final_state["attempt_history"]
                        .as_array()
                        .context("final attempt history")?
                        == retained_state["attempt_history"]
                            .as_array()
                            .context("retained attempt history")?,
                    "visible scope correction must preserve cumulative attempts"
                );
                for key in ["alpha@0.1.0", "beta@0.1.0"] {
                    let retained_attempts = retained_state["packages"][key]["attempts"]
                        .as_u64()
                        .context("retained attempt counter")?;
                    ensure!(
                        final_state["packages"][key]["attempts"].as_u64()
                            == Some(retained_attempts)
                    );
                    ensure!(final_state["packages"][key]["state"]["state"] == "skipped");
                }
                let events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
                ensure!(
                    events.starts_with(&retained_events),
                    "scope correction must preserve predecessor event history"
                );
                for line in events.lines() {
                    let event: serde_json::Value = serde_json::from_str(line)?;
                    ensure!(
                        event["event_type"]["type"] != "package_uploaded",
                        "the fixture must never accept an upload"
                    );
                }
                ensure!(report["outcome"]["next_action"]["kind"] == "none_complete");
                let status = fixture.status()?;
                ensure!(status["outcome"]["execution_result"] == "success");
                ensure!(status["outcome"]["next_action"]["kind"] == "none_complete");
                Ok(())
            })();
            let registry_result = registry.finish(Duration::from_secs(2));
            result?;
            registry_result?;
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_terminal_prefix_remains_success_without_a_fabricated_receipt() -> Result<()> {
    for parallel in [false, true] {
        let registry = spawn_bounded_registry(vec![200], 2);
        let fixture = ScopeFixture::new(&registry.base_url, true)?;
        let first = fixture.command(parallel, true).arg("publish").output()?;
        ensure!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        registry.finish(Duration::from_secs(2))?;
        let original = fixture.read("state.json")?["packages"]["alpha@0.1.0"].clone();
        let output = fixture
            .command(parallel, true)
            .args(["--resume-from", "beta", "resume"])
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        ensure!(report["execution_result"] == "success");
        ensure!(report["skipped"] == 2 && report["pending"] == 0);
        ensure!(report["packages"].as_array().map(Vec::len) == Some(2));
        ensure!(report["receipt"]["packages"].as_array().map(Vec::len) == Some(1));
        ensure!(report["receipt"]["packages"][0]["name"] == "beta");
        ensure!(
            report["receipt"]["packages"][0]["evidence"]["attempts"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        ensure!(
            fixture.read("state.json")?["packages"]["alpha@0.1.0"] == original,
            "omitted predecessor state and attempts must remain untouched"
        );
        let status = fixture.status()?;
        ensure!(status["outcome"]["execution_result"] == "success");
        ensure!(status["outcome"]["next_action"]["kind"] == "none_complete");
        fixture.assert_no_publish();
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_durable_rejects_deleted_only_selected_terminal_receipt() -> Result<()> {
    for parallel in [false, true] {
        let registry = spawn_bounded_registry(vec![200], 2);
        let fixture = ScopeFixture::new(&registry.base_url, true)?;
        let publish = fixture.command(parallel, true).arg("publish").output()?;
        ensure!(
            publish.status.success(),
            "{}",
            String::from_utf8_lossy(&publish.stderr)
        );
        registry.finish(Duration::from_secs(2))?;
        let resume = fixture
            .command(parallel, true)
            .args(["--resume-from", "beta", "resume"])
            .output()?;
        ensure!(
            resume.status.success(),
            "{}",
            String::from_utf8_lossy(&resume.stderr)
        );

        let original_receipt = fixture.read("receipt.json")?;
        ensure!(original_receipt["packages"].as_array().map(Vec::len) == Some(1));
        ensure!(original_receipt["packages"][0]["name"] == "beta");
        let original_status = fixture.status()?;
        ensure!(original_status["outcome"]["status"] == "terminal");
        ensure!(original_status["outcome"]["execution_result"] == "success");
        ensure!(original_status["outcome"]["next_action"]["kind"] == "none_complete");
        let original_state = fs::read(fixture.state_dir.join("state.json"))?;
        let original_events = fs::read(fixture.state_dir.join("events.jsonl"))?;

        let mut empty_receipt = original_receipt.clone();
        empty_receipt["packages"] = serde_json::json!([]);
        fs::write(
            fixture.state_dir.join("receipt.json"),
            serde_json::to_vec(&empty_receipt)?,
        )?;
        let status = fixture.status()?;
        ensure!(
            status["outcome"]["status"] == "evidence_disagreement",
            "{status}"
        );
        ensure!(status["outcome"]["safe_to_resume"]["value"] == false);
        ensure!(status["outcome"]["next_action"]["kind"] == "stop_and_investigate");
        ensure!(fs::read(fixture.state_dir.join("state.json"))? == original_state);
        ensure!(fs::read(fixture.state_dir.join("events.jsonl"))? == original_events);

        fs::write(
            fixture.state_dir.join("receipt.json"),
            serde_json::to_vec(&original_receipt)?,
        )?;
        let restored = fixture.status()?;
        ensure!(restored["outcome"]["status"] == "terminal");
        ensure!(restored["outcome"]["next_action"]["kind"] == "none_complete");
        fixture.assert_no_publish();
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_parallel_includes_the_targets_whole_dependency_level() -> Result<()> {
    for parallel in [false, true] {
        let registry = spawn_bounded_registry(vec![200], if parallel { 2 } else { 1 });
        let fixture = ScopeFixture::new(&registry.base_url, false)?;
        let output = fixture
            .command(parallel, true)
            .args(["--resume-from", "beta", "publish"])
            .output()?;
        ensure!(output.status.code() == Some(if parallel { 0 } else { 2 }));
        registry.finish(Duration::from_secs(2))?;
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        ensure!(report["pending"] == if parallel { 0 } else { 1 });
        ensure!(
            report["receipt"]["packages"].as_array().map(Vec::len)
                == Some(if parallel { 2 } else { 1 })
        );
        fixture.assert_no_publish();
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_keeps_failed_uploaded_unknown_and_missing_prefix_dispositions() -> Result<()> {
    use shipper_core::types::{ErrorClass, EventType, Receipt, ReconciliationOutcome};

    for parallel in [false, true] {
        let registry = spawn_bounded_registry(vec![200], 1);
        let fixture = ScopeFixture::new(&registry.base_url, true)?;
        let initial = fixture
            .command(parallel, true)
            .args(["--resume-from", "beta", "publish"])
            .output()?;
        ensure!(initial.status.code() == Some(2));
        registry.finish(Duration::from_secs(2))?;
        let original_events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
        let original_receipt: Receipt = serde_json::from_value(fixture.read("receipt.json")?)?;

        for (prior_event, state_name, action) in [
            (None, "pending", "resume"),
            (
                Some(EventType::PackageUploaded),
                "uploaded",
                "wait_for_registry",
            ),
            (
                Some(EventType::PackageFailed {
                    class: ErrorClass::Retryable,
                    message: "retry later".into(),
                }),
                "failed",
                "resume",
            ),
            (
                Some(EventType::PackageFailed {
                    class: ErrorClass::Permanent,
                    message: "resolve blocker".into(),
                }),
                "failed",
                "resolve_blockers",
            ),
            (
                Some(EventType::PublishReconciled {
                    outcome: ReconciliationOutcome::StillUnknown {
                        attempts: 1,
                        elapsed_ms: 1,
                        reason: "registry unavailable".into(),
                    },
                }),
                "ambiguous",
                "reconcile",
            ),
        ] {
            fixture.seed_prefix_event(&original_events, &original_receipt, prior_event)?;
            let output = fixture
                .command(parallel, true)
                .args(["--resume-from", "beta", "publish"])
                .output()?;
            ensure!(
                output.status.code() == Some(2),
                "{state_name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            ensure!(report["execution_result"] == "partial_failure");
            ensure!(report["packages"][0]["state"] == state_name);
            ensure!(report["packages"][0]["attempts"] == 0);
            ensure!(report["receipt"]["packages"].as_array().map(Vec::len) == Some(1));
            ensure!(
                report["outcome"]["next_action"]["kind"] == action,
                "{report}"
            );
            if action == "resolve_blockers" {
                ensure!(
                    report["receipt"]["packages"][0]["name"] == "beta",
                    "the permanent predecessor has no receipt entry"
                );
                let expected = report["outcome"]["next_action"]["reason"]
                    .as_str()
                    .context("permanent guidance")?;
                ensure!(expected.contains("retained state and event evidence"));
                ensure!(!expected.contains("recorded in the receipt"));
                for (operation, json) in [("publish", false), ("resume", false), ("resume", true)] {
                    let output = fixture
                        .command(parallel, json)
                        .args(["--resume-from", "beta", operation])
                        .output()?;
                    ensure!(
                        output.status.code() == Some(2),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let stdout = String::from_utf8(output.stdout)?;
                    let actual = if json {
                        let output: serde_json::Value = serde_json::from_str(&stdout)?;
                        ensure!(output["outcome"]["next_action"]["kind"] == "resolve_blockers");
                        output["outcome"]["next_action"]["reason"]
                            .as_str()
                            .context("permanent JSON guidance")?
                            .to_string()
                    } else {
                        human_outcome_value(&stdout, "Next:")?.to_string()
                    };
                    ensure!(
                        actual == expected,
                        "permanent evidence guidance must agree for publish/resume human/JSON"
                    );
                }
            }
            ensure!(report["outcome"]["safe_to_rerun"]["value"] == false);
            let status = fixture.status()?;
            ensure!(status["outcome"]["execution_result"] == "partial_failure");
            ensure!(
                status["outcome"]["safe_to_resume"]["value"] == (action == "resume"),
                "{status}"
            );
            fixture.assert_no_publish();
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_omitted_prefix_messages_are_not_printed() -> Result<()> {
    use shipper_core::types::{ErrorClass, EventType, Receipt, ReconciliationOutcome};
    const SECRET: &str = "OMITTED_PREFIX_SECRET_415_MUST_STAY_PRIVATE";
    for parallel in [false, true] {
        let registry = spawn_bounded_registry(vec![200], 1);
        let fixture = ScopeFixture::new(&registry.base_url, true)?;
        let initial = fixture
            .command(parallel, true)
            .args(["--resume-from", "beta", "publish"])
            .output()?;
        ensure!(initial.status.code() == Some(2));
        registry.finish(Duration::from_secs(2))?;
        let original_events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
        let original_receipt: Receipt = serde_json::from_value(fixture.read("receipt.json")?)?;
        for (prior_event, expected_state) in [
            (
                EventType::PackageFailed {
                    class: ErrorClass::Permanent,
                    message: SECRET.into(),
                },
                "failed",
            ),
            (
                EventType::PublishReconciled {
                    outcome: ReconciliationOutcome::StillUnknown {
                        attempts: 1,
                        elapsed_ms: 1,
                        reason: SECRET.into(),
                    },
                },
                "ambiguous",
            ),
        ] {
            fixture.seed_prefix_event(&original_events, &original_receipt, Some(prior_event))?;
            ensure!(
                fs::read_to_string(fixture.state_dir.join("state.json"))?.contains(SECRET),
                "the fixture must retain the sensitive payload"
            );
            let output = fixture
                .command(parallel, false)
                .args(["--resume-from", "beta", "publish"])
                .output()?;
            ensure!(output.status.code() == Some(2));
            let stdout = String::from_utf8(output.stdout)?;
            let stderr = String::from_utf8(output.stderr)?;
            ensure!(stdout.contains(&format!("alpha@0.1.0: {expected_state} (outside this invocation's receipt; durable attempts=0)")), "{stdout}");
            ensure!(
                !stdout.contains(SECRET) && !stderr.contains(SECRET),
                "omitted prefix output must not print raw stored messages"
            );
            fixture.assert_no_publish();
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn resume_scope_durable_oracle_rejects_deleted_receipts_missing_state_and_false_success()
-> Result<()> {
    let registry = spawn_bounded_registry(vec![200], 1);
    let fixture = ScopeFixture::new(&registry.base_url, true)?;
    let output = fixture
        .command(false, true)
        .args(["--resume-from", "beta", "publish"])
        .output()?;
    ensure!(output.status.code() == Some(2));
    registry.finish(Duration::from_secs(2))?;
    let original_receipt = fixture.read("receipt.json")?;
    let original_state = fixture.read("state.json")?;
    let original_events = fs::read_to_string(fixture.state_dir.join("events.jsonl"))?;
    for corruption in [
        "receipt_entry",
        "planned_state",
        "prefix_identity",
        "success",
    ] {
        let mut receipt = original_receipt.clone();
        let mut state = original_state.clone();
        let mut events = original_events.clone();
        match corruption {
            "receipt_entry" => receipt["packages"] = serde_json::json!([]),
            "planned_state" => {
                state["packages"]
                    .as_object_mut()
                    .context("state packages")?
                    .remove("alpha@0.1.0");
            }
            "prefix_identity" => {
                state["packages"]["alpha@0.1.0"]["name"] = serde_json::json!("wrong-package");
            }
            "success" => {
                receipt["execution_result"] = serde_json::json!("success");
                events = events.replace("\"partial_failure\"", "\"success\"");
            }
            _ => unreachable!(),
        }
        fs::write(
            fixture.state_dir.join("receipt.json"),
            serde_json::to_vec(&receipt)?,
        )?;
        fs::write(
            fixture.state_dir.join("state.json"),
            serde_json::to_vec(&state)?,
        )?;
        fs::write(fixture.state_dir.join("events.jsonl"), events)?;
        let status = fixture.status()?;
        ensure!(
            status["outcome"]["status"] == "evidence_disagreement",
            "{corruption}: {status}"
        );
        ensure!(status["outcome"]["safe_to_resume"]["value"] == false);
    }
    fixture.assert_no_publish();
    Ok(())
}
