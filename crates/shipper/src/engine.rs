use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use rand::Rng;

use crate::auth;
use crate::cargo;
use crate::git;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::state;
use crate::types::{
    ErrorClass, ExecutionState, PackageProgress, PackageReceipt, PackageState, Receipt,
    RuntimeOptions,
};

pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

#[derive(Debug, Clone)]
pub struct PreflightPackage {
    pub name: String,
    pub version: String,
    pub already_published: bool,
}

#[derive(Debug, Clone)]
pub struct PreflightReport {
    pub plan_id: String,
    pub token_detected: bool,
    pub packages: Vec<PreflightPackage>,
}

pub fn run_preflight(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PreflightReport> {
    let workspace_root = &ws.workspace_root;

    if !opts.allow_dirty {
        reporter.info("checking git cleanliness...");
        git::ensure_git_clean(workspace_root)?;
    }

    reporter.info("initializing registry client...");
    let reg = RegistryClient::new(ws.plan.registry.clone())?;

    let token = auth::resolve_token(&ws.plan.registry.name)?;
    let token_detected = token.as_ref().map(|s| !s.is_empty()).unwrap_or(false);

    if opts.strict_ownership && !token_detected {
        bail!(
            "strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)"
        );
    }

    // Best-effort ownership preflight: this *may* require token scopes beyond publish.
    if token_detected && !opts.skip_ownership_check {
        reporter.info("best-effort owners preflight...");
        let token = token.as_deref().unwrap();
        for p in &ws.plan.packages {
            let owners = reg.list_owners(&p.name, token);
            if let Err(e) = owners {
                if opts.strict_ownership {
                    return Err(e);
                }
                reporter.warn(&format!(
                    "owners preflight failed for {}: {} (continuing)",
                    p.name, e
                ));
            }
        }
    }

    reporter.info("checking which versions are already published...");
    let mut packages: Vec<PreflightPackage> = Vec::new();
    for p in &ws.plan.packages {
        let already_published = reg.version_exists(&p.name, &p.version)?;
        packages.push(PreflightPackage {
            name: p.name.clone(),
            version: p.version.clone(),
            already_published,
        });
    }

    Ok(PreflightReport {
        plan_id: ws.plan.plan_id.clone(),
        token_detected,
        packages,
    })
}

pub fn run_publish(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);

    if !opts.allow_dirty {
        git::ensure_git_clean(workspace_root)?;
    }

    let reg = RegistryClient::new(ws.plan.registry.clone())?;

    // Load existing state (if any), or initialize.
    let mut st = match state::load_state(&state_dir)? {
        Some(existing) => {
            if existing.plan_id != ws.plan.plan_id {
                if !opts.force_resume {
                    bail!(
                        "existing state plan_id {} does not match current plan_id {}; delete state or use --force-resume",
                        existing.plan_id,
                        ws.plan.plan_id
                    );
                }
                reporter.warn("forcing resume with mismatched plan_id (unsafe)");
            }
            existing
        }
        None => init_state(ws, &state_dir)?,
    };

    reporter.info(&format!("state dir: {}", state_dir.as_path().display()));

    let mut receipts: Vec<PackageReceipt> = Vec::new();
    let run_started = Utc::now();

    // Ensure we have entries for all packages in plan.
    for p in &ws.plan.packages {
        let key = pkg_key(&p.name, &p.version);
        st.packages.entry(key).or_insert_with(|| PackageProgress {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        });
    }
    st.updated_at = Utc::now();
    state::save_state(&state_dir, &st)?;

    for p in &ws.plan.packages {
        let key = pkg_key(&p.name, &p.version);
        let progress = st
            .packages
            .get(&key)
            .context("missing package progress in state")?
            .clone();

        match progress.state {
            PackageState::Published | PackageState::Skipped { .. } => {
                reporter.info(&format!(
                    "{}@{}: already complete ({})",
                    p.name,
                    p.version,
                    short_state(&progress.state)
                ));
                continue;
            }
            _ => {}
        }

        let started_at = Utc::now();
        let start_instant = Instant::now();

        // First, check if the version is already present.
        if reg.version_exists(&p.name, &p.version)? {
            reporter.info(&format!(
                "{}@{}: already published (skipping)",
                p.name, p.version
            ));
            let skipped = PackageState::Skipped {
                reason: "already published".into(),
            };
            update_state(&mut st, &state_dir, &key, skipped)?;
            receipts.push(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: st.packages.get(&key).unwrap().attempts,
                state: st.packages.get(&key).unwrap().state.clone(),
                started_at,
                finished_at: Utc::now(),
                duration_ms: start_instant.elapsed().as_millis(),
            });
            continue;
        }

        reporter.info(&format!("{}@{}: publishing...", p.name, p.version));

        let mut attempt = st.packages.get(&key).unwrap().attempts;
        let mut last_err: Option<(ErrorClass, String)> = None;

        while attempt < opts.max_attempts {
            attempt += 1;
            {
                let pr = st.packages.get_mut(&key).unwrap();
                pr.attempts = attempt;
                pr.last_updated_at = Utc::now();
                state::save_state(&state_dir, &st)?;
            }

            reporter.info(&format!(
                "{}@{}: attempt {}/{}",
                p.name, p.version, attempt, opts.max_attempts
            ));

            let out = cargo::cargo_publish(
                workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
            )?;

            let success = out.status_code == Some(0);

            if success {
                reporter.info(&format!(
                    "{}@{}: cargo publish exited successfully; verifying...",
                    p.name, p.version
                ));
                let visible = verify_published(
                    &reg,
                    &p.name,
                    &p.version,
                    opts.verify_timeout,
                    opts.verify_poll_interval,
                    reporter,
                )?;
                if visible {
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                    last_err = None;
                    break;
                } else {
                    // Cargo itself may warn if the index isn't updated yet. Shipper extends the wait,
                    // but if it still doesn't show up we treat this as ambiguous.
                    last_err = Some((ErrorClass::Ambiguous, "publish succeeded locally, but version not observed on registry within timeout".into()));
                }
            } else {
                // Even if cargo fails, the publish may have succeeded (timeouts, network splits).
                // Always check the registry before deciding.
                reporter.warn(&format!(
                    "{}@{}: cargo publish failed (exit={:?}); checking registry...",
                    p.name, p.version, out.status_code
                ));

                if reg.version_exists(&p.name, &p.version)? {
                    reporter.info(&format!(
                        "{}@{}: version is present on registry; treating as published",
                        p.name, p.version
                    ));
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                    last_err = None;
                    break;
                }

                let (class, msg) = classify_cargo_failure(&out.stderr, &out.stdout);
                last_err = Some((class.clone(), msg.clone()));

                match class {
                    ErrorClass::Permanent => {
                        let failed = PackageState::Failed {
                            class,
                            message: msg,
                        };
                        update_state(&mut st, &state_dir, &key, failed)?;
                        return Err(anyhow::anyhow!(
                            "{}@{}: permanent failure: {}",
                            p.name,
                            p.version,
                            last_err.unwrap().1
                        ));
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        let delay = backoff_delay(opts.base_delay, opts.max_delay, attempt);
                        reporter.warn(&format!(
                            "{}@{}: retrying in {}",
                            p.name,
                            p.version,
                            humantime::format_duration(delay)
                        ));
                        thread::sleep(delay);
                    }
                }
            }

            // If we got here without breaking, and there are attempts left, loop.
        }

        let finished_at = Utc::now();
        let duration_ms = start_instant.elapsed().as_millis();

        if let Some((class, msg)) = last_err {
            // Final chance: maybe it eventually showed up.
            if reg.version_exists(&p.name, &p.version)? {
                update_state(&mut st, &state_dir, &key, PackageState::Published)?;
            } else {
                let failed = PackageState::Failed {
                    class: class.clone(),
                    message: msg.clone(),
                };
                update_state(&mut st, &state_dir, &key, failed)?;
                receipts.push(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: st.packages.get(&key).unwrap().attempts,
                    state: st.packages.get(&key).unwrap().state.clone(),
                    started_at,
                    finished_at,
                    duration_ms,
                });
                return Err(anyhow::anyhow!("{}@{}: failed: {}", p.name, p.version, msg));
            }
        }

        receipts.push(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: st.packages.get(&key).unwrap().attempts,
            state: st.packages.get(&key).unwrap().state.clone(),
            started_at,
            finished_at,
            duration_ms,
        });
    }

    let receipt = Receipt {
        receipt_version: "shipper.receipt.v1".to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        started_at: run_started,
        finished_at: Utc::now(),
        packages: receipts,
    };

    state::write_receipt(&state_dir, &receipt)?;

    Ok(receipt)
}

