use anyhow::Result;
use chrono::Utc;

use crate::engine::Reporter;
use crate::runtime::execution::short_state;
use crate::state::events;
use crate::types::{
    EventType, PackageProgress, PackageState, PlannedPackage, PublishEvent, RuntimeOptions,
};

pub(in crate::engine) enum ResumeGate {
    Publish,
    Skip,
}

pub(in crate::engine) fn apply_resume_from_gate(
    package: &PlannedPackage,
    progress: &PackageProgress,
    opts: &RuntimeOptions,
    reached_resume_point: &mut bool,
    reporter: &mut dyn Reporter,
) -> ResumeGate {
    if *reached_resume_point {
        return ResumeGate::Publish;
    }

    let Some(resume_from) = opts.resume_from.as_ref() else {
        *reached_resume_point = true;
        return ResumeGate::Publish;
    };

    if &package.name == resume_from {
        *reached_resume_point = true;
        return ResumeGate::Publish;
    }

    if matches!(
        progress.state,
        PackageState::Published | PackageState::Skipped { .. }
    ) {
        reporter.info(&format!(
            "{}@{}: already complete (skipping)",
            package.name, package.version
        ));
    } else {
        reporter.warn(&format!(
            "{}@{}: skipping (before resume point {})",
            package.name, package.version, resume_from
        ));
    }

    ResumeGate::Skip
}

pub(in crate::engine) fn record_terminal_resume_skip(
    package: &PlannedPackage,
    progress: &PackageProgress,
    pkg_label: &str,
    events_path: &std::path::Path,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
) -> Result<()> {
    let short = short_state(&progress.state);
    reporter.info(&format!(
        "{}@{}: already complete ({})",
        package.name, package.version, short
    ));

    // #125: explicitly record resume's "state already terminal, trusting it"
    // decision so events.jsonl stays legible even though historical receipt
    // shape excludes already-terminal packages in the resume path.
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageSkipped {
            reason: format!("resume: state already {short}"),
        },
        package: pkg_label.to_string(),
    });
    event_log.write_to_file(events_path)?;
    event_log.clear();
    Ok(())
}
