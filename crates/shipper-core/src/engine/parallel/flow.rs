use std::sync::{Arc, Mutex};

use anyhow::Result;
use shipper_types::{ExecutionState, PackageState, PlannedPackage, PublishLevel};

use super::SendReporter;

pub(super) fn init_send_reporter() -> SendReporter {
    SendReporter::default()
}

/// Return the first publish level selected by `--resume-from`.
///
/// Parallel resume operates on whole dependency levels: selecting one package
/// selects its siblings in that level and every later level. Both admission
/// checks and the scheduler use this owner so they cannot disagree about which
/// packages may run.
pub(crate) fn parallel_resume_start_level(
    levels: &[PublishLevel],
    resume_from: Option<&str>,
) -> Option<usize> {
    match resume_from {
        None => Some(0),
        Some(resume_point) => levels
            .iter()
            .position(|level| level.packages.iter().any(|p| p.name == resume_point)),
    }
}

pub(super) fn is_level_already_complete(
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
