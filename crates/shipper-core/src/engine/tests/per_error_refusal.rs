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
#[serial]
fn per_error_public_entrypoints_refuse_before_state_network_cargo_or_reporter_effects() {
    let registry = RegistrySpy::start();
    for class in ["retryable", "ambiguous", "permanent"] {
        for retained in [false, true] {
            for parallel in [false, true] {
                let directory = tempdir().expect("temporary directory");
                let bin = directory.path().join("bin");
                write_fake_tools(&bin);
                let cargo_log = directory.path().join("cargo.log");
                let state_dir = directory.path().join(".shipper");
                if retained {
                    fs::create_dir(&state_dir).expect("retained directory");
                    // Refusal precedes even interpretation of retained bytes.
                    for name in [
                        "state.json",
                        "events.jsonl",
                        "receipt.json",
                        "reconciliation.json",
                        "lock",
                    ] {
                        fs::write(state_dir.join(name), format!("retained {name}\n"))
                            .expect("retained evidence");
                    }
                }
                let before = state_bytes(&state_dir);
                let ws = planned_workspace(directory.path(), registry.base_url.clone());
                let mut opts = default_opts(state_dir.clone());
                opts.parallel.enabled = parallel;
                opts.force = true;
                opts.force_resume = true;
                opts.webhook.url = registry.base_url.clone();
                let class_config = Some(crate::retry::RetryStrategyConfig::default());
                match class {
                    "retryable" => opts.retry_per_error.retryable = class_config,
                    "ambiguous" => opts.retry_per_error.ambiguous = class_config,
                    "permanent" => opts.retry_per_error.permanent = class_config,
                    _ => unreachable!(),
                }
                let client =
                    test_registry_client(ws.plan.registry.clone()).expect("registry client");
                let mut caller_state = ExecutionState {
                    state_version: state::CURRENT_STATE_VERSION.to_owned(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    attempt_history: Vec::new(),
                    packages: BTreeMap::new(),
                };
                let caller_before = serde_json::to_value(&caller_state).expect("caller state");
                with_test_env(
                    &bin,
                    vec![(
                        "SHIPPER_CARGO_ARGS_LOG",
                        Some(cargo_log.display().to_string()),
                    )],
                    || {
                        for entrypoint in [
                            "publish",
                            "publish_with_state",
                            "resume",
                            "resume_with_state",
                            "preflight",
                            "preflight_in_place",
                            "preflight_with_options",
                            "preflight_in_place_with_options",
                            "rehearsal",
                            "direct_parallel",
                        ] {
                            let mut reporter = CollectingReporter::default();
                            let mut mutable_ws = ws.clone();
                            let result = match entrypoint {
                                "publish" => run_publish(&ws, &opts, &mut reporter).map(|_| ()),
                                "publish_with_state" => {
                                    run_publish_with_state(&ws, &opts, &mut reporter).map(|_| ())
                                }
                                "resume" => run_resume(&ws, &opts, &mut reporter).map(|_| ()),
                                "resume_with_state" => {
                                    run_resume_with_state(&ws, &opts, &mut reporter).map(|_| ())
                                }
                                "preflight" => run_preflight(&ws, &opts, &mut reporter).map(|_| ()),
                                "preflight_in_place" => {
                                    run_preflight_in_place(&mut mutable_ws, &opts, &mut reporter)
                                        .map(|_| ())
                                }
                                "preflight_with_options" => run_preflight_with_options(
                                    &ws,
                                    &opts,
                                    &mut reporter,
                                    PreflightRunOptions { fresh_audit: true },
                                )
                                .map(|_| ()),
                                "preflight_in_place_with_options" => {
                                    run_preflight_in_place_with_options(
                                        &mut mutable_ws,
                                        &opts,
                                        &mut reporter,
                                        PreflightRunOptions { fresh_audit: true },
                                    )
                                    .map(|_| ())
                                }
                                "rehearsal" => run_rehearsal(&ws, &opts, &mut reporter).map(|_| ()),
                                "direct_parallel" => crate::engine::parallel::run_publish_parallel(
                                    &ws,
                                    &opts,
                                    &mut caller_state,
                                    &state_dir,
                                    &client,
                                    &mut reporter,
                                )
                                .map(|_| ()),
                                _ => unreachable!(),
                            };
                            let error = result.expect_err("unsupported per-error policy");
                            assert!(
                                error
                                    .to_string()
                                    .contains("retry.per_error is not supported"),
                                "{entrypoint}/{class}: {error:#}"
                            );
                            assert_eq!(state_bytes(&state_dir), before, "{entrypoint}/{class}");
                            assert!(!cargo_log.exists(), "{entrypoint} invoked Cargo");
                            assert_eq!(
                                registry.requests.load(Ordering::Acquire),
                                0,
                                "{entrypoint} made a request"
                            );
                            assert!(
                                reporter.infos.is_empty()
                                    && reporter.warns.is_empty()
                                    && reporter.errors.is_empty(),
                                "{entrypoint} reported effects"
                            );
                            assert_eq!(serde_json::to_value(&caller_state).unwrap(), caller_before);
                            assert_eq!(
                                serde_json::to_value(&mutable_ws.plan).unwrap(),
                                serde_json::to_value(&ws.plan).unwrap()
                            );
                        }
                    },
                );
            }
        }
    }
}
