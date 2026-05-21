//! Wave-based parallel publishing engine.
//!
//! Schedules independent crates into concurrent publish waves based on the
//! dependency graph produced by `shipper_plan::ReleasePlan::group_by_levels`.
//!
//! Absorbed from the standalone `shipper-engine-parallel` crate. See
//! `CLAUDE.md` alongside this module for module-level guidance.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::plan::PlannedWorkspace;
use crate::state::events;
use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{ExecutionState, PackageReceipt, RuntimeOptions};

mod flow;
mod policy;
mod publish;
mod readiness;
mod reconcile;
mod webhook;

/// Re-exported for parallel publish wave planning.
pub use crate::plan::chunking::chunk_by_max_concurrent;

use flow::{
    LevelResumeAction, collect_level_receipts_from_state, determine_level_resume_action,
    init_send_reporter,
};
use publish::run_publish_level;
use webhook::WebhookEvent;
#[cfg(test)]
use webhook::maybe_send_event;

/// Reporter interface shared with the host crate. Parallel publish forwards
/// status updates and warnings through this trait.
pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);

    #[allow(clippy::too_many_arguments)]
    fn retry_wait(
        &mut self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: shipper_types::ErrorClass,
        message: &str,
    ) {
        self.warn(&format!(
            "{}@{}: {} ({:?}); next attempt in {} (attempt {}/{})",
            pkg_name,
            pkg_version,
            message,
            reason,
            humantime::format_duration(delay),
            attempt.saturating_add(1),
            max_attempts,
        ));
        thread::sleep(delay);
    }
}

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

    fn retry_wait(
        &mut self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: shipper_types::ErrorClass,
        message: &str,
    ) {
        self.inner.retry_wait(
            pkg_name,
            pkg_version,
            attempt,
            max_attempts,
            delay,
            reason,
            message,
        );
    }
}

pub(super) struct RetryWaitNotice {
    pub(super) pkg_name: String,
    pub(super) pkg_version: String,
    pub(super) attempt: u32,
    pub(super) max_attempts: u32,
    pub(super) delay: Duration,
    pub(super) reason: shipper_types::ErrorClass,
    pub(super) message: String,
    pub(super) started_at: Instant,
}

#[derive(Default)]
pub(super) struct SendReporter {
    infos: Mutex<Vec<String>>,
    warns: Mutex<Vec<String>>,
    errors: Mutex<Vec<String>>,
    retry_waits: Mutex<VecDeque<RetryWaitNotice>>,
}

impl SendReporter {
    pub(super) fn info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }

    pub(super) fn warn(&self, msg: &str) {
        self.warns.lock().unwrap().push(msg.to_string());
    }

    pub(super) fn error(&self, msg: &str) {
        self.errors.lock().unwrap().push(msg.to_string());
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn retry_wait(
        &self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: shipper_types::ErrorClass,
        message: &str,
    ) {
        self.retry_waits.lock().unwrap().push_back(RetryWaitNotice {
            pkg_name: pkg_name.to_string(),
            pkg_version: pkg_version.to_string(),
            attempt,
            max_attempts,
            delay,
            reason,
            message: message.to_string(),
            started_at: Instant::now(),
        });
        thread::sleep(delay);
    }

    fn drain_infos(&self) -> Vec<String> {
        std::mem::take(&mut *self.infos.lock().unwrap())
    }

    fn drain_warns(&self) -> Vec<String> {
        std::mem::take(&mut *self.warns.lock().unwrap())
    }

    fn drain_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }

    fn drain_retry_waits(&self) -> Vec<RetryWaitNotice> {
        self.retry_waits.lock().unwrap().drain(..).collect()
    }
}

fn replay_buffered_messages(reporter: &mut dyn Reporter, send_reporter: &SendReporter) {
    for msg in send_reporter.drain_infos() {
        reporter.info(&msg);
    }
    for msg in send_reporter.drain_warns() {
        reporter.warn(&msg);
    }
    for msg in send_reporter.drain_errors() {
        reporter.error(&msg);
    }
}

pub(super) fn drain_retry_waits(reporter: &mut dyn Reporter, send_reporter: &SendReporter) {
    for notice in send_reporter.drain_retry_waits() {
        let remaining = notice.delay.saturating_sub(notice.started_at.elapsed());
        reporter.retry_wait(
            &notice.pkg_name,
            &notice.pkg_version,
            notice.attempt,
            notice.max_attempts,
            remaining,
            notice.reason,
            &notice.message,
        );
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
    let reg_inner = shipper_registry::HttpRegistryClient::new(api_base);
    let mut adapter = HostReporterAdapter { inner: reporter };
    run_publish_parallel_inner(ws, opts, st, state_dir, &reg_inner, &mut adapter)
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

    let send_reporter = Arc::new(init_send_reporter());

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();

    // Track if we've reached the resume point if one was specified
    let mut reached_resume_point = opts.resume_from.is_none();

    for level in &levels {
        // If we haven't reached the resume point, check if it's in this level
        if !reached_resume_point {
            match determine_level_resume_action(
                &level.packages,
                &st_arc,
                opts.resume_from.as_deref(),
            ) {
                LevelResumeAction::ReachedResumePoint => reached_resume_point = true,
                LevelResumeAction::SkipAlreadyComplete => {
                    reporter.info(&format!(
                        "Level {}: already complete (skipping)",
                        level.level
                    ));
                    all_receipts
                        .extend(collect_level_receipts_from_state(&level.packages, &st_arc));
                    continue;
                }
                LevelResumeAction::SkipBeforeResumePoint(resume_point) => {
                    reporter.warn(&format!(
                        "Level {}: skipping (before resume point {})",
                        level.level, resume_point
                    ));
                    all_receipts
                        .extend(collect_level_receipts_from_state(&level.packages, &st_arc));
                    continue;
                }
            };
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
            reporter,
            &send_reporter,
        )?;
        all_receipts.extend(level_receipts);
        replay_buffered_messages(reporter, send_reporter.as_ref());
    }

    replay_buffered_messages(reporter, send_reporter.as_ref());

    // Copy updated state back
    let updated_st = st_arc.lock().unwrap();
    *st = updated_st.clone();

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
