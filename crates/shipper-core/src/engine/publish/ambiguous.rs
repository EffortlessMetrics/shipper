use std::path::Path;

use anyhow::{Result, bail};
use chrono::Utc;

use crate::engine::{Reporter, sequential_reconcile, write_reconciliation_report_best_effort};
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::runtime::execution::{pkg_key, update_state};
use crate::state::events::EventLog;
use crate::state::execution_state::reconciliation_path;
use crate::types::{
    ErrorClass, EventType, ExecutionState, PackageState, PublishEvent, ReadinessConfig,
    ReconciliationOutcome, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_ambiguous_resume_state(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    state_dir: &Path,
    events_path: &Path,
    event_log: &mut EventLog,
    st: &mut ExecutionState,
    pkg_name: &str,
    pkg_version: &str,
    prior_reason: &str,
    reporter: &mut dyn Reporter,
) -> Result<()> {
    reporter.warn(&format!(
        "{pkg_name}@{pkg_version}: resume found ambiguous state ({prior_reason}); reconciling against registry"
    ));

    let effects = crate::engine::policy_effects(opts);
    let readiness_config = ReadinessConfig {
        enabled: effects.readiness_enabled,
        ..opts.readiness.clone()
    };
    let pkg_label = format!("{pkg_name}@{pkg_version}");

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PublishReconciling {
            method: readiness_config.method,
        },
        package: pkg_label.clone(),
    });

    let (outcome, _evidence) = sequential_reconcile(reg, pkg_name, pkg_version, &readiness_config);

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PublishReconciled {
            outcome: outcome.clone(),
        },
        package: pkg_label,
    });
    event_log.write_to_file(events_path)?;
    event_log.clear();
    write_reconciliation_report_best_effort(state_dir, ws, events_path, reporter);
    let reconciliation_report_path = reconciliation_path(state_dir);
    let key = pkg_key(pkg_name, pkg_version);

    match outcome {
        ReconciliationOutcome::Published { .. } => {
            update_state(st, state_dir, &key, PackageState::Published)?;
            reporter.info(&format!(
                "{pkg_name}@{pkg_version}: reconciliation outcome: Published; action: mark published and continue without republish (evidence: {})",
                reconciliation_report_path.display()
            ));
            Ok(())
        }
        ReconciliationOutcome::NotPublished { .. } => {
            update_state(st, state_dir, &key, PackageState::Pending)?;
            reporter.info(&format!(
                "{pkg_name}@{pkg_version}: reconciliation outcome: NotPublished; action: retry under publish policy (evidence: {})",
                reconciliation_report_path.display()
            ));
            Ok(())
        }
        ReconciliationOutcome::StillUnknown { reason, .. } => {
            reporter.error(&format!(
                "{pkg_name}@{pkg_version}: reconciliation outcome: StillUnknown; action: stop before blind retry; operator action required (evidence: {}): {reason}",
                reconciliation_report_path.display(),
            ));
            webhook::maybe_send_event(
                &opts.webhook,
                WebhookEvent::PublishFailed {
                    plan_id: ws.plan.plan_id.clone(),
                    package_name: pkg_name.to_owned(),
                    package_version: pkg_version.to_owned(),
                    error_class: format!("{:?}", ErrorClass::Ambiguous),
                    message: format!("resume reconciliation still inconclusive: {reason}"),
                },
            );
            bail!(
                "{pkg_name}@{pkg_version}: resume reconciliation still inconclusive; operator action required. Prior reason: {reason}"
            )
        }
    }
}
