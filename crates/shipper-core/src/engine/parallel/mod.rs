//! Wave-based parallel publishing engine.
//!
//! Schedules independent crates into concurrent publish waves based on the
//! dependency graph produced by `shipper_plan::ReleasePlan::group_by_levels`.
//!
//! Absorbed from the standalone `shipper-engine-parallel` crate. See
//! `CLAUDE.md` alongside this module for module-level guidance.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::state::events;
use shipper_types::{ExecutionState, PackageReceipt, RuntimeOptions};

mod flow;
pub(crate) mod policy;
mod scheduler;
pub(crate) mod webhook;

/// Re-exported for parallel publish wave planning.
pub use crate::plan::chunking::chunk_by_max_concurrent;

pub(crate) use flow::parallel_resume_start_level;
use flow::{init_send_reporter, is_level_already_complete};
use scheduler::run_publish_level;
#[cfg(test)]
use webhook::{WebhookEvent, maybe_send_event};

// Keep the former test/import path stable while the package implementation
// moves out of the scheduler module. New production code should import the
// executor from `crate::engine::execute_package` and scheduling from
// `scheduler`.
#[cfg(test)]
pub(crate) mod publish {
    pub(crate) use super::scheduler::run_publish_level;
    pub(crate) use crate::engine::execute_package::{emit_retry_backoff, publish_package};
}

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

struct RetryWaitNotice {
    pkg_name: String,
    pkg_version: String,
    attempt: u32,
    max_attempts: u32,
    delay: Duration,
    reason: shipper_types::ErrorClass,
    message: String,
    started_at: Instant,
}

enum ReporterMessage {
    Info(String),
    Warn(String),
    Error(String),
    RetryWait(RetryWaitNotice),
}

#[derive(Default)]
pub(crate) struct SendReporter {
    messages: Mutex<VecDeque<ReporterMessage>>,
    available: Condvar,
}

// The `Reporter` trait returns `()`, not `Result`, so poison cannot
// propagate as a typed error. A poisoned reporter mutex means a publish
// thread panicked mid-push; recovering the guard (possibly with a partial
// buffer) and continuing is safer than panicking the orchestrator, which
// would lose in-flight publish state. The authoritative record is
// `events.jsonl`, not this in-memory buffer. See `flow.rs` for the typed
// sites that CAN propagate poison as an error.
impl SendReporter {
    fn push(&self, message: ReporterMessage) {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(message);
        self.available.notify_one();
    }

    pub(crate) fn info(&self, msg: &str) {
        self.push(ReporterMessage::Info(msg.to_string()));
    }

    pub(crate) fn warn(&self, msg: &str) {
        self.push(ReporterMessage::Warn(msg.to_string()));
    }

