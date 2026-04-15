//! Wave-based parallel publishing engine.
//!
//! Schedules independent crates into concurrent publish waves based on the
//! dependency graph produced by `shipper_plan::ReleasePlan::group_by_levels`.
//!
//! Absorbed from the standalone `shipper-engine-parallel` crate. See
//! `CLAUDE.md` alongside this module for module-level guidance.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use shipper_events as events;
use shipper_plan::PlannedWorkspace;
use shipper_registry::RegistryClient;
use shipper_types::{
    ExecutionResult, ExecutionState, PackageEvidence, PackageReceipt, PackageState, RuntimeOptions,
};

mod policy;
mod publish;
mod readiness;
mod webhook;

use publish::run_publish_level;
use webhook::{WebhookEvent, maybe_send_event};

/// Reporter interface shared with the host crate. Parallel publish forwards
/// status updates and warnings through this trait.
pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

/// Re-exported from the chunking microcrate for parallel publish wave planning.
pub use shipper_chunking::chunk_by_max_concurrent;

/// Adapter that bridges the host crate's `crate::engine::Reporter` trait into
/// this module's local `Reporter` trait. Allows callers inside `shipper` to
/// pass their existing reporters without any wrapping at the call site.
struct HostReporterAdapter<'a> {
    inner: &'a mut dyn crate::engine::Reporter,
}

impl<'a> Reporter for HostReporterAdapter<'a> {
    fn info(&mut self, msg: &str) {
        self.inner.info(msg);
    }
    fn warn(&mut self, msg: &str) {
        self.inner.warn(msg);
    }
    fn error(&mut self, msg: &str) {
        self.inner.error(msg);
    }
}

/// Run publish in parallel mode using `shipper`'s wrapped `RegistryClient`.
///
/// This is the entry point called by `engine::run_publish`. It adapts the
/// host crate's types (`crate::registry::RegistryClient`, `crate::engine::Reporter`)
/// into the inner ones expected by the parallel engine.
///
/// Constructs a fresh `shipper_registry::RegistryClient` from the host
/// registry's configuration so the call works regardless of which `registry`
/// impl variant is active (micro wrapper vs. in-tree legacy).
pub fn run_publish_parallel(
    ws: &crate::plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &crate::registry::RegistryClient,
    reporter: &mut dyn crate::engine::Reporter,
) -> Result<Vec<PackageReceipt>> {
    let api_base = reg.registry().api_base.trim_end_matches('/');
    let reg_inner = shipper_registry::RegistryClient::new(api_base);
    let ws_inner = shipper_plan::PlannedWorkspace {
        workspace_root: ws.workspace_root.clone(),
        plan: ws.plan.clone(),
        skipped: ws
            .skipped
            .iter()
            .map(|s| shipper_plan::SkippedPackage {
                name: s.name.clone(),
                version: s.version.clone(),
                reason: s.reason.clone(),
            })
            .collect(),
    };
    let mut adapter = HostReporterAdapter { inner: reporter };
    run_publish_parallel_inner(&ws_inner, opts, st, state_dir, &reg_inner, &mut adapter)
}

