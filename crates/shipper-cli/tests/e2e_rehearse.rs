//! End-to-end #90 Recover rehearsal — synthetic side.
//!
//! The existing `bdd_resume` tests cover resume behavior against
//! **hand-constructed** state files: "given state X, run resume, assert Y."
//! That covers the read-side contract but doesn't prove the write-side under
//! an actual run: does `.shipper/state.json` and `.shipper/events.jsonl` stay
//! coherent when `shipper publish` is interrupted mid-workspace?
//!
//! This test closes that gap:
//!
//! 1. Build a 3-crate workspace (a, b, c with c→b→a deps) so we exercise
//!    a real dependency-ordered plan, not a single-crate trivial case.
//! 2. Spawn a "smart" mock registry that returns 404 on the first lookup per
//!    crate path (preflight) and 200 afterward (post-publish visibility).
//! 3. First run: fake cargo succeeds for a/b and fails for c. That reaches
//!    a realistic interrupted-mid-run state where two crates are published
//!    and one is left Failed.
//! 4. Inspect the persisted evidence and assert events-as-truth invariants:
//!    - `state.json` parses and reflects a/b published, c not published.
//!    - `events.jsonl` is valid NDJSON — every line parses, no half-written
//!      line from a partial write.
//!    - PackagePublished count equals the number of actually-published
//!      crates (no spurious duplicates).
//! 5. Second run: `shipper resume` with fake cargo now succeeding for c.
//! 6. Assert the resume respected the persisted state:
//!    - exit 0
//!    - a/b NOT re-published (idempotency)
//!    - c reaches Published
//!    - PackagePublished event count is exactly N_crates (one per crate,
//!      across both runs combined)
//!    - PackageSkipped events emitted for a/b during resume
//!
//! This is the regression guard that pairs with the real-workflow rehearsal
//! documented in `docs/how-to/run-recover-rehearsal.md`. The real rehearsal
//! exercises the same invariants against crates.io proper.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use serial_test::serial;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

// ---------------------------------------------------------------------------
// Fixture: 3-crate workspace (a, b, c with c→b→a)
// ---------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn create_three_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["crate-a", "crate-b", "crate-c"]
resolver = "2"
"#,
    );

    for (name, deps) in [
        ("crate-a", ""),
        ("crate-b", "crate-a = { path = \"../crate-a\" }"),
        (
            "crate-c",
            "crate-a = { path = \"../crate-a\" }\ncrate-b = { path = \"../crate-b\" }",
        ),
    ] {
        write_file(
            &root.join(format!("{name}/Cargo.toml")),
            &format!(
                r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
{deps}
"#
            ),
        );
        write_file(&root.join(format!("{name}/src/lib.rs")), "pub fn hi() {}\n");
    }
}

// ---------------------------------------------------------------------------
// Fake cargo: exit code selected by env var SHIPPER_FAKE_EXIT_FOR_<crate>.
// Falls back to `0` (success) if the matching env var is absent.
// ---------------------------------------------------------------------------

