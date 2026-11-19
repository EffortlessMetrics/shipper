//! Test-only phase entry points for the separate-run rehearsal in #465.
//! This adds no Actions transport or publication authority by itself.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::process::Output;

use super::*;

const PHASE_ENV: &str = "SHIPPER_CROSS_RUN_PHASE";
const PHASE_TEST: &str = "cross_run::hosted_recovery_phase";

fn assert_predecessor(state_dir: &Path, attempts: usize) -> Vec<u8> {
    // Validate before rewriting fixtures, starting a registry, or invoking Cargo.
    // In particular a final continuation must not accept the original seed.
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(state_dir.join("state.json")).expect("predecessor state"))
            .expect("predecessor state JSON");
    assert_eq!(
        state["packages"]["crate-c@0.1.0"]["attempts"], attempts,
        "immediate predecessor must retain the expected cumulative attempts"
    );
    assert_live_rehearsal_interrupted_state(state_dir);
    assert!(!state_dir.join("receipt.json").exists());
    let log = fake_cargo_log(state_dir);
    for name in ["crate-a", "crate-b"] {
        assert_eq!(count_fake_cargo_publishes(&log, name), 1);
    }
    assert_eq!(count_fake_cargo_publishes(&log, "crate-c"), attempts);
    let events_path = state_dir.join("events.jsonl");
    let events = read_events(&events_path);
    assert_eq!(
        count_events_matching(&events, |event| {
            event_type_matches(event, "ExecutionStarted")
        }),
        attempts,
        "immediate predecessor must retain every execution segment"
    );
    fs::read(events_path).expect("predecessor event bytes")
}

fn continue_rehearsal(root: &Path, finish: bool) {
    let attempts = if finish { 3 } else { 2 };
    let state_dir = root.join(".shipper");
    let previous_events = assert_predecessor(&state_dir, attempts - 1);
    create_three_crate_workspace(root);
    let bin = root.join("fake-bin");
    fs::create_dir_all(&bin).expect("fixture binary directory");
    let fake_cargo = write_fake_cargo(&bin);
    let log = fake_cargo_log(&state_dir);
    let (registry_url, stop, registry) = spawn_registry_at(&live_registry_addr());
    if !finish {
        registry.pin_404("crate-c");
    }
    write_live_rehearsal_config(root, &registry_url);

    let mut command = loopback_shipper_cmd();
    common_args_with_max_attempts(
        &mut command,
        &root.join("Cargo.toml"),
        &registry_url,
        &state_dir,
        &fake_cargo,
        if finish { "3" } else { "2" },
    );
    command
        .arg("resume")
        .env("SHIPPER_FAKE_CARGO_LOG", &log)
        .env("SHIPPER_FAKE_EXIT_FOR_A", "0")
        .env("SHIPPER_FAKE_EXIT_FOR_B", "0")
        .env("SHIPPER_FAKE_EXIT_FOR_C", if finish { "0" } else { "1" })
        .timeout(Duration::from_secs(30));
    if finish {
        command.assert().success();
        assert_live_rehearsal_resumed_state(&state_dir);
    } else {
        command.assert().code(1);
        assert_live_rehearsal_interrupted_state(&state_dir);
        assert!(!state_dir.join("receipt.json").exists());
    }
    let _ = stop.send(());

    let event_bytes = fs::read(state_dir.join("events.jsonl")).expect("continued events");
    assert!(event_bytes.starts_with(&previous_events));
    assert!(event_bytes.len() > previous_events.len());
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(state_dir.join("state.json")).expect("continued state"))
            .expect("continued state JSON");
    assert_eq!(state["packages"]["crate-c@0.1.0"]["attempts"], attempts);
    for name in ["crate-a", "crate-b"] {
        assert_eq!(count_fake_cargo_publishes(&log, name), 1);
    }
    assert_eq!(count_fake_cargo_publishes(&log, "crate-c"), attempts);
    assert_eq!(
        count_events_matching(&read_events(&state_dir.join("events.jsonl")), |event| {
            event_type_matches(event, "ExecutionStarted")
        }),
        attempts
    );
}

#[test]
#[ignore = "workflow-driven: one seed, retry or finish phase per fresh process"]
fn hosted_recovery_phase() {
    let phase = env::var(PHASE_ENV).expect("explicit cross-run rehearsal phase");
    assert!(
        matches!(phase.as_str(), "seed" | "retry" | "finish"),
        "unknown cross-run rehearsal phase"
    );
    if phase == "seed" {
        // Reuse the existing seed and its assertions, not another publisher.
        live_runner_interruption_seed_uploads_shipper_artifact();
    } else {
        continue_rehearsal(&live_rehearsal_root(), phase == "finish");
    }
}

fn run_phase(root: &Path, address: &str, phase: &str) -> Output {
    Command::new(env::current_exe().expect("current integration test executable"))
        .args(["--ignored", "--exact", PHASE_TEST, "--nocapture"])
        .env("SHIPPER_LIVE_REHEARSAL_ROOT", root)
        .env("SHIPPER_LIVE_REHEARSAL_REGISTRY_ADDR", address)
        .env(PHASE_ENV, phase)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .timeout(Duration::from_mins(1))
        .output()
        .expect("bounded fresh phase process")
}

fn checkpoint_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root).expect("checkpoint directory") {
        let path = entry.expect("checkpoint entry").path();
        if path.is_dir() {
            files.extend(checkpoint_bytes(&path));
        } else {
            files.insert(path.clone(), fs::read(path).expect("checkpoint file"));
        }
    }
    files
}

#[test]
#[serial]
fn fresh_process_phases_require_the_immediate_incomplete_predecessor() {
    let fixture = tempdir().expect("phase fixture");
    let root = fixture.path().join("workspace");
    let listener = TcpListener::bind("127.0.0.1:0").expect("unused loopback port");
    let address = listener.local_addr().expect("loopback address").to_string();
    drop(listener);

    let seed = run_phase(&root, &address, "seed");
    assert!(seed.status.success(), "seed output: {seed:?}");
    let state_dir = root.join(".shipper");
    // Refusal must preserve the *whole* prepared workspace, not only
    // .shipper/ evidence: config, manifests and fake Cargo are also immutable.
    let seed_bytes = checkpoint_bytes(&root);

    // A stale seed cannot stand in for the second run's checkpoint.
    let stale = run_phase(&root, &address, "finish");
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("immediate predecessor"));
    assert_eq!(checkpoint_bytes(&root), seed_bytes);
    let unknown = run_phase(&root, &address, "unknown");
    assert!(!unknown.status.success());
    assert_eq!(checkpoint_bytes(&root), seed_bytes);

    let retry = run_phase(&root, &address, "retry");
    assert!(retry.status.success(), "retry output: {retry:?}");
    assert_predecessor(&state_dir, 2);
    let finish = run_phase(&root, &address, "finish");
    assert!(finish.status.success(), "finish output: {finish:?}");
    assert_live_rehearsal_resumed_state(&state_dir);
}
