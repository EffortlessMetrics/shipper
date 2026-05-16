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

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::time::Duration;

    use tempfile::TempDir;

    use crate::encryption::EncryptionConfig;
    use crate::types::{
        ParallelConfig, PublishPolicy, ReadinessConfig, ReadinessMethod, Registry, VerifyMode,
    };
    use crate::webhook::WebhookConfig;

    #[derive(Default)]
    struct CollectingReporter {
        infos: Vec<String>,
        warns: Vec<String>,
        #[allow(dead_code)]
        errors: Vec<String>,
    }

    impl Reporter for CollectingReporter {
        fn info(&mut self, msg: &str) {
            self.infos.push(msg.to_string());
        }
        fn warn(&mut self, msg: &str) {
            self.warns.push(msg.to_string());
        }
        fn error(&mut self, msg: &str) {
            self.errors.push(msg.to_string());
        }
    }

    fn pkg(name: &str) -> PlannedPackage {
        PlannedPackage {
            name: name.to_string(),
            version: "1.2.3".to_string(),
            manifest_path: PathBuf::from(format!("{name}/Cargo.toml")),
            regime: None,
        }
    }

    fn progress(state: PackageState) -> PackageProgress {
        PackageProgress {
            name: "p".to_string(),
            version: "1.2.3".to_string(),
            attempts: 0,
            state,
            last_updated_at: Utc::now(),
        }
    }

    fn opts_with_resume_from(resume_from: Option<&str>) -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            retry_strategy: shipper_retry::RetryStrategyType::Exponential,
            retry_jitter: 0.0,
            retry_per_error: shipper_retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_millis(20),
            verify_poll_interval: Duration::from_millis(1),
            state_dir: PathBuf::from(".shipper"),
            force_resume: false,
            policy: PublishPolicy::default(),
            verify_mode: VerifyMode::default(),
            readiness: ReadinessConfig {
                enabled: false,
                method: ReadinessMethod::Api,
                initial_delay: Duration::from_millis(0),
                max_delay: Duration::from_millis(0),
                max_total_wait: Duration::from_millis(0),
                poll_interval: Duration::from_millis(0),
                jitter_factor: 0.0,
                index_path: None,
                prefer_index: false,
            },
            output_lines: 10,
            force: false,
            lock_timeout: Duration::from_secs(60),
            parallel: ParallelConfig::default(),
            webhook: WebhookConfig::default(),
            encryption: EncryptionConfig::default(),
            registries: vec![Registry::crates_io()],
            resume_from: resume_from.map(|s| s.to_string()),
            rehearsal_registry: None,
            rehearsal_skip: false,
            rehearsal_smoke_install: None,
        }
    }

    // ---- apply_resume_from_gate ----

    #[test]
    fn gate_publishes_when_no_resume_from_configured() {
        let mut reached = false;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(None);

        let decision = apply_resume_from_gate(
            &pkg("a"),
            &progress(PackageState::Pending),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Publish));
        assert!(
            reached,
            "absence of resume_from should mark resume point as reached"
        );
        assert!(reporter.warns.is_empty());
        assert!(reporter.infos.is_empty());
    }

    #[test]
    fn gate_publishes_once_resume_point_already_reached() {
        // Even with resume_from set, prior packages flipped the flag — gate
        // must short-circuit to Publish without inspecting state.
        let mut reached = true;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(Some("b"));

        let decision = apply_resume_from_gate(
            &pkg("c"),
            &progress(PackageState::Pending),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Publish));
        assert!(reached);
        assert!(reporter.warns.is_empty());
        assert!(reporter.infos.is_empty());
    }

    #[test]
    fn gate_flips_reached_and_publishes_when_target_matches() {
        let mut reached = false;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(Some("b"));

        let decision = apply_resume_from_gate(
            &pkg("b"),
            &progress(PackageState::Pending),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Publish));
        assert!(
            reached,
            "matching resume_from name should mark resume point as reached"
        );
        // No skip narration when we match — caller will start publishing.
        assert!(reporter.warns.is_empty());
        assert!(reporter.infos.is_empty());
    }

    #[test]
    fn gate_skips_pending_packages_before_resume_point_with_warn() {
        let mut reached = false;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(Some("b"));

        let decision = apply_resume_from_gate(
            &pkg("a"),
            &progress(PackageState::Pending),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Skip));
        assert!(
            !reached,
            "pending package before resume point must NOT flip reached"
        );
        assert_eq!(reporter.warns.len(), 1, "{:?}", reporter.warns);
        assert!(reporter.warns[0].contains("a@1.2.3"));
        assert!(reporter.warns[0].contains("before resume point b"));
        assert!(
            reporter.infos.is_empty(),
            "pending pre-resume should warn, not info"
        );
    }

    #[test]
    fn gate_logs_info_for_already_published_before_resume_point() {
        let mut reached = false;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(Some("b"));

        let decision = apply_resume_from_gate(
            &pkg("a"),
            &progress(PackageState::Published),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Skip));
        assert!(!reached);
        assert!(
            reporter.warns.is_empty(),
            "already-Published should be info, not warn"
        );
        assert_eq!(reporter.infos.len(), 1, "{:?}", reporter.infos);
        assert!(reporter.infos[0].contains("a@1.2.3"));
        assert!(reporter.infos[0].contains("already complete"));
    }

    #[test]
    fn gate_logs_info_for_already_skipped_before_resume_point() {
        let mut reached = false;
        let mut reporter = CollectingReporter::default();
        let opts = opts_with_resume_from(Some("b"));

        let decision = apply_resume_from_gate(
            &pkg("a"),
            &progress(PackageState::Skipped {
                reason: "already published".into(),
            }),
            &opts,
            &mut reached,
            &mut reporter,
        );

        assert!(matches!(decision, ResumeGate::Skip));
        assert!(!reached);
        assert!(reporter.warns.is_empty());
        assert_eq!(reporter.infos.len(), 1);
        assert!(reporter.infos[0].contains("already complete"));
    }

    // ---- record_terminal_resume_skip ----

    #[test]
    fn record_terminal_resume_skip_emits_info_and_event() {
        let dir = TempDir::new().expect("tempdir");
        let events_path = dir.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let mut reporter = CollectingReporter::default();

        record_terminal_resume_skip(
            &pkg("a"),
            &progress(PackageState::Published),
            "a@1.2.3",
            &events_path,
            &mut event_log,
            &mut reporter,
        )
        .expect("write events");

        // Reporter narration
        assert_eq!(reporter.infos.len(), 1);
        assert!(reporter.infos[0].contains("a@1.2.3"));
        assert!(reporter.infos[0].contains("already complete"));

        // Event was persisted and cleared from the in-memory log.
        let contents = std::fs::read_to_string(&events_path).expect("read events");
        assert!(!contents.is_empty(), "events file should be written");
        assert!(
            contents.contains("PackageSkipped") || contents.contains("package_skipped"),
            "expected PackageSkipped event, got: {contents}"
        );
        assert!(
            contents.contains("a@1.2.3"),
            "event should carry the package label"
        );
    }

    #[test]
    fn record_terminal_resume_skip_writes_skipped_reason_with_state_short_form() {
        let dir = TempDir::new().expect("tempdir");
        let events_path = dir.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let mut reporter = CollectingReporter::default();

        record_terminal_resume_skip(
            &pkg("a"),
            &progress(PackageState::Skipped {
                reason: "irrelevant".into(),
            }),
            "a@1.2.3",
            &events_path,
            &mut event_log,
            &mut reporter,
        )
        .expect("write events");

        // The reason in the recorded event should include "resume: state already <short-state>"
        let contents = std::fs::read_to_string(&events_path).expect("read events");
        assert!(
            contents.contains("resume: state already"),
            "expected resume-skip reason prefix, got: {contents}"
        );
    }
}
