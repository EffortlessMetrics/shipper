//! Unsupported read-only bridge for the separately packaged CLI adapter.

use std::path::Path;

use anyhow::Result;

use crate::types::{
    ControlledStopReason, ExecutionResult, ExecutionState, PlannedWorkspace, ReconciliationReport,
    ReleaseSpec, RuntimeOptions,
};

/// Workspace metadata captured before the CLI resolves workspace configuration.
///
/// Registry-dependent filtering and plan identity are computed only after the
/// caller supplies the effective target. Multiple targets share this snapshot
/// rather than rerunning Cargo or relabeling a plan for another registry.
pub struct PlanningWorkspace {
    metadata: cargo_metadata::Metadata,
}

impl PlanningWorkspace {
    pub fn load(manifest_path: &Path) -> Result<Self> {
        Ok(Self {
            metadata: crate::ops::cargo::load_metadata(manifest_path)?,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        self.metadata.workspace_root.as_std_path()
    }

    pub fn build_plan(&self, spec: &ReleaseSpec) -> Result<PlannedWorkspace> {
        crate::plan::build_plan_from_metadata(spec, &self.metadata)
    }
}

/// Apply the engine's resume-selection guard before any selected target runs.
/// The engine also retains this check for direct publish/resume callers.
pub fn validate_resume_target(ws: &PlannedWorkspace, opts: &RuntimeOptions) -> Result<()> {
    crate::engine::validate_resume_target(ws, opts)
}

/// Why unfinished evidence could not prove whether its publisher is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownLivenessReason {
    MissingLock,
    CorruptLock,
    CrossHost,
    LegacyLock,
    MissingPlanIdentity,
    PlanMismatch,
    ProcessIdentityMismatch,
    ProcessProbeUnavailable,
}

/// Fail-closed liveness classification for an unfinished run segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLiveness {
    Live,
    NotLive,
    Unknown(UnknownLivenessReason),
}

/// Current durable run segment. Finished segments cannot carry liveness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunObservation {
    NoEvidence,
    Unfinished {
        plan_id: Option<String>,
        liveness: RunLiveness,
    },
    Stopped {
        plan_id: Option<String>,
        reason: ControlledStopReason,
        package: String,
        /// `None` means the lock is absent after an authoritative controlled
        /// stop. Present lock evidence retains the normal fail-closed probe.
        liveness: Option<RunLiveness>,
    },
    Finished {
        plan_id: Option<String>,
        result: ExecutionResult,
    },
}

/// Detect retryable NotPublished posture without treating it as authorization.
pub fn has_not_published_retryable_posture(
    state: &ExecutionState,
    reconciliation: Option<&ReconciliationReport>,
) -> bool {
    crate::state::consistency::has_not_published_retryable_posture(state, reconciliation)
}

/// Read authoritative events and advisory lock evidence without modifying it.
pub fn observe_run(state_dir: &Path, workspace_root: Option<&Path>) -> Result<RunObservation> {
    crate::state::run_observation::observe_run(state_dir, workspace_root)
}

/// Check nonterminal state and reconciliation evidence against authoritative events.
pub fn unfinished_evidence_consistent(
    events_path: &Path,
    state: &ExecutionState,
    reconciliation: Option<&ReconciliationReport>,
) -> Result<bool> {
    Ok(
        crate::state::consistency::verify_unfinished_consistency(
            events_path,
            state,
            reconciliation,
        )
        .is_ok(),
    )
}

/// Check that a controlled-stop marker agrees with state and reconciliation.
pub fn controlled_stop_evidence_consistent(
    events_path: &Path,
    state: &ExecutionState,
    reconciliation: Option<&ReconciliationReport>,
) -> Result<bool> {
    Ok(
        crate::state::consistency::verify_controlled_stop_consistency(
            events_path,
            state,
            reconciliation,
        )
        .is_ok(),
    )
}