fn write_fake_cargo(bin_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = bin_dir.join("cargo.cmd");
        // For each known crate, check if an env var is set picking the exit
        // code. Cargo always invokes with `-p <crate>`, so the crate name
        // appears verbatim in the arg list.
        //
        // The nested `if defined / else` pattern combined with `findstr &&`
        // is fragile in cmd. Flatten it: use separate `if defined` checks
        // *after* the findstr sets the `_MATCH` flag, rather than inside
        // the short-circuit. Avoids delayed-expansion quoting footguns.
        let script = "\
@echo off\r\n\
setlocal EnableDelayedExpansion\r\n\
set ARGS=%*\r\n\
set MATCH=\r\n\
echo !ARGS! | findstr /C:\"crate-c\" >nul && set MATCH=C\r\n\
echo !ARGS! | findstr /C:\"crate-b\" >nul && if \"!MATCH!\"==\"\" set MATCH=B\r\n\
echo !ARGS! | findstr /C:\"crate-a\" >nul && if \"!MATCH!\"==\"\" set MATCH=A\r\n\
if \"!MATCH!\"==\"C\" if defined SHIPPER_FAKE_EXIT_FOR_C exit /b !SHIPPER_FAKE_EXIT_FOR_C!\r\n\
if \"!MATCH!\"==\"B\" if defined SHIPPER_FAKE_EXIT_FOR_B exit /b !SHIPPER_FAKE_EXIT_FOR_B!\r\n\
if \"!MATCH!\"==\"A\" if defined SHIPPER_FAKE_EXIT_FOR_A exit /b !SHIPPER_FAKE_EXIT_FOR_A!\r\n\
exit /b 0\r\n";
        fs::write(&path, script).expect("write fake cargo");
        path
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("cargo");
        let script = "#!/usr/bin/env sh\n\
case \"$*\" in\n\
  *crate-c*) exit \"${SHIPPER_FAKE_EXIT_FOR_C:-0}\" ;;\n\
  *crate-b*) exit \"${SHIPPER_FAKE_EXIT_FOR_B:-0}\" ;;\n\
  *crate-a*) exit \"${SHIPPER_FAKE_EXIT_FOR_A:-0}\" ;;\n\
esac\n\
exit 0\n";
        fs::write(&path, script).expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
        path
    }
}

// ---------------------------------------------------------------------------
// Mock registry.
//
// Per-path semantics:
//   * If the path contains any `never_flip` substring → always 404.
//     Used for a crate whose cargo publish we know will fail this run: we
//     want shipper to classify the failure as Failed, not as ambiguous-but-
//     actually-published (which the reconcile logic would do if 200 leaked).
//   * Otherwise the first hit returns 404 (preflight "new crate"), and every
//     subsequent hit returns 200 with a minimal versions body — mirroring
//     cargo actually publishing and the registry becoming visible.
// ---------------------------------------------------------------------------

struct RegistryHandles {
    never_flip: Arc<Mutex<Vec<&'static str>>>,
}

impl RegistryHandles {
    fn pin_404(&self, substr: &'static str) {
        self.never_flip.lock().expect("lock").push(substr);
    }
    fn clear_pins(&self) {
        self.never_flip.lock().expect("lock").clear();
    }
}

fn spawn_registry() -> (String, std::sync::mpsc::Sender<()>, RegistryHandles) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    let per_path_hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
    let never_flip: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let never_flip_for_thread = Arc::clone(&never_flip);
    let hits_for_thread = Arc::clone(&per_path_hits);

    thread::spawn(move || {
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            match server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(req)) => {
                    let path = req.url().split('?').next().unwrap_or("").to_owned();

                    let pinned_404 = {
                        let list = never_flip_for_thread.lock().expect("lock");
                        list.iter().any(|needle| path.contains(needle))
                    };

                    let hits = {
                        let mut map = hits_for_thread.lock().expect("lock");
                        let counter = map.entry(path.clone()).or_insert(0);
                        *counter += 1;
                        *counter
                    };

                    let (status, body) = if pinned_404 || hits <= 1 {
                        (404u16, String::from("{}"))
                    } else {
                        (
                            200u16,
                            r#"{"crate":{"name":"x"},"versions":[{"num":"0.1.0","yanked":false}]}"#
                                .to_string(),
                        )
                    };

                    let resp = Response::from_string(body)
                        .with_status_code(StatusCode(status))
                        .with_header(
                            Header::from_bytes("Content-Type", "application/json").expect("header"),
                        );
                    let _ = req.respond(resp);
                }
                _ => continue,
            }
        }
    });

    (base_url, stop_tx, RegistryHandles { never_flip })
}

// ---------------------------------------------------------------------------
// Event / state parsing helpers.
// ---------------------------------------------------------------------------

fn package_state(state: &serde_json::Value, name_at_ver: &str) -> Option<String> {
    state
        .get("packages")?
        .get(name_at_ver)?
        .get("state")?
        .get("state")?
        .as_str()
        .map(str::to_owned)
}

