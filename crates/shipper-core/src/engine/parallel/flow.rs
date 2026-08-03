use std::sync::{Arc, Mutex};

use anyhow::Result;
use shipper_types::PlannedPackage;
use shipper_types::{ExecutionState, PackageState};

use super::SendReporter;

pub(super) enum LevelResumeAction {
    ReachedResumePoint,
    SkipAlreadyComplete,
    SkipBeforeResumePoint(String),
}

pub(super) fn init_send_reporter() -> SendReporter {
    SendReporter::default()
}

pub(super) fn determine_level_resume_action(
    level_packages: &[PlannedPackage],
    st_arc: &Arc<Mutex<ExecutionState>>,
    resume_from: Option<&str>,
) -> Result<LevelResumeAction> {
    let Some(resume_point) = resume_from else {
        return Ok(LevelResumeAction::ReachedResumePoint);
    };

    if level_packages.iter().any(|p| p.name == resume_point) {
        return Ok(LevelResumeAction::ReachedResumePoint);
    }

    if is_level_already_complete(level_packages, st_arc)? {
        Ok(LevelResumeAction::SkipAlreadyComplete)
    } else {
        Ok(LevelResumeAction::SkipBeforeResumePoint(
            resume_point.to_string(),
        ))
    }
}

fn is_level_already_complete(
    level_packages: &[PlannedPackage],
    st_arc: &Arc<Mutex<ExecutionState>>,
) -> Result<bool> {
    let st_guard = st_arc.lock().map_err(|_| {
        anyhow::anyhow!("execution state lock poisoned while checking completed level")
    })?;

    Ok(level_packages.iter().all(|p| {
        let key = crate::runtime::execution::pkg_key(&p.name, &p.version);
        st_guard.packages.get(&key).is_some_and(|progress| {
            matches!(
                progress.state,
                PackageState::Published | PackageState::Skipped { .. }
            )
        })
    }))
}