pub fn run_resume(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);
    if state::load_state(&state_dir)?.is_none() {
        bail!(
            "no existing state found in {}; run shipper publish first",
            state_dir.display()
        );
    }
    run_publish(ws, opts, reporter)
}

fn init_state(ws: &PlannedWorkspace, state_dir: &Path) -> Result<ExecutionState> {
    let mut packages: BTreeMap<String, PackageProgress> = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }

    let st = ExecutionState {
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    };

    state::save_state(state_dir, &st)?;
    Ok(st)
}

fn update_state(
    st: &mut ExecutionState,
    state_dir: &Path,
    key: &str,
    new_state: PackageState,
) -> Result<()> {
    let pr = st
        .packages
        .get_mut(key)
        .context("missing package in state")?;
    pr.state = new_state;
    pr.last_updated_at = Utc::now();
    st.updated_at = Utc::now();
    state::save_state(state_dir, st)
}

fn resolve_state_dir(workspace_root: &Path, state_dir: &PathBuf) -> PathBuf {
    if state_dir.is_absolute() {
        state_dir.clone()
    } else {
        workspace_root.join(state_dir)
    }
}

fn pkg_key(name: &str, version: &str) -> String {
    format!("{}@{}", name, version)
}

fn short_state(st: &PackageState) -> &'static str {
    match st {
        PackageState::Pending => "pending",
        PackageState::Published => "published",
        PackageState::Skipped { .. } => "skipped",
        PackageState::Failed { .. } => "failed",
        PackageState::Ambiguous { .. } => "ambiguous",
    }
}