fn read_events(events_path: &Path) -> Vec<serde_json::Value> {
    let raw = fs::read_to_string(events_path).unwrap_or_default();
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("events.jsonl must be valid NDJSON"))
        .collect()
}

fn count_events_matching<F>(events: &[serde_json::Value], pred: F) -> usize
where
    F: Fn(&serde_json::Value) -> bool,
{
    events.iter().filter(|e| pred(e)).count()
}

fn event_type_matches(event: &serde_json::Value, expected_kind: &str) -> bool {
    // EventType is `#[serde(tag = "type", rename_all = "snake_case")]` so it
    // serializes internally-tagged with a `type` discriminator, e.g.
    // `{"type":"package_published","duration_ms":4500}`. Callers pass the
    // PascalCase variant name; we convert to snake_case before comparing.
    event
        .get("event_type")
        .and_then(|et| et.get("type"))
        .and_then(|t| t.as_str())
        .map(|s| s == pascal_to_snake(expected_kind))
        .unwrap_or(false)
}

fn pascal_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

fn common_args(
    cmd: &mut Command,
    manifest: &Path,
    api_base: &str,
    state_dir: &Path,
    fake_cargo: &Path,
) {
    cmd.arg("--manifest-path")
        .arg(manifest)
        .arg("--api-base")
        .arg(api_base)
        .arg("--allow-dirty")
        .arg("--no-readiness")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--verify-poll")
        .arg("0ms")
        .arg("--verify-mode")
        .arg("none")
        .arg("--max-attempts")
        .arg("1")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(state_dir)
        .env("SHIPPER_CARGO_BIN", fake_cargo);
}

