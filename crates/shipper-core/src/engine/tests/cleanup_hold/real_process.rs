use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

use super::super::timeout_reconciliation::ProbeRegistry;
use super::*;

const CHILD: &str = "ops::process::timeout::cleanup_tests::cleanup_process_fixture";

struct ReentryReporter<'a> {
    ws: &'a PlannedWorkspace,
    opts: &'a RuntimeOptions,
    checked: bool,
}

impl Reporter for ReentryReporter<'_> {
    fn info(&mut self, message: &str) {
        if !self.checked && message.starts_with("rehearsal starting") {
            self.checked = true;
            let mut other = CollectingReporter::default();
            let mut publish_opts = self.opts.clone();
            publish_opts.rehearsal_registry = None;
            let error = run_publish(self.ws, &publish_opts, &mut other).unwrap_err();
            assert!(is_publish_admission_error(&error), "{error:#}");
            let error = run_rehearsal(self.ws, self.opts, &mut other).unwrap_err();
            assert!(is_publish_admission_error(&error), "{error:#}");
            assert!(format!("{error:#}").contains("rehearsal ownership"));
        }
    }
    fn warn(&mut self, _: &str) {}
    fn error(&mut self, _: &str) {}
}

struct DescendantCleanup(PathBuf);

impl Drop for DescendantCleanup {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("stop-descendant"), "stop");
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.0.join("descendant-ready").exists()
            && !self.0.join("descendant-finished").exists()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn write_cleanup_cargo(bin: &Path) {
    write_fake_tools(bin);
    let path = fake_cargo_path(bin);
    fs::write(&path, format!(
        "#!/bin/sh\nprintf 'publish\\n' >> cargo-invocations\nif [ -f already-invoked ]; then exit 0; fi\n: > already-invoked\nexec \"$SHIPPER_CLEANUP_FIXTURE_EXE\" --exact {CHILD} --ignored --nocapture --quiet\n"
    )).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture_env(bin: &Path) -> Vec<(&'static str, Option<String>)> {
    let mut env = fake_program_env_vars(bin);
    env.push((
        "SHIPPER_CLEANUP_FIXTURE_EXE",
        Some(std::env::current_exe().unwrap().display().to_string()),
    ));
    env
}

fn cargo_calls(root: &Path) -> usize {
    fs::read_to_string(root.join("cargo-invocations"))
        .unwrap()
        .lines()
        .count()
}

#[test]
#[serial]
fn cleanup_hold_from_real_cargo_precedes_exit_timeout_registry_and_resume() {
    for parallel in [false, true] {
        for mode in ["escape0", "escape", "escape-timeout"] {
            let td = tempdir().unwrap();
            let _descendant = DescendantCleanup(td.path().to_path_buf());
            let bin = td.path().join("bin");
            write_cleanup_cargo(&bin);
            fs::write(td.path().join("mode"), mode).unwrap();
            let registry = ProbeRegistry::start(vec![404]);
            let ws = planned_workspace(td.path(), registry.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = parallel;
            opts.parallel.per_package_timeout = Duration::from_secs(1);
            opts.max_attempts = 3;
            opts.readiness.max_total_wait = Duration::ZERO;
            temp_env::with_vars(fixture_env(&bin), || {
                let mut reporter = CollectingReporter::default();
                let started = Instant::now();
                let error = run_publish(&ws, &opts, &mut reporter).unwrap_err();
                assert_cleanup(&error);
                assert!(
                    started.elapsed() < Duration::from_secs(4),
                    "{mode} {parallel}"
                );
                assert_eq!(cargo_calls(td.path()), 1);
                assert_eq!(
                    *registry.seen.lock().unwrap(),
                    [404],
                    "hold never asks registry to authorize another upload"
                );
                let retained = state::load_state(&state_dir).unwrap().unwrap();
                assert!(matches!(
                    retained.packages[KEY].state,
                    PackageState::CleanupIncomplete { .. }
                ));
                assert_eq!(retained.packages[KEY].attempts, 1);
                assert_eq!(retained.attempt_history.len(), 1);
                assert_eq!(
                    retained.attempt_history[0].error_class,
                    Some(ErrorClass::Ambiguous)
                );
                assert!(retained.attempt_history[0].next_attempt_at.is_none());
                let log =
                    events::EventLog::read_from_file(&events::events_path(&state_dir)).unwrap();
                assert_eq!(
                    log.all_events()
                        .iter()
                        .filter(|e| matches!(
                            e.event_type,
                            EventType::PackageCleanupIncomplete { .. }
                        ))
                        .count(),
                    1
                );
                assert!(!log.all_events().iter().any(|e| matches!(
                    e.event_type,
                    EventType::PublishReconciling { .. }
                        | EventType::PublishReconciled { .. }
                        | EventType::RetryScheduled { .. }
                        | EventType::PackageUploaded
                        | EventType::PackagePublished { .. }
                )));
                assert!(!state::receipt_path(&state_dir).exists());
                let state_bytes = fs::read(state::state_path(&state_dir)).unwrap();
                let event_bytes = fs::read(events::events_path(&state_dir)).unwrap();
                opts.force = true;
                opts.force_resume = true;
                opts.max_attempts = 999;
                assert_cleanup(&run_resume(&ws, &opts, &mut reporter).unwrap_err());
                assert_cleanup(&run_publish(&ws, &opts, &mut reporter).unwrap_err());
                assert_eq!(
                    fs::read(state::state_path(&state_dir)).unwrap(),
                    state_bytes
                );
                assert_eq!(
                    fs::read(events::events_path(&state_dir)).unwrap(),
                    event_bytes
                );
                let rebuilt =
                    rebuild_state_file_from_events(&state_dir, rebuild_options(&ws)).unwrap();
                assert_eq!(rebuilt.packages[KEY].state, retained.packages[KEY].state);
                assert_eq!(rebuilt.attempt_history, retained.attempt_history);
                assert_cleanup(&run_resume(&ws, &opts, &mut reporter).unwrap_err());
                assert_eq!(cargo_calls(td.path()), 1);
                assert_eq!(*registry.seen.lock().unwrap(), [404]);
            });
        }
    }
}

#[test]
#[serial]
fn cleanup_hold_stops_later_parallel_chunks() {
    let td = tempdir().unwrap();
    let _descendant = DescendantCleanup(td.path().to_path_buf());
    let bin = td.path().join("bin");
    write_cleanup_cargo(&bin);
    fs::write(td.path().join("mode"), "escape0").unwrap();
    let registry = ProbeRegistry::start(vec![404]);
    let mut ws = planned_workspace(td.path(), registry.base_url.clone());
    let mut later = ws.plan.packages[0].clone();
    later.name = "later".into();
    ws.plan.packages.push(later);
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = true;
    opts.parallel.max_concurrent = 1;
    temp_env::with_vars(fixture_env(&bin), || {
        assert_cleanup(&run_publish(&ws, &opts, &mut CollectingReporter::default()).unwrap_err());
        let state = state::load_state(&state_dir).unwrap().unwrap();
        assert_eq!(state.packages["later@0.1.0"].attempts, 0);
        assert!(matches!(
            state.packages["later@0.1.0"].state,
            PackageState::Pending
        ));
        assert_eq!(cargo_calls(td.path()), 1);
        assert_eq!(*registry.seen.lock().unwrap(), [404]);
    });
}

#[test]
#[serial]
fn cleanup_hold_blocks_retry_of_an_already_admitted_parallel_peer() {
    let td = tempdir().unwrap();
    let _descendant = DescendantCleanup(td.path().to_path_buf());
    let bin = td.path().join("bin");
    write_cleanup_cargo(&bin);
    // Both commands cross commit_attempt before either can return. The peer
    // then reports a retryable failure only after the other worker saved its
    // cleanup hold. A missing global gate would launch peer attempt two.
    let script = format!(
        r#"#!/bin/sh
case "$3" in
  demo)
    printf 'demo\n' >> cargo-invocations
    while [ ! -f peer-started ]; do sleep 0.02; done
    exec "$SHIPPER_CLEANUP_FIXTURE_EXE" --exact {CHILD} --ignored --nocapture --quiet
    ;;
  peer)
    printf 'peer\n' >> cargo-invocations
    : > peer-started
    while ! grep -q '"state": "cleanup_incomplete"' .shipper/state.json; do sleep 0.02; done
    printf 'connection reset\n' >&2
    exit 42
    ;;
  *) exit 91 ;;
esac
"#
    );
    fs::write(fake_cargo_path(&bin), script).unwrap();
    fs::write(td.path().join("mode"), "escape0").unwrap();
    let registry = ProbeRegistry::start_for_packages(vec![404], vec!["demo", "peer"]);
    let mut ws = planned_workspace(td.path(), registry.base_url.clone());
    let mut peer = ws.plan.packages[0].clone();
    peer.name = "peer".into();
    ws.plan.packages.push(peer);
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.parallel.enabled = true;
    opts.parallel.max_concurrent = 2;
    opts.parallel.per_package_timeout = Duration::from_secs(5);
    opts.max_attempts = 3;
    temp_env::with_vars(fixture_env(&bin), || {
        assert_cleanup(&run_publish(&ws, &opts, &mut CollectingReporter::default()).unwrap_err());
        let state = state::load_state(&state_dir).unwrap().unwrap();
        assert_eq!(state.packages[KEY].attempts, 1);
        assert_eq!(state.packages["peer@0.1.0"].attempts, 1);
        assert!(matches!(
            state.packages[KEY].state,
            PackageState::CleanupIncomplete { .. }
        ));
        assert_eq!(
            cargo_calls(td.path()),
            2,
            "exactly the two already-admitted commands"
        );
        // The admitted peer retains its ordinary post-failure visibility
        // check. That third absence still cannot clear the other worker's
        // process hold or admit a second peer attempt.
        assert_eq!(*registry.seen.lock().unwrap(), [404, 404, 404]);
        let peer = state
            .attempt_history
            .iter()
            .find(|attempt| attempt.package == "peer")
            .unwrap();
        assert_eq!(peer.error_class, Some(ErrorClass::Retryable));
        assert!(!state::receipt_path(&state_dir).exists());
        let log = events::EventLog::read_from_file(&events::events_path(&state_dir)).unwrap();
        assert_eq!(
            log.all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::PackageAttempted { .. }))
                .count(),
            2
        );
    });
}

