//! Fail-closed projection of authoritative run events and advisory lock evidence.

use crate::cli_bridge::{RunLiveness, RunObservation, UnknownLivenessReason};
use crate::lock::{
    LockContents, LockRecord, ProcessIdentity, ProcessScope, lock_path, process_identity,
    process_scope, read_lock_contents,
};
use crate::state::events::{EventLog, events_path};
use crate::types::{EventType, ExecutionResult};
use anyhow::Result;
use std::path::Path;

#[derive(Clone)]
enum ProcessStatus {
    Running(ProcessIdentity),
    NotRunning,
    Unavailable,
}

trait ProcessProbe {
    fn scope(&mut self) -> Option<ProcessScope>;
    fn status(&mut self, pid: u32) -> ProcessStatus;
}

struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn scope(&mut self) -> Option<ProcessScope> {
        process_scope().ok().flatten()
    }

    fn status(&mut self, pid: u32) -> ProcessStatus {
        match process_identity(pid) {
            Ok(Some(identity)) => ProcessStatus::Running(identity),
            Ok(None) if cfg!(target_os = "linux") => ProcessStatus::NotRunning,
            Ok(None) | Err(_) => ProcessStatus::Unavailable,
        }
    }
}

pub(crate) fn observe_run(
    state_dir: &Path,
    workspace_root: Option<&Path>,
) -> Result<RunObservation> {
    let mut probe = SystemProcessProbe;
    observe_run_with(
        state_dir,
        workspace_root,
        &gethostname::gethostname().to_string_lossy(),
        &mut probe,
    )
}

fn observe_run_with(
    state_dir: &Path,
    workspace_root: Option<&Path>,
    hostname: &str,
    probe: &mut dyn ProcessProbe,
) -> Result<RunObservation> {
    let events = EventLog::read_from_file(&events_path(state_dir))?;
    let phase = event_phase(&events);
    let contents = match &phase {
        EventPhase::Stopped { .. } | EventPhase::Unfinished { .. } => {
            read_lock_contents(&lock_path(state_dir, workspace_root))?
        }
        _ => LockContents::Missing,
    };
    Ok(observe_phase(phase, &contents, hostname, probe))
}

/// Observe fresh events against the predecessor captured while invocation
/// ownership was reserved, before this caller installs its own lock metadata.
pub(crate) fn observe_run_with_prior_lock(
    state_dir: &Path,
    prior: &LockContents,
) -> Result<RunObservation> {
    let events = EventLog::read_from_file(&events_path(state_dir))?;
    Ok(observe_phase(
        event_phase(&events),
        prior,
        &gethostname::gethostname().to_string_lossy(),
        &mut SystemProcessProbe,
    ))
}

fn observe_phase(
    phase: EventPhase,
    contents: &LockContents,
    hostname: &str,
    probe: &mut dyn ProcessProbe,
) -> RunObservation {
    match phase {
        EventPhase::NoEvidence => RunObservation::NoEvidence,
        EventPhase::Finished { plan_id, result } => RunObservation::Finished { plan_id, result },
        EventPhase::Stopped {
            plan_id,
            reason,
            package,
        } => {
            let liveness = match contents {
                LockContents::Missing => None,
                LockContents::Corrupt => {
                    Some(RunLiveness::Unknown(UnknownLivenessReason::CorruptLock))
                }
                LockContents::Present(record) => {
                    Some(observe_lock(record, plan_id.as_deref(), hostname, probe))
                }
            };
            RunObservation::Stopped {
                plan_id,
                reason,
                package,
                liveness,
            }
        }
        EventPhase::Unfinished { plan_id } => {
            let liveness = match contents {
                LockContents::Missing => RunLiveness::Unknown(UnknownLivenessReason::MissingLock),
                LockContents::Corrupt => RunLiveness::Unknown(UnknownLivenessReason::CorruptLock),
                LockContents::Present(record) => {
                    observe_lock(record, plan_id.as_deref(), hostname, probe)
                }
            };
            RunObservation::Unfinished { plan_id, liveness }
        }
    }
}

