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
    ErrorClass, ExecutionState, PackageProgress, PackageReceipt, PackageState, Receipt, RuntimeOptions,
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

    reporter.info(&format!(
        "state dir: {}",
        state_dir.as_path().display()
    ));

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
            reporter.info(&format!("{}@{}: already published (skipping)", p.name, p.version));
            update_state(&mut st, &state_dir, &key, PackageState::Skipped { reason: "already published".into() })?;
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

            reporter.info(&format!("{}@{}: attempt {}/{}", p.name, p.version, attempt, opts.max_attempts));

            let out = cargo::cargo_publish(
                workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
            )?;

            let success = out.status_code == Some(0);

            if success {
                reporter.info(&format!("{}@{}: cargo publish exited successfully; verifying...", p.name, p.version));
                if verify_published(&reg, &p.name, &p.version, opts.verify_timeout, opts.verify_poll_interval, reporter)? {
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
                    reporter.info(&format!("{}@{}: version is present on registry; treating as published", p.name, p.version));
                    update_state(&mut st, &state_dir, &key, PackageState::Published)?;
                    last_err = None;
                    break;
                }

                let (class, msg) = classify_cargo_failure(&out.stderr, &out.stdout);
                last_err = Some((class.clone(), msg.clone()));

                match class {
                    ErrorClass::Permanent => {
                        update_state(&mut st, &state_dir, &key, PackageState::Failed { class, message: msg })?;
                        return Err(anyhow::anyhow!("{}@{}: permanent failure: {}", p.name, p.version, last_err.unwrap().1));
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        let delay = backoff_delay(opts.base_delay, opts.max_delay, attempt);
                        reporter.warn(&format!("{}@{}: retrying in {}", p.name, p.version, humantime::format_duration(delay)));
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
                update_state(
                    &mut st,
                    &state_dir,
                    &key,
                    PackageState::Failed {
                        class: class.clone(),
                        message: msg.clone(),
                    },
                )?;
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
        bail!("no existing state found in {}; run shipper publish first", state_dir.display());
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

fn update_state(st: &mut ExecutionState, state_dir: &Path, key: &str, new_state: PackageState) -> Result<()> {
    let pr = st.packages.get_mut(key).context("missing package in state")?;
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
        return (ErrorClass::Retryable, "transient failure (retryable)".into());
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
        return (ErrorClass::Permanent, "permanent failure (fix required)".into());
    }

    // Ambiguous: default. We'll always verify registry before failing.
    (ErrorClass::Ambiguous, "publish outcome ambiguous; registry did not show version".into())
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
