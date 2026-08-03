//! Single-responsibility helpers for the sequential publish orchestrator.
//!
//! `engine::run_publish` remains the public entry point, while this module
//! owns the mechanically separate pieces around bootstrap, resume-gating, and
//! end-of-run finalization.

pub(super) mod bootstrap;
pub(super) mod finalize;
pub(super) mod resume;

use crate::engine::parallel::webhook::{self, WebhookEvent};
use crate::plan::PlannedWorkspace;
use crate::types::RuntimeOptions;

/// Send the single run-start notification after publish preparation succeeds.
///
/// Keeping this at the publish-orchestration boundary prevents scheduler mode
/// from changing the run-level webhook contract.
pub(crate) fn notify_publish_started(ws: &PlannedWorkspace, opts: &RuntimeOptions) {
    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishStarted {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
            registry: ws.plan.registry.name.clone(),
        },
    );
}
