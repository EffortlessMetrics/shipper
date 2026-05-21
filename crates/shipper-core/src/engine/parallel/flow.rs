use std::sync::{Arc, Mutex};

use shipper_types::PlannedPackage;
use shipper_types::{ExecutionState, PackageEvidence, PackageReceipt, PackageState};

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
) -> LevelResumeAction {
    let Some(resume_point) = resume_from else {
        return LevelResumeAction::ReachedResumePoint;
    };

    if level_packages.iter().any(|p| p.name == resume_point) {
        return LevelResumeAction::ReachedResumePoint;
    }

    if is_level_already_complete(level_packages, st_arc) {
        LevelResumeAction::SkipAlreadyComplete
    } else {
        LevelResumeAction::SkipBeforeResumePoint(resume_point.to_string())
    }
}

pub(super) fn collect_level_receipts_from_state(
    level_packages: &[PlannedPackage],
    st_arc: &Arc<Mutex<ExecutionState>>,
) -> Vec<PackageReceipt> {
    let st_guard = st_arc.lock().unwrap();

    level_packages
        .iter()
        .filter_map(|p| {
            let key = crate::runtime::execution::pkg_key(&p.name, &p.version);
            st_guard.packages.get(&key).map(|progress| PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: progress.attempts,
                state: progress.state.clone(),
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
                duration_ms: 0,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            })
        })
        .collect()
}

fn is_level_already_complete(
    level_packages: &[PlannedPackage],
    st_arc: &Arc<Mutex<ExecutionState>>,
) -> bool {
    let st_guard = st_arc.lock().unwrap();

    level_packages.iter().all(|p| {
        let key = crate::runtime::execution::pkg_key(&p.name, &p.version);
        st_guard.packages.get(&key).is_some_and(|progress| {
            matches!(
                progress.state,
                PackageState::Published | PackageState::Skipped { .. }
            )
        })
    })
}
