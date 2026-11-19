//! Test-only scheduling at the real event-derived snapshot/replacement boundary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ReconciliationReport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WritePhase {
    Snapshot,
    Replaced,
}

type Observer = Arc<dyn Fn(WritePhase, &ReconciliationReport) + Send + Sync>;
static OBSERVER: Mutex<Option<(PathBuf, Observer)>> = Mutex::new(None);

pub(crate) struct ObserverGuard;

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        *OBSERVER.lock().expect("report observer lock") = None;
    }
}

pub(crate) fn install(state_dir: &Path, observer: Observer) -> ObserverGuard {
    let mut current = OBSERVER.lock().expect("report observer lock");
    assert!(
        current.is_none(),
        "report observers must be installed serially"
    );
    *current = Some((state_dir.to_path_buf(), observer));
    ObserverGuard
}

pub(super) fn observe(state_dir: &Path, phase: WritePhase, report: &ReconciliationReport) {
    let observer = OBSERVER
        .lock()
        .expect("report observer lock")
        .as_ref()
        .filter(|(path, _)| path == state_dir)
        .map(|(_, observer)| Arc::clone(observer));
    // A blocked callback must not hold the observer registry or affect a
    // different fixture's state directory.
    if let Some(observer) = observer {
        observer(phase, report);
    }
}
