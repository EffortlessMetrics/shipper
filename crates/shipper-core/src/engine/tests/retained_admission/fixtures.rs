use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Stay available throughout refusal checks so zero requests cannot mean that
/// a finite 404 fixture simply closed before an unauthorized request arrived.
pub(super) struct AdmissionRegistry {
    pub url: String,
    hits: Arc<AtomicUsize>,
    statuses: Arc<Mutex<VecDeque<u16>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AdmissionRegistry {
    pub fn new() -> Self {
        let server = Server::http("127.0.0.1:0").expect("registry listener");
        let url = format!("http://{}", server.server_addr());
        let hits = Arc::new(AtomicUsize::new(0));
        let statuses = Arc::new(Mutex::new(VecDeque::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (worker_hits, worker_statuses, worker_stopped) = (
            Arc::clone(&hits),
            Arc::clone(&statuses),
            Arc::clone(&stopped),
        );
        let worker = thread::spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(10)) {
                    worker_hits.fetch_add(1, Ordering::AcqRel);
                    let status = worker_statuses
                        .lock()
                        .expect("statuses")
                        .pop_front()
                        .unwrap_or(404);
                    let _ = request.respond(
                        Response::from_string("{}")
                            .with_status_code(StatusCode(status))
                            .with_header(
                                Header::from_bytes("Content-Type", "application/json")
                                    .expect("header"),
                            ),
                    );
                }
            }
        });
        Self {
            url,
            hits,
            statuses,
            stopped,
            worker: Some(worker),
        }
    }

    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Acquire)
    }

    pub fn respond_with(&self, statuses: &[u16]) {
        let mut queue = self.statuses.lock().expect("statuses");
        assert!(queue.is_empty(), "previous expected responses remain");
        queue.extend(statuses.iter().copied());
    }
}

impl Drop for AdmissionRegistry {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("registry worker");
        }
    }
}

pub(super) type ArtifactBytes = Vec<(PathBuf, Option<Vec<u8>>)>;

pub(super) struct Fixture {
    pub _directory: tempfile::TempDir,
    pub bin: PathBuf,
    pub cargo_log: PathBuf,
    pub registry: AdmissionRegistry,
    pub ws: PlannedWorkspace,
    pub state_dir: PathBuf,
    pub opts: RuntimeOptions,
}

impl Fixture {
    pub fn new(parallel: bool, sibling: bool) -> Self {
        let directory = tempdir().expect("fixture directory");
        let bin = directory.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = directory.path().join("cargo.log");
        let registry = AdmissionRegistry::new();
        let mut ws = planned_workspace(directory.path(), registry.url.clone());
        if sibling {
            let mut package = ws.plan.packages[0].clone();
            package.name = "later-sibling".into();
            ws.plan.packages.push(package);
        }
        let state_dir = directory.path().join(".shipper");
        let mut opts = default_opts(state_dir.clone());
        opts.parallel.enabled = parallel;
        opts.parallel.max_concurrent = 1;
        opts.max_attempts = 1;
        opts.readiness.enabled = false;
        opts.readiness.max_total_wait = Duration::ZERO;
        Self {
            _directory: directory,
            bin,
            cargo_log,
            registry,
            ws,
            state_dir,
            opts,
        }
    }

    pub fn with_cargo<T>(&self, exit: &str, action: impl FnOnce() -> T) -> T {
        with_test_env(
            &self.bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some(exit.into())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                ("SHIPPER_CARGO_STDERR", Some(String::new())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(self.cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            action,
        )
    }

    pub fn cargo_calls(&self) -> usize {
        fs::read_to_string(&self.cargo_log)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
    }

    pub fn retained(&self) -> Result<ExecutionState> {
        state::load_state(&self.state_dir)?.context("retained fixture state")
    }

    pub fn seed_stop(&self) -> Result<ExecutionState> {
        self.with_cargo("1", || -> Result<()> {
            let error = run_publish(&self.ws, &self.opts, &mut CollectingReporter::default())
                .err()
                .context("fixture must genuinely stop")?;
            ensure!(
                matches!(
                    classify_publish_stop(&error),
                    Some(PublishStopClassification::RecoverableNotPublished {
                        evidence_consistent: true
                    })
                ),
                "{error:#}"
            );
            Ok(())
        })?;
        ensure!(self.cargo_calls() == 1 && self.registry.hits() == 2);
        let retained = self.retained()?;
        ensure!(retained.packages["demo@0.1.0"].attempts == 1);
        Ok(retained)
    }

    pub fn lock_path(&self) -> PathBuf {
        lock::lock_path(&self.state_dir, Some(&self.ws.workspace_root))
    }

    pub fn snapshot(&self) -> Result<ArtifactBytes> {
        [
            state::state_path(&self.state_dir),
            events::events_path(&self.state_dir),
            state::receipt_path(&self.state_dir),
            state::reconciliation_path(&self.state_dir),
            self.lock_path(),
        ]
        .into_iter()
        .map(|path| {
            let bytes = if path.exists() {
                Some(fs::read(&path)?)
            } else {
                None
            };
            Ok((path, bytes))
        })
        .collect()
    }

    pub fn restore(&self, retained: &ArtifactBytes) -> Result<()> {
        for (path, bytes) in retained {
            if let Some(bytes) = bytes {
                fs::write(path, bytes)?;
            } else if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn released_owner_record(&self) -> Result<serde_json::Value> {
        let owner = lock::LockFile::acquire(&self.state_dir, Some(&self.ws.workspace_root))?;
        owner.set_plan_id(&self.ws.plan.plan_id)?;
        let record = serde_json::from_slice(&fs::read(self.lock_path())?)?;
        owner.release()?;
        Ok(record)
    }
}