#[test]
#[serial]
fn cleanup_hold_in_rehearsal_precedes_success_and_blocks_repeated_dispatch() {
    let td = tempdir().unwrap();
    let _descendant = DescendantCleanup(td.path().to_path_buf());
    let bin = td.path().join("bin");
    write_cleanup_cargo(&bin);
    fs::write(td.path().join("mode"), "escape0").unwrap();
    let live = ProbeRegistry::start(vec![404]);
    let rehearsal = ProbeRegistry::start(vec![200]);
    let ws = planned_workspace(td.path(), live.base_url.clone());
    let state_dir = td.path().join(".shipper");
    let mut opts = default_opts(state_dir.clone());
    opts.rehearsal_registry = Some("rehearsal".into());
    opts.rehearsal_smoke_install = Some("demo".into());
    opts.registries.push(Registry {
        name: "rehearsal".into(),
        api_base: rehearsal.base_url.clone(),
        index_base: Some(rehearsal.base_url.clone()),
    });
    temp_env::with_vars(fixture_env(&bin), || {
        let mut reporter = ReentryReporter {
            ws: &ws,
            opts: &opts,
            checked: false,
        };
        assert_cleanup(&run_rehearsal(&ws, &opts, &mut reporter).unwrap_err());
        assert!(
            reporter.checked,
            "owned rehearsal excludes another invocation"
        );
        let events_before = fs::read(events::events_path(&state_dir)).unwrap();
        assert_cleanup(&run_rehearsal(&ws, &opts, &mut reporter).unwrap_err());
        assert_cleanup(&run_publish(&ws, &opts, &mut reporter).unwrap_err());
        assert_eq!(cargo_calls(td.path()), 1);
        assert_eq!(
            fs::read(events::events_path(&state_dir)).unwrap(),
            events_before
        );
        assert!(
            !state::state_path(&state_dir).exists(),
            "rehearsal does not invent a publish projection"
        );
        assert!(!crate::state::rehearsal::rehearsal_path(&state_dir).exists());
        assert!(live.seen.lock().unwrap().is_empty());
        assert!(
            rehearsal.seen.lock().unwrap().is_empty(),
            "cleanup precedes visibility and smoke checks"
        );
    });
}
