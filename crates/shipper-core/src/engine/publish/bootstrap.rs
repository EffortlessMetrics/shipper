use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

use crate::engine::{Reporter, enforce_rehearsal_gate, init_registry_client, init_state};
use crate::git;
use crate::lock;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::runtime::environment;
use crate::runtime::execution::{pkg_key, resolve_state_dir};
use crate::state::events;
use crate::state::execution_state as state;
use crate::types::{
    AuthEvidence, EnvironmentFingerprint, EventType, ExecutionState, GitContext, PackageProgress,
    PackageState, PublishEvent, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

pub(in crate::engine) struct PublishBootstrap {
    pub(in crate::engine) state_dir: PathBuf,
    pub(in crate::engine) _lock: lock::LockFile,
    pub(in crate::engine) git_context: Option<GitContext>,
    pub(in crate::engine) environment: EnvironmentFingerprint,
    pub(in crate::engine) auth_evidence: AuthEvidence,
    pub(in crate::engine) registry: RegistryClient,
    pub(in crate::engine) events_path: PathBuf,
    pub(in crate::engine) event_log: events::EventLog,
    pub(in crate::engine) state: ExecutionState,
    pub(in crate::engine) run_started: DateTime<Utc>,
}

pub(in crate::engine) fn validate_resume_target(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<()> {
    if let Some(ref target) = opts.resume_from
        && !ws.plan.packages.iter().any(|p| &p.name == target)
    {
        bail!("resume-from package '{}' not found in publish plan", target);
    }

    Ok(())
}

pub(in crate::engine) fn prepare_publish_run(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PublishBootstrap> {
    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);

    // #97 PR 3: rehearsal hard gate. Only fires when a rehearsal registry
    // is configured; opt-in until rehearsal phase-2 is stable.
    enforce_rehearsal_gate(ws, opts, &state_dir, reporter)?;

    let lock = acquire_publish_lock(&state_dir, workspace_root, opts, &ws.plan.plan_id)?;

    // Collect git context and environment fingerprint at start of execution.
    let git_context = git::collect_git_context();
    let environment = environment::collect_environment_fingerprint();
    let auth_evidence = crate::ops::auth::collect_auth_evidence(&ws.plan.registry.name);

    if !opts.allow_dirty {
        git::ensure_git_clean(workspace_root)?;
    }

    let registry = init_registry_client(ws.plan.registry.clone(), &state_dir)?;
    let events_path = events::events_path(&state_dir);
    let mut event_log = events::EventLog::new();
    let mut state = load_or_initialize_state(ws, opts, &state_dir, reporter)?;

    reporter.info(&format!("state dir: {}", state_dir.as_path().display()));

    let run_started = Utc::now();
    record_execution_start(
        ws,
        opts,
        &events_path,
        &mut event_log,
        run_started,
        &auth_evidence,
    )?;
    ensure_plan_package_entries(ws, &state_dir, &mut state)?;

    Ok(PublishBootstrap {
        state_dir,
        _lock: lock,
        git_context,
        environment,
        auth_evidence,
        registry,
        events_path,
        event_log,
        state,
        run_started,
    })
}

fn acquire_publish_lock(
    state_dir: &Path,
    workspace_root: &Path,
    opts: &RuntimeOptions,
    plan_id: &str,
) -> Result<lock::LockFile> {
    let lock_timeout = if opts.force {
        Duration::ZERO
    } else {
        opts.lock_timeout
    };
    let lock = lock::LockFile::acquire_with_timeout(state_dir, Some(workspace_root), lock_timeout)
        .context("failed to acquire publish lock")?;
    lock.set_plan_id(plan_id)?;
    Ok(lock)
}

fn load_or_initialize_state(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    reporter: &mut dyn Reporter,
) -> Result<ExecutionState> {
    match state::load_state(state_dir)? {
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
            Ok(existing)
        }
        None => init_state(ws, state_dir),
    }
}

fn record_execution_start(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    events_path: &Path,
    event_log: &mut events::EventLog,
    run_started: DateTime<Utc>,
    auth_evidence: &AuthEvidence,
) -> Result<()> {
    event_log.record(PublishEvent {
        timestamp: run_started,
        event_type: EventType::ExecutionStarted,
        package: "all".to_string(),
    });

    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishStarted {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
            registry: ws.plan.registry.name.clone(),
        },
    );

    event_log.record(PublishEvent {
        timestamp: run_started,
        event_type: EventType::PlanCreated {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    event_log.record(PublishEvent {
        timestamp: run_started,
        event_type: EventType::AuthEvidenceRecorded {
            evidence: auth_evidence.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;
    event_log.clear();
    Ok(())
}

fn ensure_plan_package_entries(
    ws: &PlannedWorkspace,
    state_dir: &Path,
    state: &mut ExecutionState,
) -> Result<()> {
    for p in &ws.plan.packages {
        let key = pkg_key(&p.name, &p.version);
        state
            .packages
            .entry(key)
            .or_insert_with(|| PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            });
    }
    state.updated_at = Utc::now();
    state::save_state(state_dir, state)
}