    pub(crate) fn error(&self, msg: &str) {
        self.push(ReporterMessage::Error(msg.to_string()));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn retry_wait(
        &self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: shipper_types::ErrorClass,
        message: &str,
    ) {
        let started_at = Instant::now();
        self.push(ReporterMessage::RetryWait(RetryWaitNotice {
            pkg_name: pkg_name.to_string(),
            pkg_version: pkg_version.to_string(),
            attempt,
            max_attempts,
            delay,
            reason,
            message: message.to_string(),
            started_at,
        }));
        // Reporting and execution observe the same monotonic wait. The host
        // may render a countdown, return immediately, or be slow; none of
        // those choices owns or restarts the worker's retry delay.
        thread::sleep(delay.saturating_sub(started_at.elapsed()));
    }

    fn drain_messages(&self) -> VecDeque<ReporterMessage> {
        std::mem::take(&mut *self.messages.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(crate) fn wait_for_messages(&self) {
        let messages = self.messages.lock().unwrap_or_else(|e| e.into_inner());
        // The bound also lets schedulers observe worker completion or panic
        // when there is no final message. No consumer callback holds this lock.
        drop(
            self.available
                .wait_timeout_while(messages, Duration::from_millis(25), |queue| {
                    queue.is_empty()
                })
                .unwrap_or_else(|e| e.into_inner()),
        );
    }
}

pub(crate) fn replay_buffered_messages(reporter: &mut dyn Reporter, send_reporter: &SendReporter) {
    // Drain under one lock to preserve mixed-severity and retry-notice order.
    // Release the lock before invoking potentially slow embedding callbacks.
    for message in send_reporter.drain_messages() {
        match message {
            ReporterMessage::Info(message) => reporter.info(&message),
            ReporterMessage::Warn(message) => reporter.warn(&message),
            ReporterMessage::Error(message) => reporter.error(&message),
            ReporterMessage::RetryWait(notice) => {
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
    }
}

pub(crate) fn replay_buffered_messages_to_host(
    reporter: &mut dyn crate::engine::Reporter,
    send_reporter: &SendReporter,
) {
    let mut adapter = HostReporterAdapter { inner: reporter };
    replay_buffered_messages(&mut adapter, send_reporter);
}

/// Run publish in parallel mode using `shipper`'s wrapped `RegistryClient`.
///
/// This lower-level entry point owns its invocation lock and validates retained
/// evidence before notifications or workers. It reloads durable state under
/// that ownership; supplied memory can initialize only a fresh, unattempted run.
/// A caller already holding this invocation key is deliberately refused, even
/// in the same process. Use `engine::run_publish` / `engine::run_resume` for the
/// full execution lifecycle and receipt, or call this function without a
/// separately held lock. The full orchestrator uses the private adapter below.
/// Each admitted call records a new execution segment and completes coherent
/// controlled-stop evidence on a recoverable failure. Success returns package
/// receipts without writing a full receipt or an `ExecutionFinished` marker.
///
/// Preserves the preconfigured registry client (including cache directory and
/// transport configuration) while adapting host reporters for parallel
/// execution.
pub fn run_publish_parallel(
    ws: &crate::plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    reporter: &mut dyn crate::engine::Reporter,
) -> Result<Vec<PackageReceipt>> {
    crate::ops::process::interruption::check()?;
    crate::config::validate_per_error_config(&opts.retry_per_error)?;
    crate::engine::publish::admission::ensure_no_cleanup_hold(st)?;
    // Direct calls always schedule complete dependency levels, independently
    // of the flag used by the full orchestrator to select its scheduler.
    let mut admission_opts = opts.clone();
    admission_opts.parallel.enabled = true;
    let require_existing =
        crate::engine::publish::admission::precheck(ws, &admission_opts, state_dir, false)?;
    if !require_existing {
        crate::engine::publish::admission::validate_fresh_parallel_state(ws, st)
            .context(crate::engine::PublishAdmissionError)?;
    }
    let (reservation, retained) = crate::engine::publish::admission::reserve_and_admit(
        ws,
        &admission_opts,
        state_dir,
        require_existing,
    )?;
    let fresh = retained.is_none();
    let admitted = retained.unwrap_or_else(|| st.clone());
    let _lock = reservation
        .claim(Some(&ws.plan.plan_id))
        .context(crate::engine::PublishAdmissionError)?;
    if fresh {
        crate::state::execution_state::save_state(state_dir, &admitted)?;
    }
    *st = admitted;
    // This lower-level API owns start/controlled-stop evidence, but leaves
    // successful receipt finalization to the full publish/resume orchestrator.
    let events_path = events::events_path(state_dir);
    let mut event_log = events::EventLog::new();
    crate::engine::publish::bootstrap::record_execution_start(
        ws,
        opts,
        &events_path,
        &mut event_log,
        chrono::Utc::now(),
        &crate::ops::auth::collect_auth_evidence(&ws.plan.registry.name),
        &reg.policy_evidence(),
    )?;
    crate::engine::publish::notify_publish_started(ws, opts);
    match run_publish_parallel_without_start(ws, opts, st, state_dir, reg, reporter) {
        Ok(receipts) => Ok(receipts),
        Err(error) => Err(crate::engine::publish::finalize::record_recoverable_stop(
            error,
            state_dir,
            &events_path,
            &mut event_log,
            st,
            reporter,
        )?),
    }
}

/// Private adapter for orchestration that already owns the invocation lock,
/// admitted freshly loaded evidence, and emitted the run-start notification.
pub(crate) fn run_publish_parallel_without_start(
    ws: &crate::plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    reporter: &mut dyn crate::engine::Reporter,
) -> Result<Vec<PackageReceipt>> {
    let mut adapter = HostReporterAdapter { inner: reporter };
    run_publish_parallel_inner(ws, opts, st, state_dir, reg, &mut adapter)
}

/// Inner entry point operating on `crate::registry::RegistryClient` and the
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

    // Initialize event log
    let events_path = events::events_path(state_dir);
    let event_log = Arc::new(Mutex::new(events::EventLog::new()));

    // Wrap state and reporter in Arc<Mutex<>> for thread safety
    let st_arc = Arc::new(Mutex::new(st.clone()));

    let send_reporter = Arc::new(init_send_reporter());

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();

    let resume_start_level = parallel_resume_start_level(&levels, opts.resume_from.as_deref());

    for (level_index, level) in levels.iter().enumerate() {
        let before_resume_level = resume_start_level.is_none_or(|start| level_index < start);
        if before_resume_level {
            if is_level_already_complete(&level.packages, &st_arc)? {
                reporter.info(&format!(
                    "Level {}: already complete (skipping)",
                    level.level
                ));
            } else {
                reporter.warn(&format!(
                    "Level {}: skipping (before resume point {})",
                    level.level,
                    opts.resume_from.as_deref().unwrap_or("<none>")
                ));
            }
            record_terminal_resume_skips(&level.packages, &st_arc, &event_log, &events_path)?;
            continue;
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
        );
        if let Err(err) = level_receipts {
            replay_buffered_messages(reporter, send_reporter.as_ref());
            return Err(synchronize_parallel_failure(st, &st_arc, err));
        }
        let level_receipts = level_receipts?;
        all_receipts.extend(level_receipts);
        replay_buffered_messages(reporter, send_reporter.as_ref());
    }

    replay_buffered_messages(reporter, send_reporter.as_ref());

    // Copy updated state back
    let updated_st = st_arc
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned while copying final state"))?;
    *st = updated_st.clone();

    Ok(all_receipts)
}

fn synchronize_parallel_state(
    state: &mut ExecutionState,
    state_arc: &Arc<Mutex<ExecutionState>>,
) -> anyhow::Result<()> {
    let updated_state = state_arc
        .lock()
        .map_err(|_| {
            anyhow::anyhow!("execution state lock poisoned while synchronizing parallel state")
        })?
        .clone();
    *state = updated_state;
    Ok(())
}

fn synchronize_parallel_failure(
    state: &mut ExecutionState,
    state_arc: &Arc<Mutex<ExecutionState>>,
    error: anyhow::Error,
) -> anyhow::Error {
    match synchronize_parallel_state(state, state_arc) {
        Ok(()) => error,
        Err(sync_error) => error.context(format!(
            "additionally failed to synchronize parallel state: {sync_error}"
        )),
    }
}

fn record_terminal_resume_skips(
    level_packages: &[shipper_types::PlannedPackage],
    st_arc: &Arc<Mutex<ExecutionState>>,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
) -> Result<()> {
    let terminal_packages = {
        let state = st_arc.lock().map_err(|_| {
            anyhow::anyhow!("execution state lock poisoned while recording resume skips")
        })?;

        level_packages
            .iter()
            .filter_map(|package| {
                let key = crate::runtime::execution::pkg_key(&package.name, &package.version);
                let progress = state.packages.get(&key)?;
                {
                    matches!(
                        progress.state,
                        shipper_types::PackageState::Published
                            | shipper_types::PackageState::Skipped { .. }
                    )
                    .then(|| (progress.clone(), key))
                }
            })
            .collect::<Vec<_>>()
    };

    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned while recording resume skips"))?;
    for (progress, package_label) in terminal_packages {
        crate::engine::publish::resume::record_terminal_resume_skip_event(
            &progress,
            &package_label,
            events_path,
            &mut log,
        )?;
    }
    Ok(())
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