enum EventPhase {
    NoEvidence,
    Unfinished {
        plan_id: Option<String>,
    },
    Finished {
        plan_id: Option<String>,
        result: ExecutionResult,
    },
    Stopped {
        plan_id: Option<String>,
        reason: shipper_types::ControlledStopReason,
        package: String,
    },
}

fn event_phase(events: &EventLog) -> EventPhase {
    let mut pending_plan_id = None;
    let mut phase = EventPhase::NoEvidence;
    for event in events.all_events() {
        match &event.event_type {
            EventType::PlanCreated { plan_id, .. } => match &mut phase {
                EventPhase::Unfinished {
                    plan_id: active_plan_id,
                } => *active_plan_id = Some(plan_id.clone()),
                _ => pending_plan_id = Some(plan_id.clone()),
            },
            EventType::ExecutionStarted => {
                phase = EventPhase::Unfinished {
                    plan_id: pending_plan_id.take(),
                }
            }
            EventType::ExecutionFinished { result } => {
                let plan_id = match &phase {
                    EventPhase::Unfinished { plan_id } => plan_id.clone(),
                    _ => pending_plan_id.take(),
                };
                phase = EventPhase::Finished {
                    plan_id,
                    result: result.clone(),
                };
            }
            EventType::ExecutionStopped { reason } => {
                let plan_id = match &phase {
                    EventPhase::Unfinished { plan_id } => plan_id.clone(),
                    _ => pending_plan_id.take(),
                };
                phase = EventPhase::Stopped {
                    plan_id,
                    reason: reason.clone(),
                    package: event.package.clone(),
                };
            }
            _ => {}
        }
    }
    phase
}