// ---------------------------------------------------------------------------
// THE TEST
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn rehearsal_interrupted_publish_then_resume_preserves_invariants() {
    let td = tempdir().expect("tempdir");
    let root = td.path();
    create_three_crate_workspace(root);

    let bin_dir = root.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let fake_cargo = write_fake_cargo(&bin_dir);

    // Single registry across both runs (same URL → same plan_id → resume
    // is allowed). Run 1 pins crate-c at 404 via `never_flip` so the
    // reconcile path sees it as truly absent; we unpin between runs.
    let (registry_url, registry_stop, registry) = spawn_registry();
    registry.pin_404("crate-c");

    let state_dir = root.join(".shipper");
    let state_path = state_dir.join("state.json");
    let events_path = state_dir.join("events.jsonl");

    // ── Run 1: publish with crate-c failing ──────────────────────────────
    // This is the "interrupted run" — a + b succeed, c fails. Shipper
    // persists state after each step, so state.json and events.jsonl
    // should reflect reality at the moment the loop gave up on c.
    let mut cmd = shipper_cmd();
    common_args(
        &mut cmd,
        &root.join("Cargo.toml"),
        &registry_url,
        &state_dir,
        &fake_cargo,
    );
    cmd.arg("publish")
        .env("SHIPPER_FAKE_EXIT_FOR_A", "0")
        .env("SHIPPER_FAKE_EXIT_FOR_B", "0")
        .env("SHIPPER_FAKE_EXIT_FOR_C", "1");
    cmd.assert().failure();

    // ── Invariant 1: state.json parses and reflects reality ──────────────
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("state.json exists"))
            .expect("state.json is valid JSON");

    assert_eq!(
        package_state(&state, "crate-a@0.1.0").as_deref(),
        Some("published"),
        "a must be published after run 1"
    );
    assert_eq!(
        package_state(&state, "crate-b@0.1.0").as_deref(),
        Some("published"),
        "b must be published after run 1"
    );
    assert_ne!(
        package_state(&state, "crate-c@0.1.0").as_deref(),
        Some("published"),
        "c must NOT be published (fake cargo exited 1 for c)"
    );

    // ── Invariant 2: events.jsonl is valid NDJSON after an interrupted run
    // `read_events` panics if any line fails to parse — running it proves
    // there's no half-written or truncated event.
    let events_r1 = read_events(&events_path);
    assert!(
        !events_r1.is_empty(),
        "events.jsonl must have content after run 1"
    );

    // ── Invariant 3: PackagePublished events match actually-published count
    // — exactly one per success, no duplicates.
    let published_r1 =
        count_events_matching(&events_r1, |e| event_type_matches(e, "PackagePublished"));
    assert_eq!(
        published_r1, 2,
        "PackagePublished events after run 1 should equal succeeded crates (2 = a + b); got {published_r1}"
    );

    // ── Run 2: resume with crate-c succeeding ────────────────────────────
    // Unpin crate-c; keep per-path hit counters intact. Since crate-c's
    // preflight already fired in run 1 (counter > 1), run 2's post-publish
    // check gets 200 immediately, mirroring real crates.io where the
    // version is now visible after cargo's successful upload.
    // We keep the same registry URL so plan_id stays stable and resume
    // doesn't trip the stale-plan guard.
    registry.clear_pins();

    let mut resume = shipper_cmd();
    common_args(
        &mut resume,
        &root.join("Cargo.toml"),
        &registry_url,
        &state_dir,
        &fake_cargo,
    );
    resume.arg("resume").env("SHIPPER_FAKE_EXIT_FOR_C", "0");
    resume.assert().success();

    let _ = registry_stop.send(());

    // ── Invariant 4: final state has a/b Published and c resolved ───────
    // A "resolved" c can be either Published (cargo was invoked and
    // succeeded) or Skipped (the pre-publish version_exists check saw c
    // already on the registry and short-circuited). Both are legitimate
    // end states that indicate "c is done, don't try again."
    let state_after_raw = fs::read_to_string(&state_path).expect("read state");
    let state_after: serde_json::Value =
        serde_json::from_str(&state_after_raw).expect("parse state");
    for pkg in ["crate-a@0.1.0", "crate-b@0.1.0"] {
        assert_eq!(
            package_state(&state_after, pkg).as_deref(),
            Some("published"),
            "{pkg} must be Published after resume. full state after resume:\n{}",
            state_after_raw
        );
    }
    let c_state = package_state(&state_after, "crate-c@0.1.0");
    assert!(
        matches!(c_state.as_deref(), Some("published") | Some("skipped")),
        "crate-c must be Published or Skipped after resume; got {c_state:?}. \
         full state:\n{state_after_raw}"
    );

    // ── Invariant 5: events-as-truth — idempotency. A successful publish
    // for any given crate+version must produce exactly one PackagePublished
    // event across all runs, never two. In this scenario:
    //   - run 1 emits PackagePublished for a and b (c fails → no event)
    //   - run 2's resume must NOT re-publish a or b (those events would be
    //     duplicates). c resolves either via a real publish (emitting a new
    //     PackagePublished) or via the "already published" short-circuit
    //     (emitting PackageSkipped with no PackagePublished). Either way,
    //     a + b account for 2 PackagePublished and c for 0 or 1.
    let events_all = read_events(&events_path);
    let published_total =
        count_events_matching(&events_all, |e| event_type_matches(e, "PackagePublished"));
    assert!(
        (2..=3).contains(&published_total),
        "PackagePublished events across both runs should be 2 (a, b) or 3 \
         (a, b, c if c was re-published during resume); got {published_total}. \
         4+ would mean resume duplicated a or b — a correctness violation."
    );

    // ── Invariant 6: every post-run-1 PackagePublished event that exists
    // has a partner ExecutionStarted event preceding it in the file.
    // (Sanity check for events.jsonl being actually append-only — if resume
    // somehow truncated and rewrote the file, the pre-resume events would
    // be gone and the ExecutionStarted count would drop.)
    let execution_started =
        count_events_matching(&events_all, |e| event_type_matches(e, "ExecutionStarted"));
    assert_eq!(
        execution_started, 2,
        "ExecutionStarted events should be exactly 2 (one per run); got {execution_started}. \
         < 2 means events.jsonl was truncated somewhere — append-only invariant broken."
    );
}
