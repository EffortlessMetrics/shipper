//! Consume the actual generated shell step and the documented workflow wrapper.
//! Artifact actions are structurally checked; no hosted transfer is simulated.

use super::*;
use serde_yaml::Value;
use serial_test::serial;

const SECRET: &str = "ci-recipe-secret-sentinel";
const GUIDE: &str = include_str!("../../../../docs/how-to/run-in-github-actions.md");

fn documented_workflow() -> Result<Value> {
    documented_yaml_after("## Minimal workflow\n")
}

fn documented_yaml_after(heading: &str) -> Result<Value> {
    let section = GUIDE
        .split_once(heading)
        .context("documented workflow section")?
        .1;
    let yaml = section
        .split_once("```yaml\n")
        .context("workflow fence")?
        .1
        .split_once("\n```")
        .context("workflow closing fence")?
        .0;
    Ok(serde_yaml::from_str(yaml)?)
}

fn documented_auth_steps() -> Result<Vec<Value>> {
    let workflow = documented_yaml_after("### Token vs Trusted Publishing\n")?;
    ensure!(workflow["permissions"]["id-token"] == "write");
    ensure!(workflow["jobs"]["publish"]["environment"] == "release");
    Ok(workflow["jobs"]["publish"]["steps"]
        .as_sequence()
        .context("documented auth steps")?
        .clone())
}

#[test]
fn ci_github_actions_auth_example_keeps_explicit_fallback_control() -> Result<()> {
    let steps = documented_auth_steps()?;
    let auth_index = steps
        .iter()
        .position(|step| step["id"] == "auth")
        .context("auth action")?;
    let auth = &steps[auth_index];
    ensure!(auth["uses"] == "rust-lang/crates-io-auth-action@v1");
    ensure!(
        auth["continue-on-error"] == true,
        "failed mint must reach the explicit fallback"
    );
    let publish = publish_step(&steps)?;
    ensure!(
        steps.get(auth_index + 1) == Some(publish),
        "token selection must immediately follow authentication"
    );
    ensure!(publish["shell"] == "bash");
    ensure!(publish["if"].is_null() && publish["continue-on-error"].is_null());
    ensure!(
        publish["env"]["CARGO_REGISTRY_TOKEN"]
            == "${{ steps.auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN }}",
        "keep the release owner's selected-token expression"
    );
    ensure!(publish["env"]["SHIPPER_TOKEN_MINTED"] == "${{ steps.auth.outputs.token != '' }}");
    Ok(())
}

#[test]
#[cfg(unix)]
#[serial]
fn ci_github_actions_auth_example_selects_tokens_and_refuses_missing_credentials() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    const MINTED: &str = "auth-guide-minted-sentinel";
    const FALLBACK: &str = "auth-guide-fallback-sentinel";
    ci_github_actions_auth_example_keeps_explicit_fallback_control()?;
    let steps = documented_auth_steps()?;
    let script = publish_step(&steps)?["run"]
        .as_str()
        .context("documented auth publish script")?;
    // The exact expressions and continuation setting are checked above. Feed
    // their synthetic string results to the actual documented Bash; this is
    // not a GitHub Actions interpreter or a real token-exchange test.
    for (name, minted, fallback, expected, source) in [
        (
            "minted wins",
            MINTED,
            FALLBACK,
            Some(MINTED),
            "trusted_publishing",
        ),
        (
            "failed mint with fallback",
            "",
            FALLBACK,
            Some(FALLBACK),
            "fallback_secret",
        ),
        (
            "minted only",
            MINTED,
            "",
            Some(MINTED),
            "trusted_publishing",
        ),
        ("neither token", "", "", None, ""),
        ("blank fallback", "", " \t\r\n", None, ""),
        ("blank selected mint", " \t", FALLBACK, None, ""),
    ] {
        let runner = tempdir()?;
        let bin = runner.path().join("bin");
        fs::create_dir(&bin)?;
        let spy = bin.join("shipper");
        fs::write(
            &spy,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SHIPPER_AUTH_TEST_CALLS\"\ntest \"${CARGO_REGISTRY_TOKEN-}\" = \"$SHIPPER_AUTH_TEST_EXPECTED\"\n",
        )?;
        fs::set_permissions(&spy, fs::Permissions::from_mode(0o755))?;
        let calls = runner.path().join("publish-calls");
        let mut search_paths = vec![bin];
        search_paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").context("Bash fixture search path")?,
        ));
        let fixture_path = std::env::join_paths(search_paths)?;
        let selected = if minted.is_empty() { fallback } else { minted };
        let output = Command::new("bash")
            .current_dir(runner.path())
            .timeout(Duration::from_secs(10))
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                script,
            ])
            .env_clear()
            .env("PATH", fixture_path)
            .env("CARGO_REGISTRY_TOKEN", selected)
            .env("SHIPPER_TOKEN_MINTED", (!minted.is_empty()).to_string())
            .env("SHIPPER_AUTH_TEST_EXPECTED", expected.unwrap_or(""))
            .env("SHIPPER_AUTH_TEST_CALLS", &calls)
            .output()?;
        ensure!(
            output.status.success() == expected.is_some(),
            "auth branch outcome differed: {name}"
        );
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;
        if expected.is_some() {
            ensure!(fs::read_to_string(&calls)? == "publish --policy safe\n");
            ensure!(stdout.contains(&format!("Publish token source: {source}")));
            ensure!(stderr.is_empty());
        } else {
            ensure!(
                !calls.exists(),
                "missing-token refusal invoked publish: {name}"
            );
            ensure!(stderr.contains("No usable publish token"));
            ensure!(!stdout.contains("Publish token source:"));
        }
        for sentinel in [MINTED, FALLBACK] {
            ensure!(
                !stdout.contains(sentinel) && !stderr.contains(sentinel),
                "auth example exposed a synthetic credential"
            );
            if calls.exists() {
                ensure!(!fs::read_to_string(&calls)?.contains(sentinel));
            }
        }
    }
    Ok(())
}