fn verify_published(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    timeout: Duration,
    poll: Duration,
    reporter: &mut dyn Reporter,
) -> Result<bool> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if reg.version_exists(crate_name, version)? {
            return Ok(true);
        }
        reporter.info(&format!(
            "{}@{}: not visible yet; waiting {}",
            crate_name,
            version,
            humantime::format_duration(poll)
        ));
        thread::sleep(poll);
    }
    Ok(false)
}

fn classify_cargo_failure(stderr: &str, stdout: &str) -> (ErrorClass, String) {
    let hay = format!("{}\n{}", stderr, stdout).to_lowercase();

    // Retryable: backpressure and transient network failures.
    let retryable_patterns = [
        "too many requests",
        "429",
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "connection closed",
        "dns",
        "tls",
        "temporarily unavailable",
        "failed to download",
        "failed to send",
        "server error",
        "502",
        "503",
        "504",
    ];

    if retryable_patterns.iter().any(|p| hay.contains(p)) {
        return (
            ErrorClass::Retryable,
            "transient failure (retryable)".into(),
        );
    }

    // Permanent: manifest / packaging / validation failures.
    let permanent_patterns = [
        "failed to parse manifest",
        "invalid",
        "missing",
        "license",
        "description",
        "readme",
        "repository",
        "could not compile",
        "compilation failed",
        "failed to verify",
        "package is not allowed to be published",
        "publish is disabled",
        "yanked",
        "forbidden",
        "permission denied",
        "not authorized",
        "unauthorized",
    ];

    if permanent_patterns.iter().any(|p| hay.contains(p)) {
        return (
            ErrorClass::Permanent,
            "permanent failure (fix required)".into(),
        );
    }

    // Ambiguous: default. We'll always verify registry before failing.
    (
        ErrorClass::Ambiguous,
        "publish outcome ambiguous; registry did not show version".into(),
    )
}

