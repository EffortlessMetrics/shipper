use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

use crate::lock;
use crate::plan::PlannedWorkspace;
use crate::runtime::execution::pkg_key;
use crate::state::{events, execution_state as state};
use crate::types::{
    EventType, ExecutionState, PackageProgress, PackageState, PublishEvent, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

use super::super::Reporter;

pub(crate) fn validate_resume_target(ws: &PlannedWorkspace, opts: &RuntimeOptions) -> Result<()> {
    if let Some(ref target) = opts.resume_from
        && !ws.plan.packages.iter().any(|p| &p.name == target)
    {
        bail!("resume-from package '{}' not found in publish plan", target);
    }

    Ok(())
}

pub(crate) fn acquire_publish_lock(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
) -> Result<lock::LockFile> {
    let lock_timeout = if opts.force {
        Duration::ZERO
    } else {
        opts.lock_timeout
    };
    let lock = lock::LockFile::acquire_with_timeout(
        state_dir,
        Some(ws.workspace_root.as_path()),
        lock_timeout,
    )
    .context("failed to acquire publish lock")?;
    lock.set_plan_id(&ws.plan.plan_id)?;
    Ok(lock)
}

pub(crate) fn load_or_init_state(
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
        None => super::super::init_state(ws, state_dir),
    }
}

pub(crate) fn record_execution_start(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    event_log: &mut events::EventLog,
    run_started: DateTime<Utc>,
) -> Result<()> {
    let events_path = events::events_path(state_dir);

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
    event_log.write_to_file(&events_path)?;
    event_log.clear();

    Ok(())
}

pub(crate) fn ensure_package_state_entries(
    ws: &PlannedWorkspace,
    state_dir: &Path,
    st: &mut ExecutionState,
) -> Result<()> {
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
    state::save_state(state_dir, st)
}