fn generate(root: &Path, state_dir: &str) -> Result<(String, Vec<Value>)> {
    // A local negative-control run can use an unchanged, retained old generator.
    // This is a test-only selector, not a CLI or production behavior switch.
    let mut command = match std::env::var_os("SHIPPER_CI_GENERATOR_TEST_BIN") {
        Some(path) => Command::new(path),
        None => shipper_cmd(),
    };
    let output = command
        .current_dir(root)
        .timeout(Duration::from_secs(20))
        .args(["--state-dir", state_dir, "ci", "github-actions"])
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .output()?;
    ensure!(
        output.status.success(),
        "generator failed: {:?}",
        output.status
    );
    let text = String::from_utf8(output.stdout)?;
    ensure!(!text.contains(SECRET), "generator exposed a credential");
    let parsed: Value = serde_yaml::from_str(&text)?;
    let steps = parsed.as_sequence().context("generated steps sequence")?;
    Ok((text, steps.clone()))
}

fn publish_step(steps: &[Value]) -> Result<&Value> {
    steps
        .iter()
        .find(|step| {
            step["run"]
                .as_str()
                .is_some_and(|script| script.contains("shipper publish"))
        })
        .context("generated publish step")
}

fn upload_step(steps: &[Value]) -> Result<&Value> {
    steps
        .iter()
        .rev()
        .find(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|action| action.starts_with("actions/upload-artifact@"))
        })
        .context("generated upload step")
}

#[test]
#[serial]
fn ci_github_actions_assembles_the_documented_workflow_and_literal_paths() -> Result<()> {
    let generator = tempdir()?;
    create_single_crate_workspace(generator.path());
    let workflow = documented_workflow()?;
    ensure!(workflow["permissions"]["contents"] == "read");
    ensure!(workflow["on"]["workflow_dispatch"].is_mapping());
    let job = &workflow["jobs"]["publish"];
    ensure!(job["environment"] == "release");
    let documented = job["steps"].as_sequence().context("documented steps")?;

    for path in [
        ".shipper",
        "release evidence",
        "release 'quoted' $literal",
        "~evidence/é",
    ] {
        let (text, steps) = generate(generator.path(), path)?;
        ensure!(
            steps.len() == 2,
            "fresh recipe must contain publish and retention"
        );
        ensure!(!text.contains(&generator.path().display().to_string()));
        ensure!(!text.contains("actions/cache") && !text.contains("actions/download-artifact"));
        let publish = publish_step(&steps)?;
        let upload = upload_step(&steps)?;
        let normalized = format!("./{path}");
        ensure!(publish["shell"] == "bash");
        ensure!(publish["working-directory"] == "${{ github.workspace }}");
        ensure!(publish["env"]["SHIPPER_STATE_DIR"] == normalized);
        ensure!(upload["uses"] == "actions/upload-artifact@v7");
        ensure!(upload["if"] == "always()");
        ensure!(upload["with"]["include-hidden-files"] == true);
        ensure!(upload["with"]["if-no-files-found"] == "error");
        ensure!(upload["with"]["path"] == format!("{normalized}/"));
        ensure!(
            upload["with"]["name"]
                == "shipper-state-${{ github.run_id }}-${{ github.run_attempt }}"
        );
        if path == ".shipper" {
            ensure!(
                documented.ends_with(&steps),
                "guide and actual generated steps diverged"
            );
            ensure!(job["env"]["SHIPPER_STATE_DIR"] == publish["env"]["SHIPPER_STATE_DIR"]);
        }
        let mut assembled = workflow.clone();
        let assembled_steps = assembled["jobs"]["publish"]["steps"]
            .as_sequence_mut()
            .context("assembled steps")?;
        assembled_steps.truncate(assembled_steps.len() - 2);
        assembled_steps.extend(steps);
        assembled["jobs"]["publish"]["env"]["SHIPPER_STATE_DIR"] = Value::String(normalized);
        let roundtrip: Value = serde_yaml::from_str(&serde_yaml::to_string(&assembled)?)?;
        ensure!(roundtrip == assembled, "assembled workflow roundtrip");
    }
    Ok(())
}