fn backoff_delay(base: Duration, max: Duration, attempt: u32) -> Duration {
    let pow = attempt.saturating_sub(1).min(16);
    let mut delay = base.saturating_mul(2_u32.saturating_pow(pow));
    if delay > max {
        delay = max;
    }

    // 0.5x..1.5x jitter
    let jitter: f64 = rand::thread_rng().gen_range(0.5..1.5);
    let millis = (delay.as_millis() as f64 * jitter).round() as u128;
    Duration::from_millis(millis as u64)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use chrono::Utc;
    use insta::assert_debug_snapshot;
    use serial_test::serial;
    use tempfile::tempdir;
    use tiny_http::{Header, Response, Server, StatusCode};

    use super::*;
    use crate::plan::PlannedWorkspace;
    use crate::types::{PlannedPackage, Registry, ReleasePlan};

    struct CollectingReporter {
        infos: Vec<String>,
        warns: Vec<String>,
        errors: Vec<String>,
    }

    impl Default for CollectingReporter {
        fn default() -> Self {
            Self {
                infos: Vec::new(),
                warns: Vec::new(),
                errors: Vec::new(),
            }
        }
    }

    impl Reporter for CollectingReporter {
        fn info(&mut self, msg: &str) {
            self.infos.push(msg.to_string());
        }

        fn warn(&mut self, msg: &str) {
            self.warns.push(msg.to_string());
        }

        fn error(&mut self, msg: &str) {
            self.errors.push(msg.to_string());
        }
    }

    #[derive(Clone)]
    struct EnvGuard {
        key: String,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            Self {
                key: key.to_string(),
                old,
            }
        }

        fn unset(key: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::remove_var(key) };
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                unsafe { env::set_var(&self.key, v) };
            } else {
                unsafe { env::remove_var(&self.key) };
            }
        }
    }

    #[cfg(windows)]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo.cmd")
    }

    #[cfg(not(windows))]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo")
    }

    #[cfg(windows)]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git.cmd")
    }

    #[cfg(not(windows))]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git")
    }

    fn configure_fake_programs(bin_dir: &Path) -> (EnvGuard, EnvGuard) {
        let cargo = EnvGuard::set(
            "SHIPPER_CARGO_BIN",
            fake_cargo_path(bin_dir).to_str().expect("utf8"),
        );
        let git = EnvGuard::set(
            "SHIPPER_GIT_BIN",
            fake_git_path(bin_dir).to_str().expect("utf8"),
        );
        (cargo, git)
    }

    fn write_fake_cargo(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("cargo.cmd"),
                "@echo off\r\nif not \"%SHIPPER_CARGO_ARGS_LOG%\"==\"\" echo %*>>\"%SHIPPER_CARGO_ARGS_LOG%\"\r\nif not \"%SHIPPER_CARGO_STDOUT%\"==\"\" echo %SHIPPER_CARGO_STDOUT%\r\nif not \"%SHIPPER_CARGO_STDERR%\"==\"\" echo %SHIPPER_CARGO_STDERR% 1>&2\r\nif \"%SHIPPER_CARGO_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_CARGO_EXIT%)\r\n",
            )
            .expect("write fake cargo");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("cargo");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ -n \"$SHIPPER_CARGO_ARGS_LOG\" ]; then\n  echo \"$*\" >>\"$SHIPPER_CARGO_ARGS_LOG\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDOUT\" ]; then\n  echo \"$SHIPPER_CARGO_STDOUT\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDERR\" ]; then\n  echo \"$SHIPPER_CARGO_STDERR\" >&2\nfi\nexit \"${SHIPPER_CARGO_EXIT:-0}\"\n",
            )
            .expect("write fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod");
        }
    }

    fn write_fake_git(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("git.cmd"),
                "@echo off\r\nif \"%SHIPPER_GIT_FAIL%\"==\"1\" (\r\n  echo fatal: git failed 1>&2\r\n  exit /b 1\r\n)\r\nif \"%SHIPPER_GIT_CLEAN%\"==\"0\" (\r\n  echo M src/lib.rs\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"${SHIPPER_GIT_FAIL:-0}\" = \"1\" ]; then\n  echo 'fatal: git failed' >&2\n  exit 1\nfi\nif [ \"${SHIPPER_GIT_CLEAN:-1}\" = \"0\" ]; then\n  echo 'M src/lib.rs'\nfi\nexit 0\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod");
        }
    }

    fn write_fake_tools(bin_dir: &Path) {
        fs::create_dir_all(bin_dir).expect("mkdir");
        write_fake_cargo(bin_dir);
        write_fake_git(bin_dir);
    }

    struct TestRegistryServer {
        base_url: String,
        seen: Arc<Mutex<Vec<(String, Option<String>)>>>,
        handle: thread::JoinHandle<()>,
    }

    impl TestRegistryServer {
        fn join(self) {
            self.handle.join().expect("join server");
        }
    }

    fn spawn_registry_server(
        mut routes: std::collections::BTreeMap<String, Vec<(u16, String)>>,
        expected_requests: usize,
    ) -> TestRegistryServer {
        let server = Server::http("127.0.0.1:0").expect("server");
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
        let seen_thread = Arc::clone(&seen);

        let handle = thread::spawn(move || {
            for _ in 0..expected_requests {
                let req = server.recv().expect("request");
                let path = req.url().to_string();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                seen_thread.lock().expect("lock").push((path.clone(), auth));

                let response = if let Some(list) = routes.get_mut(&path) {
                    if list.is_empty() {
                        (404, "{}".to_string())
                    } else if list.len() == 1 {
                        list[0].clone()
                    } else {
                        list.remove(0)
                    }
                } else {
                    (404, "{}".to_string())
                };

                let resp = Response::from_string(response.1)
                    .with_status_code(StatusCode(response.0))
                    .with_header(
                        Header::from_bytes("Content-Type", "application/json").expect("header"),
                    );
                req.respond(resp).expect("respond");
            }
        });

        TestRegistryServer {
            base_url,
            seen,
            handle,
        }
    }

    fn planned_workspace(workspace_root: &Path, api_base: String) -> PlannedWorkspace {
        PlannedWorkspace {
            workspace_root: workspace_root.to_path_buf(),
            plan: ReleasePlan {
                plan_id: "plan-demo".to_string(),
                created_at: Utc::now(),
                registry: Registry {
                    name: "crates-io".to_string(),
                    api_base,
                },
                packages: vec![PlannedPackage {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: workspace_root.join("demo").join("Cargo.toml"),
                }],
            },
        }
    }

    fn default_opts(state_dir: PathBuf) -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            verify_timeout: Duration::from_millis(20),
            verify_poll_interval: Duration::from_millis(1),
            state_dir,
            force_resume: false,
        }
    }

    #[test]
    fn classify_cargo_failure_covers_retryable_permanent_and_ambiguous() {
        let retryable = classify_cargo_failure("HTTP 429 too many requests", "");
        assert_eq!(retryable.0, ErrorClass::Retryable);

        let permanent = classify_cargo_failure("permission denied", "");
        assert_eq!(permanent.0, ErrorClass::Permanent);

        let ambiguous = classify_cargo_failure("strange output", "");
        assert_eq!(ambiguous.0, ErrorClass::Ambiguous);
    }

    #[test]
    fn collecting_reporter_error_method_records_message() {
        let mut reporter = CollectingReporter::default();
        reporter.error("boom");
        assert_eq!(reporter.errors, vec!["boom".to_string()]);
    }

    #[test]
    fn helper_functions_return_expected_values() {
        let root = PathBuf::from("root");
        let rel = resolve_state_dir(&root, &PathBuf::from(".shipper"));
        assert_eq!(rel, root.join(".shipper"));

        #[cfg(windows)]
        {
            let abs = PathBuf::from(r"C:\x\state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }
        #[cfg(not(windows))]
        {
            let abs = PathBuf::from("/x/state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }

        assert_eq!(pkg_key("a", "1.2.3"), "a@1.2.3");
        assert_eq!(short_state(&PackageState::Pending), "pending");
        assert_eq!(short_state(&PackageState::Published), "published");
        assert_eq!(
            short_state(&PackageState::Skipped {
                reason: "r".to_string()
            }),
            "skipped"
        );
        assert_eq!(
            short_state(&PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "m".to_string()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&PackageState::Ambiguous {
                message: "m".to_string()
            }),
            "ambiguous"
        );
    }

    #[test]
    fn backoff_delay_is_bounded_with_jitter() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);
        let d1 = backoff_delay(base, max, 1);
        let d20 = backoff_delay(base, max, 20);

        assert!(d1 >= Duration::from_millis(50));
        assert!(d1 <= Duration::from_millis(150));

        assert!(d20 >= Duration::from_millis(250));
        assert!(d20 <= Duration::from_millis(750));
    }

    #[test]
    fn verify_published_returns_true_when_registry_visibility_appears() {
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );

        let reg = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
        })
        .expect("client");

        let mut reporter = CollectingReporter::default();
        let ok = verify_published(
            &reg,
            "demo",
            "0.1.0",
            Duration::from_millis(500),
            Duration::from_millis(1),
            &mut reporter,
        )
        .expect("verify");
        assert!(ok);
        assert!(!reporter.infos.is_empty());
        server.join();
    }

    #[test]
    fn verify_published_returns_false_on_timeout() {
        let reg = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: "http://127.0.0.1:9".to_string(),
        })
        .expect("client");

        let mut reporter = CollectingReporter::default();
        let ok = verify_published(
            &reg,
            "demo",
            "0.1.0",
            Duration::from_millis(0),
            Duration::from_millis(1),
            &mut reporter,
        )
        .expect("verify");
        assert!(!ok);
    }

    #[test]
    fn registry_server_helper_returns_404_for_unknown_or_empty_routes() {
        let server_unknown = spawn_registry_server(std::collections::BTreeMap::new(), 1);
        let reg_unknown = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server_unknown.base_url.clone(),
        })
        .expect("client");
        let exists_unknown = reg_unknown
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_unknown);
        server_unknown.join();

        let server_empty = spawn_registry_server(
            std::collections::BTreeMap::from([("/api/v1/crates/demo/0.1.0".to_string(), vec![])]),
            1,
        );
        let reg_empty = RegistryClient::new(Registry {
            name: "crates-io".to_string(),
            api_base: server_empty.base_url.clone(),
        })
        .expect("client");
        let exists_empty = reg_empty
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_empty);
        server_empty.join();
    }

    #[test]
    #[serial]
    fn run_preflight_errors_in_strict_mode_without_token() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.strict_ownership = true;
        opts.skip_ownership_check = false;

        let mut reporter = CollectingReporter::default();
        let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("strict ownership requested but no token found"));
    }

    #[test]
    #[serial]
    fn run_preflight_warns_on_owners_failure_when_not_strict() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(403, "{}".to_string())],
                ),
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = false;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert!(rep.token_detected);
        assert_eq!(rep.packages.len(), 1);
        assert!(!rep.packages[0].already_published);
        assert!(reporter
            .warns
            .iter()
            .any(|w| w.contains("owners preflight failed")));

        assert_debug_snapshot!(
            rep,
            @r#"
PreflightReport {
    plan_id: "plan-demo",
    token_detected: true,
    packages: [
        PreflightPackage {
            name: "demo",
            version: "0.1.0",
            already_published: false,
        },
    ],
}
"#
        );
        let seen = server.seen.lock().expect("lock");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1.as_deref(), Some("token-abc"));
        drop(seen);
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_owners_success_path() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([
                (
                    "/api/v1/crates/demo/owners".to_string(),
                    vec![(
                        200,
                        r#"{"users":[{"id":1,"login":"alice","name":"Alice"}]}"#.to_string(),
                    )],
                ),
                (
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                ),
            ]),
            2,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = false;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert_eq!(rep.packages.len(), 1);
        assert!(reporter.warns.is_empty());
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_returns_error_when_strict_ownership_check_fails() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/owners".to_string(),
                vec![(403, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.skip_ownership_check = false;
        opts.strict_ownership = true;

        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-abc");

        let mut reporter = CollectingReporter::default();
        let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("forbidden when querying owners"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_preflight_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _git_clean = EnvGuard::set("SHIPPER_GIT_CLEAN", "1");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = false;
        opts.skip_ownership_check = true;

        let mut reporter = CollectingReporter::default();
        let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
        assert_eq!(rep.packages.len(), 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_skips_when_version_already_exists() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(
            receipt.packages[0].state,
            PackageState::Skipped { .. }
        ));

        let state_dir = td.path().join(".shipper");
        assert!(state::state_path(&state_dir).exists());
        assert!(state::receipt_path(&state_dir).exists());
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _git_clean = EnvGuard::set("SHIPPER_GIT_CLEAN", "1");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.allow_dirty = false;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_adds_missing_package_entries_to_existing_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let state_dir = td.path().join(".shipper");
        let existing = ExecutionState {
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        state::save_state(&state_dir, &existing).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

        let st = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert!(st.packages.contains_key("demo@0.1.0"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_marks_published_after_successful_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.verify_timeout = Duration::from_millis(200);
        opts.verify_poll_interval = Duration::from_millis(1);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_errors_when_verify_check_returns_unexpected_status() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (500, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.verify_timeout = Duration::from_millis(200);
        opts.verify_poll_interval = Duration::from_millis(1);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("unexpected status while checking version existence"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_treats_failed_cargo_as_published_if_registry_shows_version() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "timeout while uploading");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert_eq!(receipt.packages[0].attempts, 1);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_retries_on_retryable_failures() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "timeout talking to server");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    (200, "{}".to_string()),
                ],
            )]),
            3,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 2;
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert_eq!(receipt.packages[0].attempts, 2);
        assert!(reporter.warns.iter().any(|w| w.contains("retrying in")));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_errors_when_cargo_command_cannot_start() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let missing = td.path().join("no-cargo-here");
        let _cargo_bin = EnvGuard::set("SHIPPER_CARGO_BIN", missing.to_str().expect("utf8"));

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to execute cargo publish"));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_returns_error_on_permanent_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "1");
        let _cargo_err = EnvGuard::set("SHIPPER_CARGO_STDERR", "permission denied");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.base_delay = Duration::from_millis(0);
        opts.max_delay = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("permanent failure"));

        let st = state::load_state(&td.path().join(".shipper"))
            .expect("load")
            .expect("exists");
        let pkg = st.packages.get("demo@0.1.0").expect("pkg");
        assert!(matches!(
            pkg.state,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                ..
            }
        ));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_marks_ambiguous_failure_after_success_without_registry_visibility() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (404, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.verify_timeout = Duration::from_millis(0);
        opts.verify_poll_interval = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed"));

        let st = state::load_state(&td.path().join(".shipper"))
            .expect("load")
            .expect("exists");
        let pkg = st.packages.get("demo@0.1.0").expect("pkg");
        assert!(matches!(
            pkg.state,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                ..
            }
        ));
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_recovers_on_final_registry_check_after_ambiguous_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let (_cargo_bin, _git_bin) = configure_fake_programs(&bin);
        let _cargo_exit = EnvGuard::set("SHIPPER_CARGO_EXIT", "0");

        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.verify_timeout = Duration::from_millis(0);
        opts.verify_poll_interval = Duration::from_millis(0);

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        server.join();
    }

    #[test]
    fn run_publish_errors_on_plan_mismatch_without_force_resume() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("does not match current plan_id"));
    }

    #[test]
    fn run_publish_allows_forced_resume_with_plan_mismatch() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.force_resume = true;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(receipt.packages.is_empty());
        assert!(reporter
            .warns
            .iter()
            .any(|w| w.contains("forcing resume with mismatched plan_id")));
    }

    #[test]
    fn run_resume_errors_when_state_is_missing() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let err = run_resume(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("no existing state found"));
    }

    #[test]
    fn run_resume_runs_publish_when_state_exists() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let receipt = run_resume(&ws, &opts, &mut reporter).expect("resume");
        assert!(receipt.packages.is_empty());
    }
}