/// Inner entry point operating on `shipper_registry::RegistryClient` and the
/// local `Reporter` trait. Kept `pub` for tests inside this module.
pub(crate) fn run_publish_parallel_inner(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    reporter: &mut dyn Reporter,
) -> Result<Vec<PackageReceipt>> {
    let levels = ws.plan.group_by_levels();

    reporter.info(&format!(
        "parallel publish: {} levels, {} packages total",
        levels.len(),
        ws.plan.packages.len()
    ));

    // Send webhook notification: publish started
    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishStarted {
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
            registry: ws.plan.registry.name.clone(),
        },
    );

    // Initialize event log
    let events_path = events::events_path(state_dir);
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));

    // Wrap state and reporter in Arc<Mutex<>> for thread safety
    let st_arc = Arc::new(Mutex::new(st.clone()));

    // Create a thread-safe reporter wrapper
    struct SendReporter {
        infos: Mutex<Vec<String>>,
        warns: Mutex<Vec<String>>,
        errors: Mutex<Vec<String>>,
    }
    impl Reporter for SendReporter {
        fn info(&mut self, msg: &str) {
            self.infos.lock().unwrap().push(msg.to_string());
        }
        fn warn(&mut self, msg: &str) {
            self.warns.lock().unwrap().push(msg.to_string());
        }
        fn error(&mut self, msg: &str) {
            self.errors.lock().unwrap().push(msg.to_string());
        }
    }

    let send_reporter = Arc::new(Mutex::new(SendReporter {
        infos: Mutex::new(Vec::new()),
        warns: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
    }));

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();

    // Track if we've reached the resume point if one was specified
    let mut reached_resume_point = opts.resume_from.is_none();

    for level in &levels {
        // If we haven't reached the resume point, check if it's in this level
        if !reached_resume_point {
            if level
                .packages
                .iter()
                .any(|p| Some(&p.name) == opts.resume_from.as_ref())
            {
                reached_resume_point = true;
            } else {
                // Check if all packages in this level are already done in state
                // If so, we can "skip" it silently (as already done).
                // If NOT done, we skip it with a warning because of resume_from.
                let mut level_done = true;
                {
                    let st_guard = st_arc.lock().unwrap();
                    for p in &level.packages {
                        let key = shipper_execution_core::pkg_key(&p.name, &p.version);
                        if let Some(progress) = st_guard.packages.get(&key) {
                            if !matches!(
                                progress.state,
                                PackageState::Published | PackageState::Skipped { .. }
                            ) {
                                level_done = false;
                                break;
                            }
                        } else {
                            level_done = false;
                            break;
                        }
                    }
                }

                if level_done {
                    reporter.info(&format!(
                        "Level {}: already complete (skipping)",
                        level.level
                    ));
                } else {
                    reporter.warn(&format!(
                        "Level {}: skipping (before resume point {})",
                        level.level,
                        opts.resume_from.as_ref().unwrap()
                    ));
                }

                // Still need to "collect" receipts for these skipped packages so they appear in final receipt
                for p in &level.packages {
                    let key = shipper_execution_core::pkg_key(&p.name, &p.version);
                    let st_guard = st_arc.lock().unwrap();
                    if let Some(progress) = st_guard.packages.get(&key) {
                        all_receipts.push(PackageReceipt {
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
                        });
                    }
                }
                continue;
            }
        }

        let level_receipts = run_publish_level(
            level,
            ws,
            opts,
            reg,
            &st_arc,
            state_dir,
            &event_log,
            &events_path,
            &(send_reporter.clone() as Arc<Mutex<dyn Reporter + Send>>),
        )?;
        all_receipts.extend(level_receipts);
    }

    // Replay messages to the real reporter
    {
        let sr = send_reporter.lock().unwrap();
        for msg in sr.infos.lock().unwrap().iter() {
            reporter.info(msg);
        }
        for msg in sr.warns.lock().unwrap().iter() {
            reporter.warn(msg);
        }
        for msg in sr.errors.lock().unwrap().iter() {
            reporter.error(msg);
        }
    }

    // Copy updated state back
    let updated_st = st_arc.lock().unwrap();
    *st = updated_st.clone();

    // Calculate publish completion statistics
    let total_packages = all_receipts.len();
    let success_count = all_receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Published))
        .count();
    let failure_count = all_receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Failed { .. }))
        .count();
    let skipped_count = all_receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Skipped { .. }))
        .count();

    let exec_result = if all_receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    }) {
        ExecutionResult::Success
    } else if success_count == 0 {
        ExecutionResult::CompleteFailure
    } else {
        ExecutionResult::PartialFailure
    };

    // Send webhook notification: all complete
    maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishCompleted {
            plan_id: ws.plan.plan_id.clone(),
            total_packages,
            success_count,
            failure_count,
            skipped_count,
            result: match exec_result {
                ExecutionResult::Success => "success".to_string(),
                ExecutionResult::PartialFailure => "partial_failure".to_string(),
                ExecutionResult::CompleteFailure => "complete_failure".to_string(),
            },
        },
    );

    Ok(all_receipts)
}

#[cfg(test)]
mod property_tests {
    use proptest::prelude::*;

    use super::chunk_by_max_concurrent;

    fn names() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec("[a-z]{1,8}", 0..64)
    }

    proptest! {
        #[test]
        fn chunking_preserves_order_and_limits_size(items in names(), limit in 0usize..64) {
            let chunks = chunk_by_max_concurrent(&items, limit);
            let flattened: Vec<String> = chunks.iter().flatten().cloned().collect();

            prop_assert_eq!(flattened.as_slice(), items.as_slice());

            let max_size = limit.max(1);
            for chunk in &chunks {
                prop_assert!(chunk.len() <= max_size);
            }

            if !flattened.is_empty() {
                if max_size == 1 {
                    prop_assert!(chunks.iter().all(|chunk| chunk.len() <= 1));
                } else {
                    prop_assert!(chunks.iter().all(|chunk| !chunk.is_empty() && chunk.len() <= max_size));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
