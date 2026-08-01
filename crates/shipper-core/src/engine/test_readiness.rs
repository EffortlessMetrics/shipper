//! Test-only readiness adapter for engine-level characterization tests.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;

use super::Reporter;
use crate::registry::RegistryClient;
use crate::state::events;
use crate::types::{EventType, PublishEvent, ReadinessEvidence};

#[cfg(test)]
pub(crate) fn verify_published(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &crate::types::ReadinessConfig,
    reporter: &mut dyn Reporter,
    event_log: &mut events::EventLog,
    events_path: &Path,
    pkg_label: &str,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    record_readiness_event(
        event_log,
        events_path,
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessStarted {
                method: config.method,
            },
            package: pkg_label.to_string(),
        },
    )?;
    verify_published_inner(
        reg,
        crate_name,
        version,
        config,
        reporter,
        event_log,
        events_path,
        pkg_label,
    )
}

fn verify_published_inner(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &crate::types::ReadinessConfig,
    reporter: &mut dyn Reporter,
    event_log: &mut events::EventLog,
    events_path: &Path,
    pkg_label: &str,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    reporter.info(&format!(
        "{}@{}: readiness check ({:?})...",
        crate_name, version, config.method
    ));
    let started_at = Instant::now();
    let mut emit_event = |event| record_readiness_event(event_log, events_path, event);
    let (visible, evidence) = crate::engine::readiness::is_version_visible_with_backoff_and_events(
        reg,
        crate_name,
        version,
        config,
        &mut emit_event,
    )?;
    if visible {
        reporter.info(&format!(
            "{}@{}: visible after {} checks",
            crate_name,
            version,
            evidence.len()
        ));
        record_readiness_event(
            event_log,
            events_path,
            PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::ReadinessComplete {
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    attempts: evidence.len() as u32,
                },
                package: pkg_label.to_string(),
            },
        )?;
    } else {
        reporter.warn(&format!(
            "{}@{}: not visible after {} checks",
            crate_name,
            version,
            evidence.len()
        ));
        record_readiness_event(
            event_log,
            events_path,
            PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::ReadinessTimeout {
                    max_wait_ms: config.max_total_wait.as_millis() as u64,
                },
                package: pkg_label.to_string(),
            },
        )?;
    }
    Ok((visible, evidence))
}

fn record_readiness_event(
    event_log: &mut events::EventLog,
    events_path: &Path,
    event: PublishEvent,
) -> Result<()> {
    event_log.record(event);
    event_log.write_to_file(events_path)?;
    event_log.clear();
    Ok(())
}