#[test]
#[serial]
fn ci_github_actions_refuses_nonportable_or_interpreted_paths_without_output() -> Result<()> {
    let workspace = tempdir()?;
    create_single_crate_workspace(workspace.path());
    for path in [
        "/tmp/machine-only",
        "../state",
        ".",
        "",
        "C:\\release",
        "logs/*",
        "!state",
        "${{ secrets.CARGO_REGISTRY_TOKEN }}",
        "line\nbreak",
    ] {
        let output = shipper_cmd()
            .current_dir(workspace.path())
            .timeout(Duration::from_secs(20))
            .args(["--state-dir", path, "ci", "github-actions"])
            .output()?;
        ensure!(!output.status.success(), "unsupported path accepted");
        ensure!(
            output.stdout.is_empty(),
            "refused path emitted partial YAML"
        );
        ensure!(
            !String::from_utf8_lossy(&output.stderr).contains(path)
                || path.is_empty()
                || path == ".",
            "path value echoed in diagnostic"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn install_shippershim(root: &Path) -> Result<(String, String, String)> {
    use std::os::unix::fs::PermissionsExt;
    let environment = setup_fake_cargo(root);
    let shim = root.join("fake-bin/shipper");
    fs::write(&shim, "#!/bin/sh\nexec \"$SHIPPER_RECIPE_CLI\" \"$@\"\n")?;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
    Ok(environment)
}

#[cfg(unix)]
fn install_refusal_spy(root: &Path) -> Result<()> {
    // Prove refusal at the generated shell boundary. A real CLI could reject
    // malformed history itself and let a deleted shell guard escape detection.
    fs::write(
        root.join("fake-bin/shipper"),
        "#!/bin/sh\nprintf '%s\\n' 'unexpected shipper invocation' >> \"$SHIPPER_FAKE_PUBLISH_LOG\"\nexit 0\n",
    )?;
    Ok(())
}

#[cfg(unix)]
fn run_script(
    root: &Path,
    script: &str,
    state_dir: &str,
    environment: &(String, String, String),
    exit: &str,
) -> Result<std::process::Output> {
    let mut command = Command::new("bash");
    command
        .current_dir(root)
        .timeout(Duration::from_secs(20))
        .args([
            "--noprofile",
            "--norc",
            "-e",
            "-o",
            "pipefail",
            "-c",
            script,
        ])
        .env("PATH", &environment.0)
        .env("REAL_CARGO", &environment.1)
        .env("SHIPPER_CARGO_BIN", &environment.2)
        .env(
            "SHIPPER_RECIPE_CLI",
            assert_cmd::cargo::cargo_bin!("shipper-cli"),
        )
        .env("SHIPPER_STATE_DIR", state_dir)
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .env("SHIPPER_FAKE_PUBLISH_LOG", root.join("cargo-publish-calls"))
        .env("SHIPPER_FAKE_PUBLISH_EXIT", exit)
        .env(
            "SHIPPER_FAKE_PUBLISH_STDERR",
            format!("error: 401 Unauthorized\nCARGO_REGISTRY_TOKEN={SECRET}"),
        );
    Ok(command.output()?)
}

#[cfg(unix)]
fn configure_registry(root: &Path, base: &str) -> Result<()> {
    fs::write(
        root.join(".shipper.toml"),
        format!(
            r#"
[flags]
allow_dirty = true
skip_ownership_check = true
[verify]
mode = "none"
[readiness]
method = "api"
initial_delay = "0ms"
poll_interval = "1ms"
max_delay = "1ms"
max_total_wait = "50ms"
jitter_factor = 0.0
[retry]
policy = "custom"
max_attempts = 1
[registry]
name = "ci-fixture"
api_base = "{base}"
index_base = "{base}"
[rehearsal]
registry = "ci-fixture"
allow_loopback = true
"#
        ),
    )?;
    Ok(())
}

#[cfg(unix)]
fn files_below(root: &Path) -> Result<Vec<(std::path::PathBuf, Vec<u8>)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(files_below(&path)?);
        } else {
            files.push((path.clone(), fs::read(path)?));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

#[cfg(unix)]
fn generated_publication(state_dir: &str, success: bool) -> Result<()> {
    let generator = tempdir()?;
    create_single_crate_workspace(generator.path());
    let (_, steps) = generate(generator.path(), state_dir)?;
    let publish = publish_step(&steps)?;
    let script = publish["run"].as_str().context("publish script")?;
    let artifact_path = upload_step(&steps)?["with"]["path"]
        .as_str()
        .context("artifact path")?;
    let runner = tempdir()?;
    create_single_crate_workspace(runner.path());
    let environment = install_shippershim(runner.path())?;
    // Preflight version + new-crate probes; publish version + readiness/failure probe.
    let statuses = if success {
        vec![404, 404, 404, 200]
    } else {
        vec![404; 4]
    };
    let registry = spawn_bounded_registry(statuses, 4);
    configure_registry(runner.path(), &registry.base_url)?;
    let workflow = documented_workflow()?;
    let preflight = workflow["jobs"]["publish"]["steps"]
        .as_sequence()
        .context("workflow steps")?
        .iter()
        .find(|step| step["name"] == "Preflight")
        .context("documented preflight")?["run"]
        .as_str()
        .context("preflight script")?;
    let normalized = format!("./{state_dir}");
    let preflight_output = run_script(runner.path(), preflight, &normalized, &environment, "0")?;
    ensure!(
        preflight_output.status.success(),
        "documented preflight failed: {}",
        String::from_utf8_lossy(&preflight_output.stderr)
    );
    let evidence = runner.path().join(state_dir);
    ensure!(
        !evidence.join("events.jsonl").exists() && !evidence.join("state.json").exists(),
        "fresh audit created authoritative publication history"
    );
    ensure!(
        fs::read_dir(&evidence)?.any(|entry| entry.is_ok_and(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("preflight-only-"))),
        "preflight sidecar absent"
    );

    let output = run_script(
        runner.path(),
        script,
        &normalized,
        &environment,
        if success { "0" } else { "1" },
    )?;
    ensure!(
        output.status.success() == success,
        "generated publish outcome: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    registry.finish(Duration::from_secs(3))?;
    let calls = fs::read_to_string(runner.path().join("cargo-publish-calls"))?;
    ensure!(
        calls.lines().count() == 1,
        "generated invocation changed Cargo count: {calls}"
    );
    ensure!(
        runner.path().join(artifact_path) == evidence,
        "artifact path and actual selected state directory differ"
    );
    ensure!(
        evidence.join("state.json").is_file() && evidence.join("events.jsonl").is_file(),
        "generated command did not write evidence at its upload path"
    );
    ensure!(
        evidence.join("receipt.json").is_file() == success,
        "receipt did not match actual completion"
    );
    let state =
        shipper_core::state::execution_state::load_state(&evidence)?.context("published state")?;
    let log_path = evidence.join("events.jsonl");
    let log = shipper_core::state::events::EventLog::read_from_file(&log_path)?;
    ensure!(
        log.all_events()
            .iter()
            .filter(|event| matches!(
                event.event_type,
                shipper_core::types::EventType::PackageAttempted { .. }
            ))
            .count()
            == 1
    );
    ensure!(
        shipper_core::state::consistency::verify_events_state_consistency(&log_path, &state)?
            .is_consistent(),
        "event/state mismatch"
    );
    let retained = files_below(&evidence)?;
    for (_, bytes) in &retained {
        ensure!(
            !bytes
                .windows(SECRET.len())
                .any(|window| window == SECRET.as_bytes()),
            "credential reached retained evidence"
        );
    }
    for bytes in [
        &preflight_output.stdout,
        &preflight_output.stderr,
        &output.stdout,
        &output.stderr,
    ] {
        ensure!(
            !bytes
                .windows(SECRET.len())
                .any(|window| window == SECRET.as_bytes()),
            "credential reached command output"
        );
    }
    install_refusal_spy(runner.path())?;
    let refusal = run_script(runner.path(), script, &normalized, &environment, "0")?;
    ensure!(
        !refusal.status.success(),
        "fresh recipe accepted retained execution evidence"
    );
    ensure!(
        fs::read_to_string(runner.path().join("cargo-publish-calls"))? == calls,
        "refusal started another Cargo publish"
    );
    ensure!(
        files_below(&evidence)? == retained,
        "refusal altered retained evidence"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
#[serial]
fn ci_github_actions_generated_default_success_and_failure_retain_evidence() -> Result<()> {
    for success in [true, false] {
        generated_publication(".shipper", success)?;
    }
    Ok(())
}

#[test]
#[cfg(unix)]
#[serial]
fn ci_github_actions_generated_custom_path_success_and_failure_retain_evidence() -> Result<()> {
    for success in [true, false] {
        generated_publication("release 'evidence' $literal", success)?;
    }
    Ok(())
}

#[test]
#[cfg(unix)]
#[serial]
fn ci_github_actions_fresh_guard_preserves_each_retained_evidence_path() -> Result<()> {
    let workspace = tempdir()?;
    create_single_crate_workspace(workspace.path());
    let (_, steps) = generate(workspace.path(), ".shipper")?;
    let script = publish_step(&steps)?["run"]
        .as_str()
        .context("publish script")?;
    let environment = install_shippershim(workspace.path())?;
    install_refusal_spy(workspace.path())?;
    let evidence = workspace.path().join(".shipper");
    fs::create_dir(&evidence)?;
    for name in [
        "state.json",
        "events.jsonl",
        "receipt.json",
        "reconciliation.json",
        "registry-a/state.json",
        "registry-b/events.jsonl",
    ] {
        let path = evidence.join(name);
        fs::create_dir_all(path.parent().context("evidence parent")?)?;
        let bytes = b"deliberately malformed retained evidence\n";
        fs::write(&path, bytes)?;
        let output = run_script(workspace.path(), script, "./.shipper", &environment, "0")?;
        ensure!(
            !output.status.success(),
            "generated freshness check invoked shipper for retained evidence"
        );
        ensure!(fs::read(&path)? == bytes);
        ensure!(!workspace.path().join("cargo-publish-calls").exists());
        fs::remove_file(path)?;
    }
    let dangling = evidence.join("events.jsonl");
    std::os::unix::fs::symlink("missing-evidence-target", &dangling)?;
    ensure!(
        !run_script(workspace.path(), script, "./.shipper", &environment, "0")?
            .status
            .success()
    );
    ensure!(fs::symlink_metadata(&dangling)?.file_type().is_symlink());
    ensure!(!workspace.path().join("cargo-publish-calls").exists());
    fs::remove_file(dangling)?;
    std::os::unix::fs::symlink(".", evidence.join("loop"))?;
    ensure!(
        !run_script(workspace.path(), script, "./.shipper", &environment, "0")?
            .status
            .success(),
        "an incomplete freshness scan authorized publication"
    );
    ensure!(!workspace.path().join("cargo-publish-calls").exists());
    Ok(())
}

#[test]
#[cfg(unix)]
#[serial]
fn ci_github_actions_ordinary_preflight_is_not_fresh_publication_authority() -> Result<()> {
    let workspace = tempdir()?;
    create_single_crate_workspace(workspace.path());
    let (_, steps) = generate(workspace.path(), ".shipper")?;
    let environment = install_shippershim(workspace.path())?;
    let registry = spawn_bounded_registry(vec![404; 2], 2);
    configure_registry(workspace.path(), &registry.base_url)?;
    let workflow = documented_workflow()?;
    let preflight = workflow["jobs"]["publish"]["steps"]
        .as_sequence()
        .context("steps")?
        .iter()
        .find(|step| step["name"] == "Preflight")
        .context("preflight")?["run"]
        .as_str()
        .context("script")?
        .replace("--preflight-only ", "");
    ensure!(
        run_script(
            workspace.path(),
            &preflight,
            "./.shipper",
            &environment,
            "0"
        )?
        .status
        .success()
    );
    registry.finish(Duration::from_secs(3))?;
    let events = workspace.path().join(".shipper/events.jsonl");
    let before = fs::read(&events)?;
    install_refusal_spy(workspace.path())?;
    let script = publish_step(&steps)?["run"]
        .as_str()
        .context("publish script")?;
    ensure!(
        !run_script(workspace.path(), script, "./.shipper", &environment, "0")?
            .status
            .success()
    );
    ensure!(fs::read(events)? == before);
    ensure!(!workspace.path().join("cargo-publish-calls").exists());
    Ok(())
}