fn observe_lock(
    record: &LockRecord,
    event_plan_id: Option<&str>,
    hostname: &str,
    probe: &mut dyn ProcessProbe,
) -> RunLiveness {
    if record.info.hostname != hostname {
        return RunLiveness::Unknown(UnknownLivenessReason::CrossHost);
    }
    let (Some(lock_plan_id), Some(event_plan_id)) = (record.info.plan_id.as_deref(), event_plan_id)
    else {
        return RunLiveness::Unknown(UnknownLivenessReason::MissingPlanIdentity);
    };
    if lock_plan_id != event_plan_id {
        return RunLiveness::Unknown(UnknownLivenessReason::PlanMismatch);
    }
    let Some(expected_identity) = record.process_identity.as_ref() else {
        return RunLiveness::Unknown(UnknownLivenessReason::LegacyLock);
    };
    let Some(observer_scope) = probe.scope() else {
        return RunLiveness::Unknown(UnknownLivenessReason::ProcessProbeUnavailable);
    };
    if observer_scope.boot_id != expected_identity.boot_id
        || observer_scope.pid_namespace != expected_identity.pid_namespace
    {
        return RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch);
    }
    match probe.status(record.info.pid) {
        ProcessStatus::Running(actual_identity) if actual_identity == *expected_identity => {
            RunLiveness::Live
        }
        ProcessStatus::Running(_) => {
            RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch)
        }
        ProcessStatus::NotRunning => RunLiveness::NotLive,
        ProcessStatus::Unavailable => {
            RunLiveness::Unknown(UnknownLivenessReason::ProcessProbeUnavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{LockInfo, parse_process_start_ticks};
    use crate::types::PublishEvent;
    use anyhow::{Result, ensure};
    use chrono::{TimeZone, Utc};
    use std::fs;
    use tempfile::tempdir;

    struct FixedProbe {
        scope: Option<ProcessScope>,
        status: ProcessStatus,
    }
    impl ProcessProbe for FixedProbe {
        fn scope(&mut self) -> Option<ProcessScope> {
            self.scope.clone()
        }

        fn status(&mut self, _pid: u32) -> ProcessStatus {
            self.status.clone()
        }
    }

    fn time(seconds: i64) -> Result<chrono::DateTime<Utc>> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .ok_or_else(|| anyhow::anyhow!("valid fixture time"))
    }
    fn identity(start: u64) -> ProcessIdentity {
        ProcessIdentity {
            boot_id: "boot-a".into(),
            pid_namespace: "pid:[100]".into(),
            process_start_ticks: start,
        }
    }
    fn probe(status: ProcessStatus) -> FixedProbe {
        FixedProbe {
            scope: Some(ProcessScope {
                boot_id: "boot-a".into(),
                pid_namespace: "pid:[100]".into(),
            }),
            status,
        }
    }
    fn record(
        host: &str,
        plan: Option<&str>,
        process_identity: Option<ProcessIdentity>,
    ) -> Result<LockRecord> {
        Ok(LockRecord {
            info: LockInfo {
                pid: 42,
                hostname: host.into(),
                acquired_at: time(1_800_000_000)?,
                plan_id: plan.map(str::to_owned),
            },
            process_identity,
        })
    }
    fn write_lock(dir: &Path, record: &LockRecord) -> Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(lock_path(dir, None), serde_json::to_vec(record)?)?;
        Ok(())
    }
    fn write_events(dir: &Path, kinds: Vec<EventType>) -> Result<()> {
        let mut log = EventLog::new();
        for event_type in kinds {
            log.record(PublishEvent {
                timestamp: time(1_800_000_000)?,
                event_type,
                package: "workspace".into(),
            });
        }
        log.write_to_file(&events_path(dir))?;
        Ok(())
    }
    fn started(plan: &str) -> Vec<EventType> {
        vec![
            EventType::PlanCreated {
                plan_id: plan.into(),
                package_count: 1,
            },
            EventType::ExecutionStarted,
        ]
    }
    fn started_in_production_order(plan: &str) -> Vec<EventType> {
        vec![
            EventType::ExecutionStarted,
            EventType::PlanCreated {
                plan_id: plan.into(),
                package_count: 1,
            },
        ]
    }

    #[test]
    fn lock_json_is_backward_and_publicly_compatible() -> Result<()> {
        let old = record("a", Some("p"), None)?.info;
        let old_json = serde_json::to_string(&old)?;
        let parsed: LockRecord = serde_json::from_str(&old_json)?;
        ensure!(parsed.process_identity.is_none());
        let new = serde_json::to_string(&record("a", Some("p"), Some(identity(7)))?)?;
        let public: LockInfo = serde_json::from_str(&new)?;
        ensure!(public.pid == old.pid && new.contains("process_identity"));
        Ok(())
    }
    #[test]
    fn exact_local_identity_is_live_and_absence_is_not_live() -> Result<()> {
        let r = record("a", Some("p"), Some(identity(7)))?;
        ensure!(
            observe_lock(
                &r,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Live
        );
        ensure!(
            observe_lock(
                &record("a", None, Some(identity(7)))?,
                None,
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::MissingPlanIdentity)
        );
        ensure!(
            observe_lock(
                &record("a", Some("p"), Some(identity(7)))?,
                None,
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::MissingPlanIdentity)
        );
        ensure!(
            observe_lock(&r, Some("p"), "a", &mut probe(ProcessStatus::NotRunning))
                == RunLiveness::NotLive
        );
        Ok(())
    }
    #[test]
    fn inconclusive_lock_evidence_is_unknown() -> Result<()> {
        let r = record("a", Some("p"), Some(identity(7)))?;
        ensure!(
            observe_lock(
                &record("b", Some("p"), Some(identity(7)))?,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::CrossHost)
        );
        let mut other_boot = identity(7);
        other_boot.boot_id = "boot-b".into();
        ensure!(
            observe_lock(
                &r,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(other_boot))
            ) == RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch)
        );
        let mut other_namespace = identity(7);
        other_namespace.pid_namespace = "pid:[200]".into();
        ensure!(
            observe_lock(
                &r,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(other_namespace))
            ) == RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch)
        );
        ensure!(
            observe_lock(
                &record("a", Some("p"), None)?,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::LegacyLock)
        );
        ensure!(
            observe_lock(
                &r,
                Some("q"),
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::PlanMismatch)
        );
        ensure!(
            observe_lock(
                &r,
                Some("p"),
                "a",
                &mut probe(ProcessStatus::Running(identity(8)))
            ) == RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch)
        );
        ensure!(
            observe_lock(&r, Some("p"), "a", &mut probe(ProcessStatus::Unavailable))
                == RunLiveness::Unknown(UnknownLivenessReason::ProcessProbeUnavailable)
        );
        for foreign_scope in [
            ProcessScope {
                boot_id: "boot-b".into(),
                pid_namespace: "pid:[100]".into(),
            },
            ProcessScope {
                boot_id: "boot-a".into(),
                pid_namespace: "pid:[200]".into(),
            },
        ] {
            ensure!(
                observe_lock(
                    &r,
                    Some("p"),
                    "a",
                    &mut FixedProbe {
                        scope: Some(foreign_scope),
                        status: ProcessStatus::NotRunning,
                    }
                ) == RunLiveness::Unknown(UnknownLivenessReason::ProcessIdentityMismatch)
            );
        }
        Ok(())
    }
    #[test]
    fn proc_stat_parser_handles_hostile_command_and_fails_closed() -> Result<()> {
        let prefix = "42 (cargo ) worker (odd))";
        let fields = [
            "S", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
            "16", "17", "18", "4242",
        ];
        let stat = format!("{prefix} {}", fields.join(" "));
        ensure!(parse_process_start_ticks(&stat)? == 4242);
        ensure!(parse_process_start_ticks("42 no terminator").is_err());
        ensure!(parse_process_start_ticks("42 (short) S 1").is_err());
        ensure!(
            parse_process_start_ticks(
                "42 (bad) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 nope"
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn controlled_stop_with_missing_lock_is_explicitly_absent() -> Result<()> {
        let td = tempdir()?;
        let mut events = started("p");
        events.push(EventType::ExecutionStopped {
            reason: shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
        });
        write_events(td.path(), events)?;
        ensure!(matches!(
            observe_run_with(
                td.path(),
                None,
                "a",
                &mut probe(ProcessStatus::Unavailable)
            )?,
            RunObservation::Stopped {
                plan_id: Some(plan_id),
                reason: shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
                liveness: None,
                ..
            } if plan_id == "p"
        ));
        write_lock(td.path(), &record("a", Some("p"), Some(identity(7)))?)?;
        ensure!(matches!(
            observe_run_with(
                td.path(),
                None,
                "a",
                &mut probe(ProcessStatus::Running(identity(7)))
            )?,
            RunObservation::Stopped {
                liveness: Some(RunLiveness::Live),
                ..
            }
        ));
        fs::write(lock_path(td.path(), None), b"bad")?;
        ensure!(matches!(
            observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable))?,
            RunObservation::Stopped {
                liveness: Some(RunLiveness::Unknown(UnknownLivenessReason::CorruptLock)),
                ..
            }
        ));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn reserved_predecessor_observation_does_not_read_the_new_holder() -> Result<()> {
        let td = tempdir()?;
        let mut kinds = started("p");
        kinds.push(EventType::ExecutionStopped {
            reason: shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
        });
        write_events(td.path(), kinds)?;
        let current = crate::lock::LockFile::acquire(td.path(), None)?;
        current.set_plan_id("p")?;
        ensure!(matches!(
            observe_run(td.path(), None)?,
            RunObservation::Stopped {
                liveness: Some(_),
                ..
            }
        ));
        ensure!(matches!(
            observe_run_with_prior_lock(td.path(), &LockContents::Missing)?,
            RunObservation::Stopped { liveness: None, .. }
        ));
        Ok(())
    }
    #[test]
    fn lock_age_and_clock_skew_do_not_downgrade_exact_identity() -> Result<()> {
        let mut future = record("a", Some("p"), Some(identity(7)))?;
        future.info.acquired_at = time(2_000_000_000)?;
        for r in [&record("a", Some("p"), Some(identity(7)))?, &future] {
            ensure!(
                observe_lock(
                    r,
                    Some("p"),
                    "a",
                    &mut probe(ProcessStatus::Running(identity(7)))
                ) == RunLiveness::Live
            );
        }
        Ok(())
    }
    #[test]
    fn missing_corrupt_and_crash_fresh_lock_fail_closed() -> Result<()> {
        let td = tempdir()?;
        write_events(td.path(), started("p"))?;
        let missing =
            observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable))?;
        ensure!(matches!(
            missing,
            RunObservation::Unfinished {
                liveness: RunLiveness::Unknown(UnknownLivenessReason::MissingLock),
                ..
            }
        ));
        fs::write(lock_path(td.path(), None), b"bad")?;
        let corrupt =
            observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable))?;
        ensure!(matches!(
            corrupt,
            RunObservation::Unfinished {
                liveness: RunLiveness::Unknown(UnknownLivenessReason::CorruptLock),
                ..
            }
        ));
        write_lock(td.path(), &record("a", Some("p"), Some(identity(7)))?)?;
        let crash = observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable))?;
        ensure!(matches!(
            crash,
            RunObservation::Unfinished {
                liveness: RunLiveness::Unknown(UnknownLivenessReason::ProcessProbeUnavailable),
                ..
            }
        ));
        Ok(())
    }
    #[test]
    fn terminal_structurally_dominates_orphan_lock() -> Result<()> {
        let td = tempdir()?;
        let mut e = started("p");
        e.push(EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        });
        write_events(td.path(), e)?;
        fs::create_dir_all(td.path())?;
        fs::write(lock_path(td.path(), None), b"bad")?;
        let observed = observe_run_with(
            td.path(),
            None,
            "a",
            &mut probe(ProcessStatus::Running(identity(7))),
        )?;
        ensure!(matches!(
            observed,
            RunObservation::Finished {
                result: ExecutionResult::Success,
                ..
            }
        ));
        Ok(())
    }
    #[test]
    fn latest_run_segment_controls_phase_and_plan() -> Result<()> {
        let td = tempdir()?;
        let mut e = started("old");
        e.push(EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        });
        e.extend(started_in_production_order("new"));
        write_events(td.path(), e)?;
        write_lock(td.path(), &record("a", Some("new"), Some(identity(7)))?)?;
        let observed = observe_run_with(
            td.path(),
            None,
            "a",
            &mut probe(ProcessStatus::Running(identity(7))),
        )?;
        ensure!(
            matches!(observed,RunObservation::Unfinished{plan_id:Some(ref p),liveness:RunLiveness::Live} if p=="new")
        );
        Ok(())
    }
    #[test]
    fn production_order_binds_plan_to_first_active_segment() -> Result<()> {
        let td = tempdir()?;
        write_events(td.path(), started_in_production_order("plan-a"))?;
        write_lock(td.path(), &record("a", Some("plan-a"), Some(identity(7)))?)?;
        let observed = observe_run_with(
            td.path(),
            None,
            "a",
            &mut probe(ProcessStatus::Running(identity(7))),
        )?;
        ensure!(
            matches!(observed, RunObservation::Unfinished { plan_id: Some(ref plan_id), liveness: RunLiveness::Live } if plan_id == "plan-a")
        );
        Ok(())
    }
    #[test]
    fn missing_events_are_none_and_corrupt_events_error() -> Result<()> {
        let td = tempdir()?;
        ensure!(
            observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable))?
                == RunObservation::NoEvidence
        );
        fs::write(events_path(td.path()), b"bad\n")?;
        ensure!(
            observe_run_with(td.path(), None, "a", &mut probe(ProcessStatus::Unavailable)).is_err()
        );
        Ok(())
    }
}
