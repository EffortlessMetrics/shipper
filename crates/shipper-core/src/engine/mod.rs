use std::collections::BTreeMap;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::cargo;
use crate::ops::auth;
use crate::plan::PlannedWorkspace;
use crate::registry::{RegistryClient, RegistryPolicy, ValidatedRegistry};
#[cfg(test)]
use crate::runtime::environment;
#[cfg(test)]
use crate::runtime::execution::backoff_delay;
#[cfg(test)]
use crate::runtime::execution::short_state;
#[cfg(test)]
#[expect(
    deprecated,
    reason = "legacy compatibility coverage for quarantined state-only helper"
)]
use crate::runtime::execution::update_state;
use crate::runtime::execution::{classify_cargo_failure, pkg_key, resolve_state_dir};
use crate::state::events;
use crate::state::execution_state as state;
#[cfg(test)]
use crate::types::ExecutionResult;
use crate::types::{
    ErrorClass, EventType, ExecutionState, PackageProgress, PackageState, PreflightReport,
    PublishEvent, Receipt, Registry, RegistryTrustOptions, RuntimeOptions,
};
#[cfg(test)]
use crate::types::{Finishability, PreflightPackage};

#[doc(hidden)]
pub use crate::ops::process::interruption::PublishInterruption;

mod cleanup;
pub(crate) mod execute_package;
mod preflight;
mod publish;
mod reconcile;
mod rehearsal;
#[cfg(test)]
mod test_readiness;
pub(crate) mod transition;

#[cfg(test)]
use crate::types::{
    AttemptEvidence, PackageReceipt, PublishRegime, ReadinessEvidence, ReconciliationOutcome,
};
pub use preflight::PreflightRunOptions;
pub(crate) use publish::bootstrap::validate_resume_target;
#[cfg(test)]
use test_readiness::verify_published;

/// Typed identity for a publish attempt whose registry reconciliation remained
/// inconclusive after durable evidence was written.
///
/// This is public only so the separate CLI adapter can preserve the safe-stop
/// posture without parsing error prose. It is not a completed receipt.
#[doc(hidden)]
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct PublishStillUnknownError {
    message: String,
    reconciliation_written: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct PublishRecoverableStopError {
    message: String,
    package: String,
    evidence_consistent: bool,
}

/// A local cleanup hold must survive adapter error classification and cannot
/// be converted into registry reconciliation or a retry-ceiling suggestion.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct PublishCleanupIncompleteError {
    message: String,
}

/// Inspect retained cleanup safety before adapter convenience checks. The
/// engine repeats this check through its owned shared admission boundary.
#[doc(hidden)]
pub fn validate_publish_cleanup_safety(state_dir: &Path) -> Result<()> {
    publish::admission::validate_cleanup_safety(state_dir).context(PublishAdmissionError)
}

/// Typed context for an invocation refused before publish execution is admitted.
#[derive(Debug, thiserror::Error)]
#[error("publish admission was refused")]
pub(crate) struct PublishAdmissionError;

/// Preserve admission identity across the separate CLI adapter without parsing prose.
#[doc(hidden)]
pub fn is_publish_admission_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PublishAdmissionError>().is_some()
}

#[derive(Debug)]
struct ResumeRetryBudgetError {
    package: String,
    current_attempts: u32,
    requested_max_attempts: u32,
    minimum_max_attempts: Option<u32>,
}

impl std::fmt::Display for ResumeRetryBudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "resume retry ceiling exhausted for {}: persisted attempts {}, requested --max-attempts {}",
            self.package, self.current_attempts, self.requested_max_attempts
        )?;
        match self.minimum_max_attempts {
            Some(minimum) => write!(
                formatter,
                "; use --max-attempts {minimum} or greater to permit another attempt"
            ),
            None => formatter.write_str(
                "; persisted attempts cannot be increased within the supported retry-ceiling range",
            ),
        }
    }
}

impl std::error::Error for ResumeRetryBudgetError {}

/// CLI-facing identity for a resume rejected before mutation because its
/// cumulative per-package retry ceiling cannot permit another attempt.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeRetryBudgetClassification {
    pub package: String,
    pub current_attempts: u32,
    pub requested_max_attempts: u32,
    pub minimum_max_attempts: Option<u32>,
}

#[doc(hidden)]
pub fn classify_resume_retry_budget(
    error: &anyhow::Error,
) -> Option<ResumeRetryBudgetClassification> {
    error
        .downcast_ref::<ResumeRetryBudgetError>()
        .map(|budget| ResumeRetryBudgetClassification {
            package: budget.package.clone(),
            current_attempts: budget.current_attempts,
            requested_max_attempts: budget.requested_max_attempts,
            minimum_max_attempts: budget.minimum_max_attempts,
        })
}

/// CLI-facing classification for publish stops whose safety posture must not
/// be inferred from human-readable error text.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishStopClassification {
    CleanupIncomplete,
    Interrupted,
    StillUnknown { reconciliation_written: bool },
    RecoverableNotPublished { evidence_consistent: bool },
}

#[doc(hidden)]
pub fn classify_publish_stop(error: &anyhow::Error) -> Option<PublishStopClassification> {
    if error
        .downcast_ref::<PublishCleanupIncompleteError>()
        .is_some()
    {
        return Some(PublishStopClassification::CleanupIncomplete);
    }
    if error
        .downcast_ref::<crate::ops::process::interruption::Interrupted>()
        .is_some()
    {
        return Some(PublishStopClassification::Interrupted);
    }
    if let Some(still_unknown) = error.downcast_ref::<PublishStillUnknownError>() {
        return Some(PublishStopClassification::StillUnknown {
            reconciliation_written: still_unknown.reconciliation_written,
        });
    }
    error
        .downcast_ref::<PublishRecoverableStopError>()
        .map(
            |recoverable| PublishStopClassification::RecoverableNotPublished {
                evidence_consistent: recoverable.evidence_consistent,
            },
        )
}

fn controlled_stop_evidence_consistent(
    state_dir: &Path,
    events_path: &Path,
    state: &ExecutionState,
) -> bool {
    let reconciliation_path = state::reconciliation_path(state_dir);
    let Ok(raw) = std::fs::read_to_string(reconciliation_path) else {
        return false;
    };
    let Ok(report) = serde_json::from_str::<shipper_types::ReconciliationReport>(&raw) else {
        return false;
    };
    crate::state::consistency::verify_controlled_stop_consistency(events_path, state, Some(&report))
        .is_ok()
}

impl PublishStillUnknownError {
    #[doc(hidden)]
    pub fn reconciliation_written(&self) -> bool {
        self.reconciliation_written
    }
}

pub trait Reporter {
    fn info(&mut self, msg: &str);
    fn warn(&mut self, msg: &str);
    fn error(&mut self, msg: &str);

    /// Narrate the remaining retry-backoff wait after the
    /// [`EventType::RetryBackoffStarted`] event has been recorded. Both
    /// schedulers forward this callback on the host thread while the package
    /// worker owns the actual retry sleep. `delay` is the time remaining on
    /// that same monotonic wait when the callback is delivered.
    ///
    /// The default implementation warns once and sleeps for `delay`; richer
    /// UIs may render a countdown or return immediately. Callback duration
    /// does not authorize, extend, or restart the worker's retry delay. A slow
    /// callback can delay later narration while the worker retains control of
    /// retry admission and durable transitions.
    #[allow(clippy::too_many_arguments)]
    fn retry_wait(
        &mut self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: ErrorClass,
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

pub(crate) fn policy_effects(opts: &RuntimeOptions) -> crate::runtime::policy::PolicyEffects {
    crate::runtime::policy::policy_effects(opts)
}

pub(in crate::engine) fn require_strict_ownership_token(
    effects: &crate::runtime::policy::PolicyEffects,
    token: Option<&str>,
) -> Result<()> {
    if effects.strict_ownership && token.is_none_or(str::is_empty) {
        bail!(
            "strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)"
        );
    }
    Ok(())
}

pub(in crate::engine) fn enforce_strict_ownership_for_registry(
    registry_name: &str,
    effects: &crate::runtime::policy::PolicyEffects,
) -> Result<()> {
    if !effects.strict_ownership {
        return Ok(());
    }
    let token = auth::resolve_token(registry_name)?;
    require_strict_ownership_token(effects, token.as_deref())
}

/// Validate strict-ownership token presence for every registry selected for a
/// publish command before any registry-specific execution begins.
///
/// This is exposed for the CLI adapter's atomic multi-registry boundary. The
/// publish bootstrap invokes it again defensively before engine side effects.
#[doc(hidden)]
pub fn prevalidate_publish_registry_tokens(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<()> {
    let effects = policy_effects(opts);
    if opts.registries.is_empty() {
        return enforce_strict_ownership_for_registry(&ws.plan.registry.name, &effects);
    }
    for registry in &opts.registries {
        enforce_strict_ownership_for_registry(&registry.name, &effects)?;
    }
    Ok(())
}

fn init_registry_client(
    registry: Registry,
    state_dir: &Path,
    opts: &RuntimeOptions,
) -> Result<RegistryClient> {
    let cache_dir = state_dir.join("cache");
    let policy = registry_policy(&registry, opts);
    RegistryClient::with_policy(registry, policy).map(|c| c.with_cache_dir(cache_dir))
}

fn registry_policy(registry: &Registry, opts: &RuntimeOptions) -> RegistryPolicy {
    let allow_private = opts
        .registry_policies
        .get(&registry.name)
        .is_some_and(|policy| policy.allow_private);
    let allow_loopback = opts
        .registry_policies
        .get(&registry.name)
        .is_some_and(|policy| policy.allow_loopback);
    RegistryPolicy::secure()
        .with_private(allow_private)
        .with_loopback(allow_loopback)
}

fn validated_registry_identity(
    registry: Registry,
    opts: &RuntimeOptions,
) -> Result<ValidatedRegistry> {
    let policy = registry_policy(&registry, opts);
    ValidatedRegistry::new(registry, policy)
}

#[cfg(test)]
fn test_registry_client(mut registry: Registry) -> Result<RegistryClient> {
    if registry.index_base.is_none() {
        registry.index_base = Some(registry.api_base.clone());
    }
    RegistryClient::with_policy(registry, RegistryPolicy::rehearsal())
}

/// Run preflight verification checks before publishing.
///
/// This is the backward-compatible read-only entry point for embedders.
/// Call [`run_preflight_in_place`] if you want preflight to stamp the detected
/// [`crate::types::PublishRegime`] onto each `PlannedPackage` for a subsequent
/// publish in the same process.
///
/// This function performs various pre-publish checks to catch issues early:
/// - Git cleanliness (if `allow_dirty` is false)
/// - Registry reachability
/// - Dry-run compilation verification
/// - Version existence check (skip already-published versions)
/// - Ownership verification (optional, based on policy)
///
/// # Arguments
///
/// * `ws` - The planned workspace containing packages to publish
/// * `opts` - Runtime options controlling behavior
/// * `reporter` - A reporter for outputting progress and warnings
///
/// # Returns
///
/// Returns a [`PreflightReport`] containing:
/// - Whether a token was detected
/// - The finishability assessment (Proven/NotProven/Failed)
/// - Per-package preflight results
///
/// # Example
///
/// ```ignore
/// let ws = plan::build_plan(&spec)?;
/// let opts = types::RuntimeOptions { /* ... */ };
/// let mut reporter = MyReporter::default();
/// let report = engine::run_preflight(&ws, &opts, &mut reporter)?;
/// println!("Finishability: {:?}", report.finishability);
/// ```
/// Back-compat entry point preserving the historical preflight shape
/// (appends to `events.jsonl`). Equivalent to
/// `run_preflight_with_options(ws, opts, reporter, Default::default())`.
pub fn run_preflight(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PreflightReport> {
    let mut ws = ws.clone();
    run_preflight_in_place_with_options(&mut ws, opts, reporter, PreflightRunOptions::default())
}

/// Run preflight checks for a planned workspace and stamp regime metadata.
///
/// Takes `&mut PlannedWorkspace` so preflight can stamp the detected
/// [`crate::types::PublishRegime`] onto each `PlannedPackage` once it has
/// queried the registry (#106 PR 1). The mutation is additive: the new `regime`
/// field defaults to `None` and is skipped in serialization when unset,
/// so older readers of `state.json` / plan files stay compatible.
pub fn run_preflight_in_place(
    ws: &mut PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PreflightReport> {
    run_preflight_in_place_with_options(ws, opts, reporter, PreflightRunOptions::default())
}

/// Run preflight with caller-chosen [`PreflightRunOptions`] (#100).
///
/// See [`run_preflight`] for the full pipeline description; this entry
/// point additionally honors the `fresh_audit` flag, which redirects
/// event writes to a session-scoped sidecar (see
/// [`PreflightRunOptions::fresh_audit`] for details).
pub fn run_preflight_with_options(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    run_opts: PreflightRunOptions,
) -> Result<PreflightReport> {
    let mut ws = ws.clone();
    run_preflight_in_place_with_options(&mut ws, opts, reporter, run_opts)
}

/// Run preflight checks for a planned workspace and stamp regime metadata,
/// with caller-chosen [`PreflightRunOptions`] (#100 + #106).
pub fn run_preflight_in_place_with_options(
    ws: &mut PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    run_opts: PreflightRunOptions,
) -> Result<PreflightReport> {
    crate::config::validate_per_error_config(&opts.retry_per_error)?;
    preflight::run(ws, opts, reporter, run_opts)
}

/// Execute the publish operation for all packages in the workspace.
///
/// This is the main publishing function that:
/// 1. Acquires an owned local filesystem guard to prevent cooperating concurrent publishes
/// 2. Checks git cleanliness (if configured)
/// 3. Initializes or resumes from existing state
/// 4. Publishes each package in dependency order
/// 5. Verifies visibility on the registry after each publish
/// 6. Writes a receipt with full evidence upon completion
///
/// # Arguments
///
/// * `ws` - The planned workspace containing packages to publish
/// * `opts` - Runtime options controlling retry, readiness, policy, etc.
/// * `reporter` - A reporter for outputting progress and warnings
///
/// # Returns
///
/// Returns a [`Receipt`] containing:
/// - The plan ID and registry
/// - Start and finish timestamps
/// - Per-package receipts with evidence
/// - Git context and environment fingerprint
/// - Path to the event log
///
/// # Behavior
///
/// - **Resumability**: If interrupted, the state is persisted and `run_resume` can continue
/// - **Parallel publishing**: If `opts.parallel.enabled` is true, uses parallel publishing
/// - **Readiness checks**: Verifies crate visibility after publishing (configurable)
/// - **Retry logic**: Retries transient failures with exponential backoff
///
/// # Error Handling
///
/// Returns an error if:
/// - Lock acquisition fails
/// - Git check fails (when required)
/// - A permanent error occurs (e.g., authentication failure)
/// - All retry attempts are exhausted
pub fn run_publish(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    run_publish_with_state(ws, opts, reporter).map(|outcome| outcome.receipt)
}

/// A completed invocation's receipt and the full-plan state verified with it.
///
/// `--resume-from` can omit earlier packages from the receipt. Callers rendering
/// whole-plan progress should use this snapshot, rather than rereading mutable
/// state after the execution lock has been released.
pub struct PublishRunOutcome {
    pub receipt: Receipt,
    pub state: ExecutionState,
}

/// Run publish and retain the final state snapshot used to verify its receipt.
pub fn run_publish_with_state(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PublishRunOutcome> {
    run_publish_with_state_requirement(ws, opts, reporter, false)
}

fn run_publish_with_state_requirement(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    require_existing_state: bool,
) -> Result<PublishRunOutcome> {
    crate::ops::process::interruption::check()?;
    crate::config::validate_per_error_config(&opts.retry_per_error)?;
    let state_dir = resolve_state_dir(&ws.workspace_root, &opts.state_dir);
    let require_existing_state =
        publish::admission::precheck(ws, opts, &state_dir, require_existing_state)?;
    let publish::bootstrap::PublishBootstrap {
        state_dir,
        _lock,
        git_context,
        environment,
        auth_evidence,
        registry: reg,
        events_path,
        mut event_log,
        state: mut st,
        run_started,
    } = publish::bootstrap::prepare_publish_run(ws, opts, reporter, require_existing_state)?;

    publish::notify_publish_started(ws, opts);

    // Check for parallel mode
    if opts.parallel.enabled {
        let parallel_receipts = match crate::engine::parallel::run_publish_parallel_without_start(
            ws, opts, &mut st, &state_dir, &reg, reporter,
        ) {
            Ok(receipts) => receipts,
            Err(error) => {
                return Err(publish::finalize::record_recoverable_stop(
                    error,
                    &state_dir,
                    &events_path,
                    &mut event_log,
                    &st,
                    reporter,
                )?);
            }
        };

        publish::finalize::record_consistency_drift(&events_path, &st, &mut event_log, reporter);
        crate::ops::process::interruption::check()?;
        let receipt = publish::finalize::finish_parallel_run(
            ws,
            opts,
            &state_dir,
            &events_path,
            &mut event_log,
            &st,
            parallel_receipts,
            run_started,
            git_context,
            environment,
            auth_evidence,
        )?;
        return Ok(PublishRunOutcome { receipt, state: st });
    }

    let receipts = match execute_package::run_sequential_scheduler(
        ws,
        opts,
        &mut st,
        &state_dir,
        &reg,
        &mut event_log,
        &events_path,
        reporter,
    ) {
        Ok(receipts) => receipts,
        Err(error) => {
            return Err(publish::finalize::record_recoverable_stop(
                error,
                &state_dir,
                &events_path,
                &mut event_log,
                &st,
                reporter,
            )?);
        }
    };
    publish::finalize::record_consistency_drift(&events_path, &st, &mut event_log, reporter);
    crate::ops::process::interruption::check()?;
    let receipt = publish::finalize::finish_sequential_run(
        ws,
        opts,
        &state_dir,
        &events_path,
        &mut event_log,
        &st,
        receipts,
        run_started,
        git_context,
        environment,
        auth_evidence,
    )?;
    Ok(PublishRunOutcome { receipt, state: st })
}

/// Resume a previously interrupted publish operation.
///
/// This function loads existing state from the state directory and continues
/// publishing from where it left off. It handles:
/// - Packages that were never attempted (Pending)
/// - Packages that failed and should be retried
/// - Packages that were uploaded but not verified (Uploaded)
/// - Already-successful packages (Published/Skipped) - skipped automatically
///
/// # Arguments
///
/// * `ws` - The planned workspace (should match the original plan)
/// * `opts` - Runtime options
/// * `reporter` - A reporter for outputting progress
///
/// # Returns
///
/// Returns a [`Receipt`] similar to [`run_publish`].
///
/// # Error Handling
///
/// Returns an error if:
/// - No existing state is found in the state directory
/// - The plan ID doesn't match (use `opts.force_resume` to override)
/// - Lock acquisition fails
///
/// # Example
///
/// ```ignore
/// let mut ws = plan::build_plan(&spec)?;
/// let opts = types::RuntimeOptions { /* ... */ };
/// let mut reporter = MyReporter::default();
/// let receipt = engine::run_resume(&ws, &opts, &mut reporter)?;
/// println!("Published {} packages", receipt.packages.len());
/// ```
pub fn run_resume(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<Receipt> {
    run_resume_with_state(ws, opts, reporter).map(|outcome| outcome.receipt)
}

/// Resume and retain the final state snapshot used to verify its receipt.
pub fn run_resume_with_state(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<PublishRunOutcome> {
    run_publish_with_state_requirement(ws, opts, reporter, true)
}

fn reject_exhausted_resume_retry_ceiling(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state: &ExecutionState,
) -> Result<()> {
    let reject_package = |package: &shipper_types::PlannedPackage| -> Result<()> {
        let key = pkg_key(&package.name, &package.version);
        let Some(progress) = state.packages.get(&key) else {
            return Ok(());
        };
        if matches!(
            progress.state,
            PackageState::Failed {
                class: ErrorClass::Retryable,
                ..
            }
        ) && progress.attempts >= opts.max_attempts
        {
            Err(ResumeRetryBudgetError {
                package: key,
                current_attempts: progress.attempts,
                requested_max_attempts: opts.max_attempts,
                minimum_max_attempts: progress.attempts.checked_add(1),
            }
            .into())
        } else {
            Ok(())
        }
    };

    if opts.parallel.enabled {
        let levels = ws.plan.group_by_levels();
        let start_level = crate::engine::parallel::parallel_resume_start_level(
            &levels,
            opts.resume_from.as_deref(),
        )
        .ok_or_else(|| anyhow::anyhow!("validated resume target has no publish level"))?;
        for level in levels.iter().skip(start_level) {
            for package in &level.packages {
                reject_package(package)?;
            }
        }
    } else {
        let mut reached_resume_target = opts.resume_from.is_none();
        for package in &ws.plan.packages {
            if !reached_resume_target {
                reached_resume_target = opts.resume_from.as_deref() == Some(package.name.as_str());
                if !reached_resume_target {
                    continue;
                }
            }
            reject_package(package)?;
        }
    }
    Ok(())
}

/// Outcome of a rehearsal run. Sufficient for callers (CLI, future hard gate)
/// to decide whether live dispatch is authorized without re-reading events.
///
/// #97 PR 2. The hard gate (#97 PR 3) will bind this outcome to a `plan_id`
/// so "rehearsal passed" can't be claimed for a different workspace state.
#[derive(Debug, Clone)]
pub struct RehearsalOutcome {
    pub passed: bool,
    pub registry_name: String,
    pub packages_attempted: usize,
    pub packages_published: usize,
    pub summary: String,
}

/// Run a rehearsal publish against an alternate registry (#97 PR 2).
///
/// Phase-2 preflight: publish every crate in the plan to a non-live
/// registry, verify each is visible on that registry, and emit a
/// `RehearsalComplete` event summarizing the outcome.
///
/// **Contract**:
/// - Reads `opts.rehearsal_registry` (set via `--rehearsal-registry` or
///   `[rehearsal]` config). Must resolve to a [`Registry`] entry in
///   `opts.registries`; bails clean otherwise.
/// - Refuses to rehearse against the live target (`ws.plan.registry`).
///   Rehearsal and live must be different registries.
/// - Runs sequentially (no parallel yet); stops at the first failure.
/// - Does NOT touch `state.json`. Rehearsal is a pre-publish proof, not
///   an execution; it only appends to `events.jsonl` so auditors can
///   replay the rehearsal from the event log.
/// - Post-publish visibility check uses the SAME readiness mechanism as
///   live publish (`reg.version_exists`). A rehearsal artifact that's
///   not visible within the readiness window fails the rehearsal.
///
/// **Not in this PR**:
/// - Hard gate wiring into `run_publish` (PR 3).
/// - Install/smoke check against the rehearsal registry (PR 3 or 4).
/// - Parallel rehearsal (not planned; rehearsal is infrequent and
///   correctness > speed).
pub fn run_rehearsal(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<RehearsalOutcome> {
    crate::ops::process::interruption::check()?;
    crate::config::validate_per_error_config(&opts.retry_per_error)?;
    let rehearsal_name = opts
        .rehearsal_registry
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no rehearsal registry configured; set --rehearsal-registry <name> \
             or enable [rehearsal] in .shipper.toml"
            )
        })?
        .clone();

    if opts.rehearsal_skip {
        reporter.warn(&format!(
            "--skip-rehearsal set; rehearsal against '{rehearsal_name}' was requested but will not run. \
             Once #97 PR 3 lands, live dispatch will refuse without a prior passing rehearsal."
        ));
        return Ok(RehearsalOutcome {
            passed: false,
            registry_name: rehearsal_name,
            packages_attempted: 0,
            packages_published: 0,
            summary: "skipped by --skip-rehearsal".to_string(),
        });
    }

    let rehearsal_reg = opts
        .registries
        .iter()
        .find(|r| r.name == rehearsal_name)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "rehearsal registry '{rehearsal_name}' is not configured. \
             Add it to [[registries.registries]] in .shipper.toml and select it with --registries."
            )
        })?;

    let mut rehearsal_opts = opts.clone();
    let rehearsal_policy = rehearsal_opts
        .registry_policies
        .entry(rehearsal_reg.name.clone())
        .or_insert_with(RegistryTrustOptions::secure);
    rehearsal_policy.allow_loopback = true;
    let live_identity = validated_registry_identity(ws.plan.registry.clone(), opts)
        .context("invalid live registry identity for rehearsal isolation")?;
    let rehearsal_identity = validated_registry_identity(rehearsal_reg.clone(), &rehearsal_opts)
        .context("invalid rehearsal registry identity")?;
    rehearsal_identity.ensure_rehearsal_isolated_from(&live_identity)?;

    let workspace_root = &ws.workspace_root;
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);
    validate_publish_cleanup_safety(&state_dir)?;
    let reservation = publish::admission::reserve_rehearsal(ws, opts, &state_dir)?;
    let _lock = reservation
        .claim(Some(&ws.plan.plan_id))
        .context(PublishAdmissionError)?;
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;
    let events_path = events::events_path(&state_dir);
    let mut event_log = events::EventLog::new();
    let started_at = Utc::now();

    reporter.info(&format!(
        "rehearsal starting â€” {} packages against '{}'",
        ws.plan.packages.len(),
        rehearsal_name
    ));

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::RehearsalStarted {
            registry: rehearsal_name.clone(),
            plan_id: ws.plan.plan_id.clone(),
            package_count: ws.plan.packages.len(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;
    event_log.clear();

    let rehearsal_client =
        init_registry_client(rehearsal_reg.clone(), &state_dir, &rehearsal_opts)?;
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::RegistryPolicyApplied {
            evidence: rehearsal_client.policy_evidence(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;
    event_log.clear();

    let mut packages_published: usize = 0;
    let mut first_failure: Option<String> = None;

    for p in &ws.plan.packages {
        crate::ops::process::interruption::check()?;
        let pkg_label = format!("{}@{}", p.name, p.version);
        reporter.info(&format!("rehearsing {pkg_label} â†’ {rehearsal_name}"));
        let start = Instant::now();

        let captured = cargo::cargo_publish_with_capture(
            workspace_root,
            &p.name,
            &rehearsal_reg.name,
            opts.allow_dirty,
            opts.no_verify,
            opts.output_lines,
            None,
        )?;
        if let Some(failure) = captured.cleanup_failure {
            let message = cleanup::message(failure);
            reporter.error(&message);
            return Err(cleanup::stop_rehearsal(
                &mut event_log,
                &events_path,
                &pkg_label,
                message,
            ));
        }
        if captured.interrupted || crate::ops::process::interruption::requested() {
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::RehearsalPackageFailed {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    class: ErrorClass::Ambiguous,
                    message: "publication interrupted; process cleanup confirmed; registry outcome is unknown".into(),
                },
                package: pkg_label.clone(),
            });
            event_log
                .write_to_file(&events_path)
                .context(crate::ops::process::interruption::Interrupted)?;
            return Err(crate::ops::process::interruption::Interrupted.into());
        }
        let capture_complete = captured.capture.is_complete();
        let out = captured.output;
        if !capture_complete {
            reporter.warn(
                "rehearsal Cargo diagnostic capture was incomplete; diagnostic tails omitted",
            );
        }

        if out.exit_code != 0 {
            let (class, msg) = if !capture_complete {
                (
                    ErrorClass::Ambiguous,
                    "cargo publish diagnostic capture was incomplete".to_string(),
                )
            } else {
                classify_cargo_failure(&out.stderr_tail, &out.stdout_tail)
            };
            reporter.error(&format!(
                "rehearsal failed for {pkg_label}: {msg}\nstderr tail:\n{}",
                out.stderr_tail
            ));
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::RehearsalPackageFailed {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    class,
                    message: msg.clone(),
                },
                package: pkg_label.clone(),
            });
            event_log.write_to_file(&events_path)?;
            event_log.clear();
            first_failure = Some(format!("{pkg_label}: {msg}"));
            break;
        }

        drop(captured._interruption_lease);
        crate::ops::process::interruption::check()?;

        // Post-publish visibility check on the rehearsal registry. Reuse
        // `version_exists` â€” same mechanism live publish trusts.
        if !rehearsal_client.version_exists(&p.name, &p.version)? {
            let msg = format!(
                "rehearsal: cargo publish succeeded but {pkg_label} is not visible on '{rehearsal_name}'"
            );
            reporter.error(&msg);
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::RehearsalPackageFailed {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    class: ErrorClass::Ambiguous,
                    message: msg.clone(),
                },
                package: pkg_label.clone(),
            });
            event_log.write_to_file(&events_path)?;
            event_log.clear();
            first_failure = Some(msg);
            break;
        }

        let duration_ms = start.elapsed().as_millis();
        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::RehearsalPackagePublished {
                name: p.name.clone(),
                version: p.version.clone(),
                duration_ms,
            },
            package: pkg_label.clone(),
        });
        event_log.write_to_file(&events_path)?;
        event_log.clear();
        packages_published += 1;
    }

    // #97 PR 4 â€” smoke-install. Opt-in. Runs only if:
    //   (a) all packages in the plan were published successfully AND
    //   (b) the operator named a crate via --smoke-install / config.
    // The named crate must be in the plan; resolves its planned version
    // to pass through to `cargo install`.
    if first_failure.is_none()
        && let Some(ref smoke_name) = opts.rehearsal_smoke_install
    {
        match ws.plan.packages.iter().find(|p| &p.name == smoke_name) {
            Some(smoke_pkg) => {
                reporter.info(&format!(
                    "smoke-install: {smoke_name}@{} from '{rehearsal_name}'",
                    smoke_pkg.version
                ));

                event_log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::RehearsalSmokeCheckStarted {
                        name: smoke_pkg.name.clone(),
                        version: smoke_pkg.version.clone(),
                        registry: rehearsal_name.clone(),
                    },
                    package: format!("{smoke_name}@{}", smoke_pkg.version),
                });
                event_log.write_to_file(&events_path)?;
                event_log.clear();

                let install_root = state_dir.join("smoke-install");
                let _ = std::fs::remove_dir_all(&install_root);
                let smoke_start = Instant::now();
                let out = cargo::cargo_install_smoke(
                    workspace_root,
                    &smoke_pkg.name,
                    &smoke_pkg.version,
                    &rehearsal_reg.name,
                    &install_root,
                    opts.output_lines,
                    None,
                )?;

                if out.exit_code == 0 {
                    let duration_ms = smoke_start.elapsed().as_millis();
                    event_log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::RehearsalSmokeCheckSucceeded {
                            name: smoke_pkg.name.clone(),
                            version: smoke_pkg.version.clone(),
                            duration_ms,
                        },
                        package: format!("{smoke_name}@{}", smoke_pkg.version),
                    });
                    reporter.info(&format!(
                        "smoke-install OK for {smoke_name}@{}",
                        smoke_pkg.version
                    ));
                } else {
                    let msg = format!(
                        "cargo install exited {} for {smoke_name}@{}. stderr tail:\n{}",
                        out.exit_code, smoke_pkg.version, out.stderr_tail
                    );
                    reporter.error(&msg);
                    event_log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::RehearsalSmokeCheckFailed {
                            name: smoke_pkg.name.clone(),
                            version: smoke_pkg.version.clone(),
                            message: msg.clone(),
                        },
                        package: format!("{smoke_name}@{}", smoke_pkg.version),
                    });
                    first_failure = Some(format!(
                        "smoke-install of {smoke_name}@{} failed: cargo exit {}",
                        smoke_pkg.version, out.exit_code
                    ));
                }
                event_log.write_to_file(&events_path)?;
                event_log.clear();
            }
            None => {
                // Operator named a crate that isn't in the plan. Warn,
                // don't fail â€” their intent is clear but the workspace
                // shape disagrees, and failing the whole rehearsal over
                // a typo would be overkill.
                reporter.warn(&format!(
                    "smoke-install target '{smoke_name}' is not in the rehearsal plan; skipping. \
                     Available crates: {}",
                    ws.plan
                        .packages
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }

    let passed = first_failure.is_none();
    let summary = if passed {
        format!("rehearsed {packages_published} packages against '{rehearsal_name}' successfully")
    } else {
        format!(
            "rehearsal stopped at {}/{}: {}",
            packages_published + 1,
            ws.plan.packages.len(),
            first_failure.as_deref().unwrap_or("")
        )
    };

    let completed_at = Utc::now();
    event_log.record(PublishEvent {
        timestamp: completed_at,
        event_type: EventType::RehearsalComplete {
            passed,
            registry: rehearsal_name.clone(),
            plan_id: ws.plan.plan_id.clone(),
            summary: summary.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;

    // Persist the sidecar receipt so the hard gate in `run_publish`
    // can consult it without parsing events.jsonl. Best-effort â€” a
    // write failure here doesn't invalidate the events log, which is
    // the authoritative source; the gate will just act as if no
    // rehearsal happened and block, which is the safe default.
    let packages_attempted = packages_published + if passed { 0 } else { 1 };
    if let Err(err) = crate::state::rehearsal::save_rehearsal(
        &state_dir,
        &crate::state::rehearsal::RehearsalReceipt {
            schema_version: crate::state::rehearsal::CURRENT_REHEARSAL_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: rehearsal_name.clone(),
            passed,
            packages_attempted,
            packages_published,
            summary: summary.clone(),
            started_at,
            completed_at,
        },
    ) {
        reporter.warn(&format!(
            "rehearsal outcome event was written, but sidecar receipt could not be persisted: {err:#}. \
             The hard gate may not recognize this rehearsal â€” check {}.",
            crate::state::rehearsal::rehearsal_path(&state_dir).display()
        ));
    }

    if passed {
        reporter.info(&summary);
    } else {
        reporter.error(&summary);
    }

    Ok(RehearsalOutcome {
        passed,
        registry_name: rehearsal_name,
        packages_attempted,
        packages_published,
        summary,
    })
}

pub(crate) fn init_state(ws: &PlannedWorkspace, state_dir: &Path) -> Result<ExecutionState> {
    let mut packages: BTreeMap<String, PackageProgress> = BTreeMap::new();
    for p in &ws.plan.packages {
        packages.insert(
            pkg_key(&p.name, &p.version),
            PackageProgress {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
    }

    let st = ExecutionState {
        state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        attempt_history: Vec::new(),
        packages,
    };

    state::save_state(state_dir, &st)?;
    Ok(st)
}

#[cfg(test)]
mod tests {
    mod capture_reconciliation;
    mod cleanup_hold;
    mod file_sync;
    mod live_retry;
    mod lock_ownership;
    mod per_error_refusal;
    mod reconciliation_concurrency;
    mod retained_admission;
    mod retained_retryable;
    mod timeout_reconciliation;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use chrono::Utc;
    use serial_test::serial;
    use tempfile::tempdir;
    use tiny_http::{Header, Response, Server, StatusCode};

    use super::*;
    use crate::plan::PlannedWorkspace;
    use crate::types::{AuthEvidenceMode, AuthType, PlannedPackage, Registry, ReleasePlan};

    #[derive(Default)]
    struct CollectingReporter {
        infos: Vec<String>,
        warns: Vec<String>,
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

    #[cfg(windows)]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo.cmd")
    }

    #[cfg(not(windows))]
    fn fake_cargo_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("cargo")
    }

    #[cfg(windows)]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git.cmd")
    }

    #[cfg(not(windows))]
    fn fake_git_path(bin_dir: &Path) -> PathBuf {
        bin_dir.join("git")
    }

    fn fake_program_env_vars(bin_dir: &Path) -> Vec<(&'static str, Option<String>)> {
        vec![
            (
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path(bin_dir).to_str().expect("utf8").to_string()),
            ),
            (
                "SHIPPER_GIT_BIN",
                Some(fake_git_path(bin_dir).to_str().expect("utf8").to_string()),
            ),
        ]
    }

    /// Build a combined env var list from fake programs + additional vars, then run closure.
    fn with_test_env<F, R>(bin_dir: &Path, extra: Vec<(&'static str, Option<String>)>, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let mut vars = fake_program_env_vars(bin_dir);
        vars.extend(extra);
        temp_env::with_vars(vars, f)
    }

    fn write_fake_cargo(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("cargo.cmd"),
                "@echo off\r\nif not \"%SHIPPER_CARGO_ARGS_LOG%\"==\"\" echo %*>>\"%SHIPPER_CARGO_ARGS_LOG%\"\r\nif not \"%SHIPPER_CARGO_STDOUT%\"==\"\" echo %SHIPPER_CARGO_STDOUT%\r\nif not \"%SHIPPER_CARGO_STDERR%\"==\"\" echo %SHIPPER_CARGO_STDERR% 1>&2\r\nif \"%SHIPPER_CARGO_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_CARGO_EXIT%)\r\n",
            )
            .expect("write fake cargo");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("cargo");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ -n \"$SHIPPER_CARGO_ARGS_LOG\" ]; then\n  echo \"$*\" >>\"$SHIPPER_CARGO_ARGS_LOG\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDOUT\" ]; then\n  echo \"$SHIPPER_CARGO_STDOUT\"\nfi\nif [ -n \"$SHIPPER_CARGO_STDERR\" ]; then\n  echo \"$SHIPPER_CARGO_STDERR\" >&2\nfi\nexit \"${SHIPPER_CARGO_EXIT:-0}\"\n",
            )
            .expect("write fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
    }

    fn write_fake_cargo_ambiguous_then_permanent(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("cargo.cmd"),
                "@echo off\r\nif not \"%SHIPPER_CARGO_ARGS_LOG%\"==\"\" echo %*>>\"%SHIPPER_CARGO_ARGS_LOG%\"\r\nset COUNT_FILE=%SHIPPER_CARGO_COUNT_FILE%\r\nif \"%COUNT_FILE%\"==\"\" set COUNT_FILE=%TEMP%\\shipper-cargo-count.txt\r\nif not exist \"%COUNT_FILE%\" (\r\n  echo 1>\"%COUNT_FILE%\"\r\n  exit /b 1\r\n)\r\necho crate version 0.1.0 is already uploaded 1>&2\r\nexit /b 1\r\n",
            )
            .expect("write sequenced fake cargo");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("cargo");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ -n \"$SHIPPER_CARGO_ARGS_LOG\" ]; then\n  echo \"$*\" >>\"$SHIPPER_CARGO_ARGS_LOG\"\nfi\ncount_file=\"${SHIPPER_CARGO_COUNT_FILE:-${TMPDIR:-/tmp}/shipper-cargo-count.txt}\"\nif [ ! -f \"$count_file\" ]; then\n  echo 1 >\"$count_file\"\n  exit 1\nfi\necho 'crate version 0.1.0 is already uploaded' >&2\nexit 1\n",
            )
            .expect("write sequenced fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
    }

    fn write_fake_git(bin_dir: &Path) {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join("git.cmd"),
                "@echo off\r\nif \"%SHIPPER_GIT_FAIL%\"==\"1\" (\r\n  echo fatal: git failed 1>&2\r\n  exit /b 1\r\n)\r\nif \"%SHIPPER_GIT_CLEAN%\"==\"0\" (\r\n  echo M src/lib.rs\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"${SHIPPER_GIT_FAIL:-0}\" = \"1\" ]; then\n  echo 'fatal: git failed' >&2\n  exit 1\nfi\nif [ \"${SHIPPER_GIT_CLEAN:-1}\" = \"0\" ]; then\n  echo 'M src/lib.rs'\nfi\nexit 0\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
    }

    fn write_fake_tools(bin_dir: &Path) {
        fs::create_dir_all(bin_dir).expect("mkdir");
        write_fake_cargo(bin_dir);
        write_fake_git(bin_dir);
    }

    #[test]
    #[serial]
    fn stale_state_does_not_enable_legacy_index_fallback() {
        let td = tempdir().expect("tempdir");
        fs::write(td.path().join("state.json"), "stale state").expect("write stale state");
        let registry = Registry {
            name: "custom".to_string(),
            api_base: "https://registry.example.com".to_string(),
            index_base: None,
        };
        let opts = default_opts(PathBuf::from(".shipper"));

        let err = init_registry_client(registry, td.path(), &opts)
            .expect_err("new or unvalidated runs must require explicit index_base");
        assert!(err.to_string().contains("explicit index_base"));
    }

    struct TestRegistryServer {
        base_url: String,
        #[allow(clippy::type_complexity)]
        seen: Arc<Mutex<Vec<(String, Option<String>)>>>,
        handle: thread::JoinHandle<()>,
    }

    impl TestRegistryServer {
        fn join(self) {
            self.handle.join().expect("join server");
        }
    }

    fn spawn_registry_server(
        mut routes: std::collections::BTreeMap<String, Vec<(u16, String)>>,
        expected_requests: usize,
    ) -> TestRegistryServer {
        let server = Server::http("127.0.0.1:0").expect("server");
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
        let seen_thread = Arc::clone(&seen);

        let handle = thread::spawn(move || {
            for _ in 0..expected_requests {
                let req = match server.recv_timeout(Duration::from_secs(30)) {
                    Ok(Some(r)) => r,
                    _ => break,
                };
                let path = req.url().to_string();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                seen_thread.lock().expect("lock").push((path.clone(), auth));

                let response = if let Some(list) = routes.get_mut(&path) {
                    if list.is_empty() {
                        (404, "{}".to_string())
                    } else if list.len() == 1 {
                        list[0].clone()
                    } else {
                        list.remove(0)
                    }
                } else {
                    (404, "{}".to_string())
                };

                let resp = Response::from_string(response.1)
                    .with_status_code(StatusCode(response.0))
                    .with_header(
                        Header::from_bytes("Content-Type", "application/json").expect("header"),
                    );
                req.respond(resp).expect("respond");
            }
        });

        TestRegistryServer {
            base_url,
            seen,
            handle,
        }
    }

    fn planned_workspace(workspace_root: &Path, api_base: String) -> PlannedWorkspace {
        PlannedWorkspace {
            workspace_root: workspace_root.to_path_buf(),
            plan: ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "plan-demo".to_string(),
                created_at: Utc::now(),
                registry: Registry {
                    name: "crates-io".to_string(),
                    api_base: api_base.clone(),
                    index_base: Some(api_base),
                },
                packages: vec![PlannedPackage {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: workspace_root.join("demo").join("Cargo.toml"),
                    regime: None,
                }],
                dependencies: std::collections::BTreeMap::new(),
            },
            skipped: vec![],
        }
    }

    fn default_opts(state_dir: PathBuf) -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            verify_timeout: Duration::from_millis(20),
            verify_poll_interval: Duration::from_millis(1),
            state_dir,
            force_resume: false,
            policy: crate::types::PublishPolicy::default(),
            verify_mode: crate::types::VerifyMode::default(),
            readiness: crate::types::ReadinessConfig {
                enabled: true,
                method: crate::types::ReadinessMethod::Api,
                initial_delay: Duration::from_millis(0),
                max_delay: Duration::from_millis(20),
                max_total_wait: Duration::from_millis(200),
                poll_interval: Duration::from_millis(1),
                jitter_factor: 0.0,
                index_path: None,
                prefer_index: false,
            },
            output_lines: 100,
            force: false,
            lock_timeout: Duration::from_hours(1),
            parallel: crate::types::ParallelConfig::default(),
            webhook: crate::webhook::WebhookConfig::default(),
            retry_strategy: crate::retry::RetryStrategyType::Exponential,
            retry_jitter: 0.0,
            retry_per_error: crate::retry::PerErrorConfig::default(),
            encryption: crate::encryption::EncryptionConfig::default(),
            registries: vec![],
            registry_policies: std::collections::BTreeMap::from([(
                "crates-io".to_string(),
                crate::types::RegistryTrustOptions {
                    allow_private: false,
                    allow_loopback: true,
                },
            )]),
            resume_from: None,
            rehearsal_registry: None,
            rehearsal_skip: false,
            rehearsal_smoke_install: None,
        }
    }

    #[test]
    fn classify_cargo_failure_covers_retryable_permanent_and_ambiguous() {
        let retryable = classify_cargo_failure("HTTP 429 too many requests", "");
        assert_eq!(retryable.0, ErrorClass::Retryable);

        let permanent = classify_cargo_failure("permission denied", "");
        assert_eq!(permanent.0, ErrorClass::Permanent);

        let ambiguous = classify_cargo_failure("strange output", "");
        assert_eq!(ambiguous.0, ErrorClass::Ambiguous);
    }

    #[test]
    fn collecting_reporter_error_method_records_message() {
        let mut reporter = CollectingReporter::default();
        reporter.error("boom");
        assert_eq!(reporter.errors, vec!["boom".to_string()]);
    }

    #[test]
    fn helper_functions_return_expected_values() {
        let root = PathBuf::from("root");
        let rel = resolve_state_dir(&root, &PathBuf::from(".shipper"));
        assert_eq!(rel, root.join(".shipper"));

        #[cfg(windows)]
        {
            let abs = PathBuf::from(r"C:\x\state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }
        #[cfg(not(windows))]
        {
            let abs = PathBuf::from("/x/state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }

        assert_eq!(pkg_key("a", "1.2.3"), "a@1.2.3");
        assert_eq!(short_state(&PackageState::Pending), "pending");
        assert_eq!(short_state(&PackageState::Uploaded), "uploaded");
        assert_eq!(short_state(&PackageState::Published), "published");
        assert_eq!(
            short_state(&PackageState::Skipped {
                reason: "r".to_string()
            }),
            "skipped"
        );
        assert_eq!(
            short_state(&PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "m".to_string()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&PackageState::Ambiguous {
                message: "m".to_string()
            }),
            "ambiguous"
        );
    }

    #[test]
    fn backoff_delay_is_bounded_with_jitter() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);
        let d1 = backoff_delay(
            base,
            max,
            1,
            crate::retry::RetryStrategyType::Exponential,
            0.5,
        );
        let d20 = backoff_delay(
            base,
            max,
            20,
            crate::retry::RetryStrategyType::Exponential,
            0.5,
        );

        assert!(d1 >= Duration::from_millis(50));
        assert!(d1 <= Duration::from_millis(150));

        assert!(d20 >= Duration::from_millis(250));
        assert!(d20 <= Duration::from_millis(750));
    }

    #[test]
    #[serial]
    fn verify_published_returns_true_when_registry_visibility_appears() {
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );

        let reg = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: true,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(50),
            // Keep this generous to avoid timing flakes under highly parallel test execution.
            max_total_wait: Duration::from_secs(2),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let (ok, evidence) = verify_published(
            &reg,
            "demo",
            "0.1.0",
            &config,
            &mut reporter,
            &mut event_log,
            &events_path,
            "demo@0.1.0",
        )
        .expect("verify");
        assert!(ok);
        assert!(!reporter.infos.is_empty());
        assert!(!evidence.is_empty());
        server.join();
    }

    #[test]
    #[serial]
    fn verify_published_returns_false_on_timeout() {
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string())],
            )]),
            1,
        );
        let reg = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: true,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(10),
            max_total_wait: Duration::from_millis(0),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let (ok, evidence) = verify_published(
            &reg,
            "demo",
            "0.1.0",
            &config,
            &mut reporter,
            &mut event_log,
            &events_path,
            "demo@0.1.0",
        )
        .expect("verify");
        assert!(!ok);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].attempt, 1);
        assert!(!evidence[0].visible);
        let events = events::EventLog::read_from_file(&events_path).expect("events");
        assert!(events.all_events().iter().any(|event| matches!(
            event.event_type,
            EventType::ReadinessTimeout { max_wait_ms: 0 }
        )));
        assert!(
            !events
                .all_events()
                .iter()
                .any(|event| matches!(event.event_type, EventType::ReadinessError { .. }))
        );
        assert_eq!(server.seen.lock().expect("requests").len(), 1);
        server.join();
    }

    #[test]
    #[serial]
    fn verify_published_keeps_unavailable_registry_unknown() {
        let reg = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: "http://127.0.0.1:9".to_string(),
            index_base: None,
        })
        .expect("client");
        let mut config = default_opts(PathBuf::from(".shipper")).readiness;
        config.max_total_wait = Duration::ZERO;
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let error = verify_published(
            &reg,
            "demo",
            "0.1.0",
            &config,
            &mut CollectingReporter::default(),
            &mut events::EventLog::new(),
            &events_path,
            "demo@0.1.0",
        )
        .expect_err("unavailable transport cannot establish absence");
        assert!(
            error
                .downcast_ref::<shipper_registry::ReadinessObservationError>()
                .is_some()
        );
        let events = events::EventLog::read_from_file(&events_path).expect("events");
        assert!(
            events
                .all_events()
                .iter()
                .any(|event| matches!(event.event_type, EventType::ReadinessError { .. }))
        );
        assert!(!events.all_events().iter().any(|event| matches!(
            event.event_type,
            EventType::ReadinessTimeout { .. } | EventType::ReadinessPoll { visible: false, .. }
        )));
    }

    /// The polling loop itself now lives in `shipper-registry`, but the
    /// *event envelope* around it stays engine-side (issue #202). This pins
    /// the ordering the engine is responsible for: `readiness_started` opens
    /// the run, `readiness_complete` closes it, and every scheduled poll is
    /// announced before the poll it schedules is recorded.
    #[test]
    #[serial]
    fn verify_published_writes_readiness_events_in_envelope_order() {
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string()), (200, "{}".to_string())],
            )]),
            2,
        );

        let reg = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: true,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_secs(2),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let (ok, _evidence) = verify_published(
            &reg,
            "demo",
            "0.1.0",
            &config,
            &mut reporter,
            &mut event_log,
            &events_path,
            "demo@0.1.0",
        )
        .expect("verify");
        assert!(ok);
        server.join();

        let raw = std::fs::read_to_string(&events_path).expect("events.jsonl");
        let kinds: Vec<String> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("event json");
                v["event_type"]["type"]
                    .as_str()
                    .expect("event type tag")
                    .to_string()
            })
            .collect();

        assert_eq!(
            kinds.first().map(String::as_str),
            Some("readiness_started"),
            "envelope must open with readiness_started: {kinds:?}"
        );
        assert_eq!(
            kinds.last().map(String::as_str),
            Some("readiness_complete"),
            "envelope must close with readiness_complete: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k == "readiness_poll"),
            "the polling loop must contribute readiness_poll events: {kinds:?}"
        );

        // Every scheduled poll is announced before the poll it schedules.
        let scheduled: Vec<u32> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).ok()?;
                if v["event_type"]["type"] == "readiness_poll_scheduled" {
                    v["event_type"]["attempt"].as_u64().map(|a| a as u32)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            !scheduled.is_empty(),
            "a 404-then-200 run must schedule a second poll: {kinds:?}"
        );
        for attempt in scheduled {
            let sched_idx = raw
                .lines()
                .filter(|l| !l.trim().is_empty())
                .position(|l| {
                    let v: serde_json::Value = serde_json::from_str(l).expect("event json");
                    v["event_type"]["type"] == "readiness_poll_scheduled"
                        && v["event_type"]["attempt"].as_u64() == Some(attempt as u64)
                })
                .expect("scheduled event index");
            let poll_idx = raw
                .lines()
                .filter(|l| !l.trim().is_empty())
                .position(|l| {
                    let v: serde_json::Value = serde_json::from_str(l).expect("event json");
                    v["event_type"]["type"] == "readiness_poll"
                        && v["event_type"]["attempt"].as_u64() == Some(attempt as u64)
                })
                .expect("poll event index");
            assert!(
                sched_idx < poll_idx,
                "attempt {attempt}: scheduling event must precede the poll it schedules"
            );
        }
    }

    #[test]
    fn registry_server_helper_returns_404_for_unknown_or_empty_routes() {
        let server_unknown = spawn_registry_server(std::collections::BTreeMap::new(), 1);
        let reg_unknown = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server_unknown.base_url.clone(),
            index_base: None,
        })
        .expect("client");
        let exists_unknown = reg_unknown
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_unknown);
        server_unknown.join();

        let server_empty = spawn_registry_server(
            std::collections::BTreeMap::from([("/api/v1/crates/demo/0.1.0".to_string(), vec![])]),
            1,
        );
        let reg_empty = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server_empty.base_url.clone(),
            index_base: None,
        })
        .expect("client");
        let exists_empty = reg_empty
            .version_exists("demo", "0.1.0")
            .expect("version exists");
        assert!(!exists_empty);
        server_empty.join();
    }

    #[test]
    #[serial]
    fn run_preflight_errors_in_strict_mode_without_token() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.strict_ownership = true;
        opts.skip_ownership_check = false;
        temp_env::with_vars(
            [
                (
                    "CARGO_HOME",
                    Some(td.path().to_str().expect("utf8").to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ],
            || {
                let mut reporter = CollectingReporter::default();
                let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(
                    format!("{err:#}").contains("strict ownership requested but no token found")
                );
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_rejects_missing_strict_token_before_state_for_both_schedulers() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        temp_env::with_vars(
            [
                (
                    "CARGO_HOME",
                    Some(td.path().join("cargo-home").display().to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ],
            || {
                for parallel in [false, true] {
                    let state_dir = td.path().join(if parallel {
                        "parallel-state"
                    } else {
                        "sequential-state"
                    });
                    let mut opts = default_opts(state_dir.clone());
                    opts.strict_ownership = true;
                    opts.skip_ownership_check = false;
                    opts.parallel.enabled = parallel;

                    let mut reporter = CollectingReporter::default();
                    let error = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                    assert!(
                        format!("{error:#}")
                            .contains("strict ownership requested but no token found")
                    );
                    assert!(
                        !state_dir.exists(),
                        "strict token gate must run before {parallel:?} scheduler state"
                    );
                    assert!(reporter.infos.is_empty());
                    assert!(reporter.warns.is_empty());
                    assert!(reporter.errors.is_empty());
                }
            },
        );
    }

    #[test]
    #[serial]
    fn strict_token_gate_preserves_token_policy_and_skip_opposites() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                (
                    "CARGO_HOME",
                    Some(td.path().join("cargo-home").display().to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", Some("test-token".to_string())),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ],
            || {
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.strict_ownership = true;
                let effects = policy_effects(&opts);
                assert!(require_strict_ownership_token(&effects, Some("test-token")).is_ok());
            },
        );

        temp_env::with_vars(
            [
                (
                    "CARGO_HOME",
                    Some(td.path().join("empty-cargo-home").display().to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ],
            || {
                for policy in [
                    crate::types::PublishPolicy::Balanced,
                    crate::types::PublishPolicy::Fast,
                ] {
                    let mut opts = default_opts(PathBuf::from(".shipper"));
                    opts.policy = policy;
                    opts.strict_ownership = true;
                    let effects = policy_effects(&opts);
                    assert!(!effects.strict_ownership);
                    assert!(require_strict_ownership_token(&effects, None).is_ok());
                }

                let mut skip_only = default_opts(PathBuf::from(".shipper"));
                skip_only.skip_ownership_check = true;
                skip_only.strict_ownership = false;
                assert!(require_strict_ownership_token(&policy_effects(&skip_only), None).is_ok());
            },
        );
    }

    #[test]
    #[serial]
    fn run_preflight_warns_on_owners_failure_when_not_strict() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo/owners".to_string(),
                        vec![(403, "{}".to_string())],
                    ),
                ]),
                3,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = false;
            opts.strict_ownership = false;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
            assert!(rep.token_detected);
            assert_eq!(rep.packages.len(), 1);
            assert!(!rep.packages[0].already_published);
            assert!(!rep.packages[0].ownership_verified);
            assert!(rep.packages[0].dry_run_passed);
            assert_eq!(rep.finishability, Finishability::NotProven);
            assert!(
                reporter
                    .warns
                    .iter()
                    .any(|w| w.contains("owners preflight failed"))
            );

            let seen = server.seen.lock().expect("lock");
            assert_eq!(seen.len(), 3);
            drop(seen);
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_owners_success_path() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(200, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo/owners".to_string(),
                        vec![(
                            200,
                            r#"{"users":[{"id":1,"login":"alice","name":"Alice"}]}"#.to_string(),
                        )],
                    ),
                ]),
                3,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = false;
            opts.strict_ownership = false;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            assert!(reporter.warns.is_empty());
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_returns_error_when_strict_ownership_check_fails() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            // Crate must exist (200) so ownership check is actually attempted;
            // 404 would mean new crate -> ownership check skipped.
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(200, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo/owners".to_string(),
                        vec![(403, "{}".to_string())],
                    ),
                ]),
                3,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = false;
            opts.strict_ownership = true;

            let mut reporter = CollectingReporter::default();
            let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
            assert!(format!("{err:#}").contains("forbidden when querying owners"));
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_strict_skips_ownership_for_new_crate() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            // Crate returns 404 (new crate) -- ownership check should be skipped.
            // No /owners endpoint needed.
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = false;
            opts.strict_ownership = true;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight_in_place(&mut ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            assert!(!rep.packages[0].ownership_verified);
            assert!(rep.packages[0].is_new_crate);
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|i| i.contains("new crate, skipping ownership check"))
            );
            // #106 PR 1: preflight must stamp PublishRegime::FirstPublish
            // on the plan so the downstream publish retry loop can
            // consume it without re-querying the registry.
            assert_eq!(
                ws.plan.packages[0].regime,
                Some(PublishRegime::FirstPublish),
                "preflight should stamp FirstPublish regime on new crates"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_legacy_entry_point_preserves_immutable_api() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));
            let mut reporter = CollectingReporter::default();

            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            assert_eq!(rep.packages.len(), 1);
            assert!(rep.packages[0].is_new_crate);
            assert_eq!(
                ws.plan.packages[0].regime, None,
                "legacy immutable API should not mutate the caller's plan"
            );
            server.join();
        });
    }

    /// #106 PR 1: preflight stamps `PublishRegime::Update` on crates
    /// that already have at least one published version. Ensures the
    /// downstream retry loop can distinguish update vs. first-publish
    /// backoff windows without re-querying the registry.
    #[test]
    #[serial]
    fn run_preflight_stamps_update_regime_on_existing_crate() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            // Crate root returns 200 (crate exists), specific version 404
            // (this version is not yet published). This is the canonical
            // "publishing a new version of an existing crate" preflight.
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(200, r#"{"crate":{"name":"demo"}}"#.to_string())],
                    ),
                ]),
                2,
            );

            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.skip_ownership_check = true;
            opts.strict_ownership = false;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight_in_place(&mut ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            assert!(!rep.packages[0].is_new_crate);
            assert_eq!(
                ws.plan.packages[0].regime,
                Some(PublishRegime::Update),
                "preflight should stamp Update regime on existing crates"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_writes_preflight_events() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = true;

            let mut reporter = CollectingReporter::default();
            let _ = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            let events_path = td.path().join(".shipper").join("events.jsonl");
            let log =
                crate::state::events::EventLog::read_from_file(&events_path).expect("read events");
            let events = log.all_events();

            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightStarted))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightWorkspaceVerify { .. }))
            );
            assert!(
                events.iter().any(|e| {
                    matches!(e.event_type, EventType::PreflightNewCrateDetected { .. })
                })
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightOwnershipCheck { .. }))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightComplete { .. }))
            );
            server.join();
        });
    }

    // #100 â€” fresh-audit mode must not read or append the authoritative
    // `events.jsonl`. We seed a pre-existing `events.jsonl` with a bogus
    // event that would fail deserialization, run preflight in
    // `fresh_audit` mode, and assert: (a) the authoritative log is
    // byte-identical to the seed afterward, and (b) the preflight trace
    // lands in the session-isolated sidecar instead.
    #[test]
    #[serial]
    fn run_preflight_fresh_audit_ignores_prior_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = true;

            // Seed a sentinel `events.jsonl`. If fresh-audit touches it at
            // all (read or append), we'll see drift after the run.
            let authoritative_events = td.path().join(".shipper").join("events.jsonl");
            std::fs::create_dir_all(authoritative_events.parent().unwrap()).expect("mkdir");
            let sentinel = "{\"sentinel\":\"do not touch\"}\n";
            std::fs::write(&authoritative_events, sentinel).expect("seed events");

            // Seed a state.json too â€” fresh-audit must not produce or
            // overwrite a publish state file as a side-effect.
            let state_json = td.path().join(".shipper").join("state.json");
            std::fs::write(&state_json, "{\"sentinel\":\"state\"}").expect("seed state");

            let mut reporter = CollectingReporter::default();
            let rep = super::run_preflight_with_options(
                &ws,
                &opts,
                &mut reporter,
                super::PreflightRunOptions { fresh_audit: true },
            )
            .expect("preflight");
            assert_eq!(rep.packages.len(), 1);

            // Authoritative events.jsonl is byte-identical to the seed.
            let after = std::fs::read_to_string(&authoritative_events).expect("read events");
            assert_eq!(
                after, sentinel,
                "fresh_audit must NOT append or rewrite events.jsonl"
            );

            // state.json untouched.
            let state_after = std::fs::read_to_string(&state_json).expect("read state");
            assert_eq!(
                state_after, "{\"sentinel\":\"state\"}",
                "fresh_audit must NOT write publish state"
            );

            // Sidecar carries the full preflight trace.
            let sidecar =
                crate::state::events::preflight_only_events_paths(&td.path().join(".shipper"))
                    .expect("discover sidecars")
                    .into_iter()
                    .next()
                    .expect("fresh audit sidecar");
            assert!(sidecar.exists(), "sidecar must be written");
            let side_log =
                crate::state::events::EventLog::read_from_file(&sidecar).expect("read sidecar");
            let events = side_log.all_events();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightStarted)),
                "sidecar must contain PreflightStarted"
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PreflightComplete { .. })),
                "sidecar must contain PreflightComplete"
            );

            server.join();
        });
    }

    // #100 â€” fresh-audit reflects *current* workspace state, not a
    // cached prior result. We run preflight twice in fresh_audit mode:
    // first against a registry that treats the crate as new (NotProven),
    // then against the same workspace but with a server that returns
    // 200 + owners â€” ownership verified, so Proven. If fresh_audit were
    // cached/reused, the second run would incorrectly still report
    // NotProven; it must reflect the changed registry reality.
    //
    // This also exercises the "prior events.jsonl NOT appended to"
    // invariant under repeated runs: the authoritative log stays empty
    // across both invocations.
    #[test]
    #[serial]
    fn run_preflight_with_dirty_git_fresh_audit() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            // Run 1: new crate (404 on crate lookup) â†’ NotProven (no
            // ownership possible for new crates when strict=false and
            // token present).
            let server1 = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws1 = planned_workspace(td.path(), server1.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = false;
            opts.strict_ownership = false;

            let mut reporter = CollectingReporter::default();
            let rep1 = super::run_preflight_with_options(
                &ws1,
                &opts,
                &mut reporter,
                super::PreflightRunOptions { fresh_audit: true },
            )
            .expect("preflight 1");
            assert_eq!(rep1.finishability, Finishability::NotProven);
            assert!(rep1.packages[0].is_new_crate);
            server1.join();

            // Authoritative events.jsonl must not exist after a fresh
            // audit (no state persistence).
            let authoritative_events = td.path().join(".shipper").join("events.jsonl");
            assert!(
                !authoritative_events.exists(),
                "fresh_audit must not create events.jsonl; found {}",
                authoritative_events.display()
            );

            // Run 2: established crate with confirmed owners â†’ Proven.
            // Same workspace, different registry reality. Fresh audit
            // must pick up the new state, not cache the previous run.
            let server2 = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(200, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo/owners".to_string(),
                        vec![(
                            200,
                            r#"{"users":[{"id":1,"login":"alice","name":"Alice"}]}"#.to_string(),
                        )],
                    ),
                ]),
                3,
            );

            let ws2 = planned_workspace(td.path(), server2.base_url.clone());
            let rep2 = super::run_preflight_with_options(
                &ws2,
                &opts,
                &mut reporter,
                super::PreflightRunOptions { fresh_audit: true },
            )
            .expect("preflight 2");
            assert_eq!(
                rep2.finishability,
                Finishability::Proven,
                "fresh_audit must reflect current registry state, not cached"
            );
            assert!(!rep2.packages[0].is_new_crate);
            server2.join();

            // Still no authoritative events.jsonl after two fresh runs.
            assert!(
                !authoritative_events.exists(),
                "authoritative events.jsonl must remain absent across fresh audits"
            );

            // Each fresh audit gets its own sidecar so repeated or concurrent
            // runs do not overwrite one another.
            let sidecars =
                crate::state::events::preflight_only_events_paths(&td.path().join(".shipper"))
                    .expect("discover sidecars");
            assert_eq!(
                sidecars.len(),
                2,
                "each fresh audit should create a new sidecar"
            );

            for sidecar in sidecars {
                let side_log =
                    crate::state::events::EventLog::read_from_file(&sidecar).expect("read sidecar");
                let started_count = side_log
                    .all_events()
                    .iter()
                    .filter(|e| matches!(e.event_type, EventType::PreflightStarted))
                    .count();
                assert_eq!(
                    started_count, 1,
                    "each sidecar should contain exactly one fresh-audit session"
                );
            }
        });
    }

    #[test]
    #[serial]
    fn run_preflight_detects_trusted_publishing_auth_type() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", None::<String>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            (
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                Some("https://example.invalid/oidc".to_string()),
            ),
            (
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                Some("oidc-token".to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = true;

            let mut reporter = CollectingReporter::default();
            let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            assert!(!report.token_detected);
            assert_eq!(
                report.packages[0].auth_type,
                Some(crate::types::AuthType::TrustedPublishing)
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_warns_when_token_auth_overrides_oidc() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            (
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                Some("https://example.invalid/oidc".to_string()),
            ),
            (
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                Some("oidc-token".to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = true;

            let mut reporter = CollectingReporter::default();
            let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            assert!(report.token_detected);
            assert_eq!(report.packages[0].auth_type, Some(AuthType::Token));
            let warnings = reporter.warns.join("\n");
            assert!(
                warnings.contains("Trusted Publishing OIDC environment is present"),
                "warnings: {warnings}"
            );
            assert!(
                !warnings.contains("token-abc"),
                "warnings must not expose token values: {warnings}"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_does_not_warn_for_plain_token_auth() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_HOME",
                Some(td.path().to_str().expect("utf8").to_string()),
            ),
            ("CARGO_REGISTRY_TOKEN", Some("token-abc".to_string())),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
            ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<String>),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<String>),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = true;
            opts.skip_ownership_check = true;

            let mut reporter = CollectingReporter::default();
            let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

            assert!(report.token_detected);
            assert_eq!(report.packages[0].auth_type, Some(AuthType::Token));
            assert!(
                reporter
                    .warns
                    .iter()
                    .all(|warning| !warning.contains("Trusted Publishing OIDC environment")),
                "warnings: {:?}",
                reporter.warns
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_preflight_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_GIT_CLEAN", Some("1".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/demo".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = false;
            opts.skip_ownership_check = true;

            let mut reporter = CollectingReporter::default();
            let rep = run_preflight(&ws, &opts, &mut reporter).expect("preflight");
            assert_eq!(rep.packages.len(), 1);
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_skips_when_version_already_exists() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        let cargo_log = td.path().join("cargo-args.log");
        env_vars.push((
            "SHIPPER_CARGO_ARGS_LOG",
            Some(cargo_log.to_string_lossy().to_string()),
        ));
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert_eq!(receipt.packages.len(), 1);
            assert!(matches!(
                receipt.packages[0].state,
                PackageState::Skipped { .. }
            ));

            let state_dir = td.path().join(".shipper");
            assert!(state::state_path(&state_dir).exists());
            assert!(state::receipt_path(&state_dir).exists());
            server.join();
        });
    }

    /// Regression for #125: resume encountering a `Published` state in
    /// state.json must emit a `PackageSkipped` event, not silently move
    /// on. events.jsonl is the authoritative audit log; a silent skip
    /// makes "did resume look at this package at all?" unanswerable from
    /// the log alone.
    #[test]
    #[serial]
    fn resume_emits_package_skipped_event_for_already_published_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");

            // Seed: existing state says demo@0.1.0 is Published. Resume
            // should recognize it and skip â€” but crucially, emit the event.
            let mut packages = BTreeMap::new();
            packages.insert(
                "demo@0.1.0".to_string(),
                PackageProgress {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    last_updated_at: Utc::now(),
                },
            );
            let seeded = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: ws.plan.plan_id.clone(),
                registry: ws.plan.registry.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attempt_history: Vec::new(),
                packages,
            };
            state::save_state(&state_dir, &seeded).expect("seed state");

            let opts = default_opts(PathBuf::from(".shipper"));
            let mut reporter = CollectingReporter::default();
            let _receipt = run_publish(&ws, &opts, &mut reporter).expect("publish resumes");

            // Read events.jsonl and assert a PackageSkipped event exists.
            let events_path = events::events_path(&state_dir);
            let raw = std::fs::read_to_string(&events_path).expect("events.jsonl");
            let skipped_count = raw
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter(|l| l.contains(r#""type":"package_skipped""#))
                .count();
            assert!(
                skipped_count >= 1,
                "expected at least one PackageSkipped event for the already-Published package; \
                 events.jsonl was:\n{raw}"
            );
        });
    }

    /// Regression for #126: when resume encounters a `Failed` state and
    /// the registry confirms the version is visible, `state.json` must
    /// transition that package to a terminal state for downstream tooling.
    /// This path now lands on `Published` after reconciliation confirms
    /// visibility instead of preserving `Failed`.
    #[test]
    #[serial]
    fn resume_from_failed_ambiguous_updates_state_to_published_when_registry_visible() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo-args.log");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.push((
            "SHIPPER_CARGO_ARGS_LOG",
            Some(cargo_log.to_string_lossy().to_string()),
        ));
        temp_env::with_vars(env_vars, || {
            // Registry returns 200 for the version check â€” the crate IS
            // visible, even though run 1 left us with Failed/Ambiguous.
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");

            let mut packages = BTreeMap::new();
            packages.insert(
                "demo@0.1.0".to_string(),
                PackageProgress {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Failed {
                        class: ErrorClass::Ambiguous,
                        message: "prior run: publish outcome ambiguous".to_string(),
                    },
                    last_updated_at: Utc::now(),
                },
            );
            let seeded = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: ws.plan.plan_id.clone(),
                registry: ws.plan.registry.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attempt_history: Vec::new(),
                packages,
            };
            state::save_state(&state_dir, &seeded).expect("seed state");

            let opts = default_opts(PathBuf::from(".shipper"));
            let mut reporter = CollectingReporter::default();
            let _receipt = run_publish(&ws, &opts, &mut reporter).expect("publish resumes");

            // State.json on disk must now say Published, not Failed.
            let reloaded = state::load_state(&state_dir)
                .expect("load")
                .expect("state exists");
            let pkg_state = &reloaded
                .packages
                .get("demo@0.1.0")
                .expect("package in state")
                .state;
            assert!(
                matches!(pkg_state, PackageState::Published),
                "expected state.json to show Published after resume reconciled against registry; got {pkg_state:?}"
            );
            let cargo_called = cargo_log.exists()
                && fs::read_to_string(&cargo_log)
                    .unwrap_or_default()
                    .lines()
                    .any(|line| !line.trim().is_empty());
            assert!(
                !cargo_called,
                "resume should not invoke cargo when Failed(Ambiguous) state is reconciled via registry visibility"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn sequential_ambiguous_publish_reconciles_to_published_without_retry() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo-calls.log");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_STDOUT", Some(String::new())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.max_attempts = 2;
            opts.readiness.enabled = false;

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            assert!(matches!(receipt.packages[0].state, PackageState::Published));
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|msg| msg.contains("reconciliation outcome: Published")
                        && msg.contains("without retry")),
                "infos: {:?}",
                reporter.infos
            );

            let cargo_invocations = std::fs::read_to_string(&cargo_log)
                .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0);
            assert_eq!(cargo_invocations, 1, "must not retry after Published");

            let events_path = events::events_path(&state_dir);
            let events = events::EventLog::read_from_file(&events_path).expect("events");
            assert!(
                events
                    .all_events()
                    .iter()
                    .any(|e| { matches!(e.event_type, EventType::PublishReconciling { .. }) })
            );
            assert!(events.all_events().iter().any(|e| {
                matches!(
                    &e.event_type,
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::Published { .. }
                    }
                )
            }));

            server.join();
        });
    }

    #[test]
    #[serial]
    fn sequential_ambiguous_publish_reconciles_to_not_published_then_retries_once() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        write_fake_cargo_ambiguous_then_permanent(&bin);
        let cargo_log = td.path().join("cargo-calls.log");
        let cargo_count = td.path().join("cargo-count.txt");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().to_string()),
            ),
            (
                "SHIPPER_CARGO_COUNT_FILE",
                Some(cargo_count.to_string_lossy().to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![
                        (404, "{}".to_string()),
                        (404, "{}".to_string()),
                        (404, "{}".to_string()),
                    ],
                )]),
                3,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.max_attempts = 2;
            opts.readiness.enabled = false;

            let mut reporter = CollectingReporter::default();
            let err = run_publish(&ws, &opts, &mut reporter).expect_err("publish should fail");
            let msg = format!("{err:#}");
            assert!(msg.contains("permanent failure"), "err: {msg}");
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|msg| msg.contains("reconciliation outcome: NotPublished")
                        && msg.contains("retry under publish policy")),
                "infos: {:?}",
                reporter.infos
            );

            let cargo_invocations = std::fs::read_to_string(&cargo_log)
                .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0);
            assert_eq!(
                cargo_invocations, 2,
                "NotPublished reconciliation should permit one retry"
            );

            let events_path = events::events_path(&state_dir);
            let events = events::EventLog::read_from_file(&events_path).expect("events");
            assert!(events.all_events().iter().any(|e| {
                matches!(
                    &e.event_type,
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::NotPublished { .. }
                    }
                )
            }));

            server.join();
        });
    }

    fn assert_sequential_not_published_controlled_stop(
        max_attempts: u32,
        expected_registry_requests: usize,
    ) {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo-calls.log");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_STDOUT", Some(String::new())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    std::iter::repeat_n((404, "{}".to_string()), expected_registry_requests)
                        .collect(),
                )]),
                expected_registry_requests,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.max_attempts = max_attempts;
            opts.readiness.enabled = false;

            let mut reporter = CollectingReporter::default();
            let error = run_publish(&ws, &opts, &mut reporter).expect_err("controlled stop");
            assert!(
                error
                    .downcast_ref::<PublishRecoverableStopError>()
                    .is_some(),
                "typed stop identity lost: {error:#}"
            );
            let state = state::load_state(&state_dir)
                .expect("load state")
                .expect("state exists");
            assert!(matches!(
                state.packages.get("demo@0.1.0").map(|p| &p.state),
                Some(PackageState::Failed {
                    class: ErrorClass::Retryable,
                    ..
                })
            ));
            assert!(!state::receipt_path(&state_dir).exists());
            let event_log =
                events::EventLog::read_from_file(&events::events_path(&state_dir)).expect("events");
            assert!(matches!(
                event_log.all_events().last().map(|event| &event.event_type),
                Some(EventType::ExecutionStopped {
                    reason: shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted
                })
            ));
            let reconciliation: shipper_types::ReconciliationReport = serde_json::from_str(
                &std::fs::read_to_string(state::reconciliation_path(&state_dir))
                    .expect("reconciliation report"),
            )
            .expect("parse reconciliation report");
            crate::state::consistency::verify_controlled_stop_consistency(
                &events::events_path(&state_dir),
                &state,
                Some(&reconciliation),
            )
            .expect("controlled stop is consistent");
            let cargo_invocations = std::fs::read_to_string(&cargo_log)
                .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
                .unwrap_or(0);
            assert_eq!(
                cargo_invocations, max_attempts as usize,
                "must dispatch Cargo exactly once per allowed attempt"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn sequential_not_published_budget_exhaustion_records_controlled_stop() {
        assert_sequential_not_published_controlled_stop(1, 2);
    }

    #[test]
    #[serial]
    fn sequential_repeated_not_published_exhaustion_records_one_controlled_stop() {
        assert_sequential_not_published_controlled_stop(2, 3);
    }

    #[test]
    #[serial]
    fn resume_consumes_consistent_controlled_stop_and_finalizes_receipt() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo-calls.log");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![
                    (404, "{}".to_string()),
                    (404, "{}".to_string()),
                    // Resume checks retained retryable work before another upload.
                    (404, "{}".to_string()),
                    (200, "{}".to_string()),
                ],
            )]),
            4,
        );
        let ws = planned_workspace(td.path(), server.base_url.clone());
        let state_dir = td.path().join(".shipper");
        let mut opts = default_opts(state_dir.clone());
        opts.max_attempts = 1;
        opts.readiness.enabled = false;

        let mut first_env = fake_program_env_vars(&bin);
        first_env.extend([
            ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_STDOUT", Some(String::new())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().to_string()),
            ),
        ]);
        temp_env::with_vars(first_env, || {
            let mut reporter = CollectingReporter::default();
            run_publish(&ws, &opts, &mut reporter).expect_err("controlled stop");
        });

        let reconciliation_path = state::reconciliation_path(&state_dir);
        let reconciliation_before =
            std::fs::read(&reconciliation_path).expect("reconciliation before marker negatives");
        let mut report: crate::types::ReconciliationReport =
            serde_json::from_slice(&reconciliation_before)
                .expect("parse reconciliation for marker negatives");
        let mut other = report
            .records
            .first()
            .cloned()
            .expect("controlled-stop reconciliation record");
        other.package = "other@1.0.0".to_string();
        other.name = "other".to_string();
        other.version = "1.0.0".to_string();
        other.outcome = ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 1,
            reason: "other package remains ambiguous".to_string(),
        };
        other.operator_action = crate::types::ReconciliationOperatorAction::OperatorActionRequired;
        report.records.push(other);
        state::write_reconciliation_report(&state_dir, &report)
            .expect("write mixed reconciliation for marker negatives");

        let controlled_events_path = events::events_path(&state_dir);
        let complete_events =
            std::fs::read_to_string(&controlled_events_path).expect("complete events");
        let mut event_lines = complete_events.lines().collect::<Vec<_>>();
        let removed = event_lines.pop().expect("controlled-stop marker line");
        assert!(removed.contains("execution_stopped"));
        std::fs::write(
            &controlled_events_path,
            format!("{}\n", event_lines.join("\n")),
        )
        .expect("remove marker for negative");
        let markerless_events_before =
            std::fs::read(&controlled_events_path).expect("markerless events before rejection");
        let state_before_rejection =
            std::fs::read(state::state_path(&state_dir)).expect("state before rejection");
        let cargo_before_rejection = std::fs::read(&cargo_log).expect("cargo log before rejection");
        let markerless_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("markerless derived posture must be rejected");
        assert!(
            format!("{markerless_error:#}").contains("lacks the current controlled-stop marker")
        );
        assert_eq!(
            std::fs::read(state::state_path(&state_dir)).expect("state after rejection"),
            state_before_rejection,
            "rejected resume must not mutate state"
        );
        assert_eq!(
            std::fs::read(&controlled_events_path).expect("events after markerless rejection"),
            markerless_events_before,
            "markerless rejection must not mutate events"
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo log after rejection"),
            cargo_before_rejection,
            "rejected resume must not dispatch Cargo"
        );
        std::fs::write(&controlled_events_path, complete_events)
            .expect("restore controlled-stop marker");

        let complete_events =
            std::fs::read_to_string(&controlled_events_path).expect("restored events");
        let mut reordered = complete_events.lines().collect::<Vec<_>>();
        let marker = reordered.pop().expect("controlled-stop marker");
        reordered.insert(1, marker);
        std::fs::write(
            &controlled_events_path,
            format!("{}\n", reordered.join("\n")),
        )
        .expect("write reordered marker");
        let reordered_events_before =
            std::fs::read(&controlled_events_path).expect("reordered events before rejection");
        let reordered_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("non-terminal marker must be rejected");
        assert!(
            format!("{reordered_error:#}").contains("controlled-stop"),
            "unexpected reordered-marker error: {reordered_error:#}"
        );
        assert_eq!(
            std::fs::read(state::state_path(&state_dir)).expect("state after reordered rejection"),
            state_before_rejection,
            "reordered marker rejection must not mutate state"
        );
        assert_eq!(
            std::fs::read(&controlled_events_path).expect("events after reordered rejection"),
            reordered_events_before,
            "reordered marker rejection must not mutate events"
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo log after reordered rejection"),
            cargo_before_rejection,
            "reordered marker rejection must not dispatch Cargo"
        );
        std::fs::write(&controlled_events_path, &complete_events)
            .expect("restore terminal marker after reordered negative");
        std::fs::write(&reconciliation_path, &reconciliation_before)
            .expect("restore reconciliation after marker negatives");

        let mut corrupt_identity: crate::types::ReconciliationReport =
            serde_json::from_slice(&reconciliation_before)
                .expect("parse reconciliation for identity negative");
        let mut duplicate = corrupt_identity
            .records
            .last()
            .cloned()
            .expect("controlled-stop reconciliation record");
        duplicate.name = "forged-package-name".to_string();
        corrupt_identity.records.push(duplicate);
        state::write_reconciliation_report(&state_dir, &corrupt_identity)
            .expect("write corrupt duplicate identity");
        let stopped_state = state::load_state(&state_dir)
            .expect("load stopped state")
            .expect("stopped state exists");
        assert!(
            !controlled_stop_evidence_consistent(
                &state_dir,
                &controlled_events_path,
                &stopped_state
            ),
            "corrupt duplicate identity must not authorize the initial stop envelope or durable status"
        );
        let corrupt_state_before =
            std::fs::read(state::state_path(&state_dir)).expect("state before identity rejection");
        let corrupt_events_before =
            std::fs::read(&controlled_events_path).expect("events before identity rejection");
        let corrupt_cargo_before =
            std::fs::read(&cargo_log).expect("cargo log before identity rejection");
        let corrupt_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("corrupt duplicate identity must reject resume");
        assert!(
            format!("{corrupt_error:#}").contains("reconciliation package identity disagrees"),
            "unexpected corrupt-identity error: {corrupt_error:#}"
        );
        assert_eq!(
            std::fs::read(state::state_path(&state_dir)).expect("state after identity rejection"),
            corrupt_state_before,
            "identity rejection must not mutate state"
        );
        assert_eq!(
            std::fs::read(&controlled_events_path).expect("events after identity rejection"),
            corrupt_events_before,
            "identity rejection must not mutate events"
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo log after identity rejection"),
            corrupt_cargo_before,
            "identity rejection must not dispatch Cargo"
        );
        std::fs::write(&reconciliation_path, &reconciliation_before)
            .expect("restore reconciliation after identity negative");

        let reconciliation_before =
            std::fs::read(&reconciliation_path).expect("reconciliation before cross-run negative");
        let mut cross_run: serde_json::Value = serde_json::from_slice(&reconciliation_before)
            .expect("parse reconciliation for cross-run negative");
        cross_run["plan_id"] = serde_json::Value::String("different-run-plan".to_string());
        std::fs::write(
            &reconciliation_path,
            serde_json::to_vec_pretty(&cross_run).expect("serialize cross-run reconciliation"),
        )
        .expect("write cross-run reconciliation");
        let _cross_run_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("cross-run reconciliation must be rejected");
        assert_eq!(
            std::fs::read(state::state_path(&state_dir)).expect("state after cross-run rejection"),
            state_before_rejection,
            "cross-run rejection must not mutate state"
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo log after cross-run rejection"),
            cargo_before_rejection,
            "cross-run rejection must not dispatch Cargo"
        );
        std::fs::write(&reconciliation_path, reconciliation_before)
            .expect("restore reconciliation after cross-run negative");

        #[cfg(target_os = "linux")]
        {
            let live_lock = crate::lock::LockFile::acquire(&state_dir, Some(&ws.workspace_root))
                .expect("acquire matching live lock");
            live_lock
                .set_plan_id(&ws.plan.plan_id)
                .expect("set matching live-lock plan");
            let live_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
                .expect_err("matching live publisher must block resume");
            assert!(format!("{live_error:#}").contains("still live"));
            assert_eq!(
                std::fs::read(state::state_path(&state_dir))
                    .expect("state after matching live-lock rejection"),
                state_before_rejection,
                "live-lock rejection must not mutate state"
            );
            assert_eq!(
                std::fs::read(&cargo_log).expect("cargo log after live-lock rejection"),
                cargo_before_rejection,
                "live-lock rejection must not dispatch Cargo"
            );
            drop(live_lock);
        }

        let budget_state_before =
            std::fs::read(state::state_path(&state_dir)).expect("state before budget rejection");
        let budget_events_before =
            std::fs::read(&controlled_events_path).expect("events before budget rejection");
        let budget_reconciliation_before =
            std::fs::read(&reconciliation_path).expect("reconciliation before budget rejection");
        let budget_cargo_before =
            std::fs::read(&cargo_log).expect("cargo log before budget rejection");
        let mut invalid_target_opts = opts.clone();
        invalid_target_opts.resume_from = Some("missing-package".to_string());
        let invalid_target_error = run_resume(
            &ws,
            &invalid_target_opts,
            &mut CollectingReporter::default(),
        )
        .expect_err("invalid resume target must precede budget admission");
        assert!(
            format!("{invalid_target_error:#}")
                .contains("resume-from package 'missing-package' not found")
        );
        assert_eq!(
            std::fs::read(state::state_path(&state_dir))
                .expect("state after invalid-target rejection"),
            budget_state_before
        );
        assert_eq!(
            std::fs::read(&controlled_events_path).expect("events after invalid-target rejection"),
            budget_events_before
        );
        assert_eq!(
            std::fs::read(&reconciliation_path)
                .expect("reconciliation after invalid-target rejection"),
            budget_reconciliation_before
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo after invalid-target rejection"),
            budget_cargo_before
        );
        let budget_error = run_resume(&ws, &opts, &mut CollectingReporter::default())
            .expect_err("same cumulative retry ceiling must reject before mutation");
        assert_eq!(
            classify_resume_retry_budget(&budget_error),
            Some(ResumeRetryBudgetClassification {
                package: "demo@0.1.0".to_string(),
                current_attempts: 1,
                requested_max_attempts: 1,
                minimum_max_attempts: Some(2),
            })
        );
        assert_eq!(
            std::fs::read(state::state_path(&state_dir)).expect("state after budget rejection"),
            budget_state_before,
            "budget rejection must not mutate state"
        );
        assert_eq!(
            std::fs::read(&controlled_events_path).expect("events after budget rejection"),
            budget_events_before,
            "budget rejection must not append a new run segment"
        );
        assert_eq!(
            std::fs::read(&reconciliation_path).expect("reconciliation after budget rejection"),
            budget_reconciliation_before,
            "budget rejection must not mutate reconciliation evidence"
        );
        assert_eq!(
            std::fs::read(&cargo_log).expect("cargo log after budget rejection"),
            budget_cargo_before,
            "budget rejection must not dispatch Cargo"
        );
        assert!(!state::receipt_path(&state_dir).exists());
        let stopped_state = state::load_state(&state_dir)
            .expect("load state after budget rejection")
            .expect("stopped state exists after budget rejection");
        crate::state::consistency::verify_controlled_stop_consistency(
            &controlled_events_path,
            &stopped_state,
            Some(
                &serde_json::from_slice(&budget_reconciliation_before)
                    .expect("parse reconciliation after budget rejection"),
            ),
        )
        .expect("budget rejection must preserve controlled-stop authorization");

        let mut resume_env = fake_program_env_vars(&bin);
        resume_env.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().to_string()),
            ),
        ]);
        temp_env::with_vars(resume_env, || {
            let mut reporter = CollectingReporter::default();
            let mut resume_opts = opts.clone();
            resume_opts.max_attempts = 2;
            let receipt = run_resume(&ws, &resume_opts, &mut reporter).expect("resume succeeds");
            assert!(matches!(receipt.execution_result, ExecutionResult::Success));
        });

        let final_state = state::load_state(&state_dir)
            .expect("load final state")
            .expect("final state exists");
        assert!(matches!(
            final_state
                .packages
                .get("demo@0.1.0")
                .map(|progress| &progress.state),
            Some(PackageState::Published)
        ));
        assert!(state::receipt_path(&state_dir).exists());
        let events =
            events::EventLog::read_from_file(&events::events_path(&state_dir)).expect("events");
        assert_eq!(
            events
                .all_events()
                .iter()
                .filter(|event| matches!(event.event_type, EventType::ExecutionStarted))
                .count(),
            2,
            "resume must create a new run segment"
        );
        let cargo_invocations = std::fs::read_to_string(&cargo_log)
            .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
            .unwrap_or(0);
        assert_eq!(cargo_invocations, 2, "one initial attempt and one resume");
        server.join();
    }

    #[test]
    #[serial]
    fn enabled_reconciliation_still_unknown_stops_publish_and_resume_in_both_modes() {
        for (parallel, method, index_response) in [
            (false, crate::types::ReadinessMethod::Api, "unused index"),
            (true, crate::types::ReadinessMethod::Api, "unused index"),
            (
                false,
                crate::types::ReadinessMethod::Index,
                "secret-response-sentinel",
            ),
            (
                false,
                crate::types::ReadinessMethod::Index,
                r#"{"vers":""}"#,
            ),
            (
                true,
                crate::types::ReadinessMethod::Index,
                r#"{"vers":"not-a-version"}"#,
            ),
        ] {
            let td = tempdir().expect("tempdir");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = td.path().join("cargo-calls.log");
            let mut env_vars = fake_program_env_vars(&bin);
            env_vars.extend([
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                ("SHIPPER_CARGO_STDERR", Some(String::new())),
                ("SHIPPER_CARGO_STDOUT", Some(String::new())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().to_string()),
                ),
            ]);
            temp_env::with_vars(env_vars, || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".into(),
                            vec![(404, "{}".into()), (500, "secret-response-sentinel".into())],
                        ),
                        ("/de/mo/demo".into(), vec![(200, index_response.into())]),
                    ]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let state_dir = td.path().join(".shipper");
                let mut opts = default_opts(state_dir.clone());
                opts.parallel.enabled = parallel;
                opts.max_attempts = 2;
                opts.readiness.enabled = true;
                opts.readiness.method = method;
                opts.readiness.max_total_wait = Duration::ZERO;
                opts.readiness.initial_delay = Duration::ZERO;

                for resume in [false, true] {
                    let mut reporter = CollectingReporter::default();
                    let result = if resume {
                        run_resume(&ws, &opts, &mut reporter)
                    } else {
                        run_publish(&ws, &opts, &mut reporter)
                    };
                    let error = result.expect_err("unknown observation must stop");
                    assert!(
                        error.downcast_ref::<PublishStillUnknownError>().is_some(),
                        "{error:#}"
                    );
                    assert!(
                        reporter.errors.iter().any(|message| {
                            message.contains("reconciliation outcome: StillUnknown")
                                && message.contains("stop before blind retry")
                        }),
                        "{:?}",
                        reporter.errors
                    );
                    assert!(!format!("{error:#}").contains("secret-response-sentinel"));
                    let cargo_invocations = std::fs::read_to_string(&cargo_log)
                        .expect("cargo log")
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .count();
                    assert_eq!(
                        cargo_invocations, 1,
                        "neither scheduler nor resume may blind-retry"
                    );
                    let retained = state::load_state(&state_dir)
                        .expect("load state")
                        .expect("state");
                    let package = retained.packages.get("demo@0.1.0").expect("package");
                    assert!(matches!(package.state, PackageState::Ambiguous { .. }));
                    assert_eq!(package.attempts, 1);
                    assert!(
                        !state::receipt_path(&state_dir).exists(),
                        "no success receipt"
                    );
                    let report: crate::types::ReconciliationReport = serde_json::from_slice(
                        &std::fs::read(state::reconciliation_path(&state_dir)).expect("report"),
                    )
                    .expect("report JSON");
                    assert!(matches!(
                        report.records.last().expect("record").outcome,
                        ReconciliationOutcome::StillUnknown { attempts: 1, .. }
                    ));
                    let log = events::EventLog::read_from_file(&events::events_path(&state_dir))
                        .expect("events");
                    assert!(log.all_events().iter().any(|event| matches!(
                        event.event_type,
                        EventType::PublishReconciled {
                            outcome: ReconciliationOutcome::StillUnknown { attempts: 1, .. }
                        }
                    )));
                    assert!(!log.all_events().iter().any(|event| matches!(
                        event.event_type,
                        EventType::PublishReconciled {
                            outcome: ReconciliationOutcome::NotPublished { .. }
                        }
                    )));
                }
                assert_eq!(server.seen.lock().expect("requests").len(), 3);
                server.join();
            });
        }
    }

    #[test]
    #[serial]
    fn run_publish_checks_git_when_allow_dirty_is_false() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_GIT_CLEAN", Some("1".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = false;

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert_eq!(receipt.packages.len(), 1);
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_sequential_webhooks_send_started_and_completed_once() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);

        let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
        let webhook_url = format!("http://{}", webhook_server.server_addr());
        let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
        let webhook_received_clone = Arc::clone(&webhook_received);
        let webhook_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let webhook_done_clone = std::sync::Arc::clone(&webhook_done);

        let webhook_handle = std::thread::spawn(move || {
            while !webhook_done_clone.load(std::sync::atomic::Ordering::Acquire) {
                if let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_millis(100)) {
                    let mut body = Vec::new();
                    let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                    webhook_received_clone
                        .lock()
                        .expect("webhook lock")
                        .push(String::from_utf8_lossy(&body).to_string());
                    req.respond(Response::from_string("ok")).expect("respond");
                    if webhook_received_clone.lock().expect("webhook lock").len() >= 2 {
                        break;
                    }
                }
            }
        });

        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_STDOUT", Some("cargo publish".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            let registry_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), registry_server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = false;
            opts.webhook = crate::webhook::WebhookConfig {
                url: webhook_url.clone(),
                ..Default::default()
            };

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert_eq!(receipt.packages.len(), 1);
            let event_log =
                events::EventLog::read_from_file(&events::events_path(&state_dir)).expect("events");
            assert!(
                event_log
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::ExecutionFinished { .. })),
                "execution did not finish"
            );

            std::thread::sleep(Duration::from_secs(2));
            webhook_done.store(true, std::sync::atomic::Ordering::Release);
            webhook_handle.join().expect("webhook thread");
            registry_server.join();
        });

        let received = webhook_received.lock().unwrap();
        let mut started_count = 0usize;
        let mut completed_count = 0usize;
        for payload in received.iter() {
            let parsed: serde_json::Value =
                serde_json::from_str(payload).expect("webhook payload should be JSON");

            let legacy_event = parsed
                .get("extra")
                .and_then(|extra| extra.get("legacy"))
                .and_then(|legacy| legacy.get("event"))
                .and_then(|event| event.as_str());
            let legacy_event = legacy_event.or_else(|| {
                parsed
                    .get("legacy")
                    .and_then(|legacy| legacy.get("event"))
                    .and_then(|event| event.as_str())
            });
            if matches!(legacy_event, Some("publish_started")) {
                started_count += 1;
            }
            if matches!(legacy_event, Some("publish_completed")) {
                completed_count += 1;
            }
        }
        assert_eq!(
            received.len(),
            2,
            "expected started and completed webhook events; got {received:?}"
        );
        assert_eq!(
            started_count, 1,
            "expected exactly one started webhook; got {started_count} started and {completed_count} completed; payloads: {received:?}"
        );
        assert_eq!(
            completed_count, 1,
            "expected exactly one completed webhook; got {started_count} started and {completed_count} completed; payloads: {received:?}"
        );
    }

    #[test]
    #[serial]
    fn run_publish_parallel_webhooks_send_started_and_completed_once() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);

        let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
        let webhook_url = format!("http://{}", webhook_server.server_addr());
        let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
        let webhook_received_clone = Arc::clone(&webhook_received);
        let webhook_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let webhook_done_clone = std::sync::Arc::clone(&webhook_done);

        let webhook_handle = std::thread::spawn(move || {
            while !webhook_done_clone.load(std::sync::atomic::Ordering::Acquire) {
                if let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_millis(100)) {
                    let mut body = Vec::new();
                    let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                    webhook_received_clone
                        .lock()
                        .expect("webhook lock")
                        .push(String::from_utf8_lossy(&body).to_string());
                    req.respond(Response::from_string("ok")).expect("respond");
                    if webhook_received_clone.lock().expect("webhook lock").len() >= 2 {
                        break;
                    }
                }
            }
        });

        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_STDOUT", Some("cargo publish".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            let registry_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), registry_server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir.clone());
            opts.parallel.enabled = true;
            opts.webhook = crate::webhook::WebhookConfig {
                url: webhook_url.clone(),
                ..Default::default()
            };

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert_eq!(receipt.packages.len(), 1);
            let event_log =
                events::EventLog::read_from_file(&events::events_path(&state_dir)).expect("events");
            assert!(
                event_log
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::ExecutionFinished { .. })),
                "execution did not finish"
            );

            std::thread::sleep(Duration::from_secs(2));
            webhook_done.store(true, std::sync::atomic::Ordering::Release);
            webhook_handle.join().expect("webhook thread");
            registry_server.join();
        });

        let received = webhook_received.lock().unwrap();
        let mut started_count = 0usize;
        let mut completed_count = 0usize;
        for payload in received.iter() {
            let parsed: serde_json::Value =
                serde_json::from_str(payload).expect("webhook payload should be JSON");

            let legacy_event = parsed
                .get("extra")
                .and_then(|extra| extra.get("legacy"))
                .and_then(|legacy| legacy.get("event"))
                .and_then(|event| event.as_str());
            let legacy_event = legacy_event.or_else(|| {
                parsed
                    .get("legacy")
                    .and_then(|legacy| legacy.get("event"))
                    .and_then(|event| event.as_str())
            });
            if matches!(legacy_event, Some("publish_started")) {
                started_count += 1;
            }
            if matches!(legacy_event, Some("publish_completed")) {
                completed_count += 1;
            }
        }
        assert_eq!(
            received.len(),
            2,
            "expected started and completed webhook events; got {received:?}"
        );
        assert_eq!(
            started_count, 1,
            "expected exactly one started webhook; got {started_count} started and {completed_count} completed; payloads: {received:?}"
        );
        assert_eq!(
            completed_count, 1,
            "expected exactly one completed webhook; got {started_count} started and {completed_count} completed; payloads: {received:?}"
        );
    }

    #[test]
    #[serial]
    fn run_publish_parallel_resume_from_webhook_counts() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);

        let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
        let webhook_url = format!("http://{}", webhook_server.server_addr());
        let webhook_received = Arc::new(Mutex::new(Vec::<String>::new()));
        let webhook_received_clone = Arc::clone(&webhook_received);
        let webhook_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let webhook_done_clone = Arc::clone(&webhook_done);
        let webhook_handle = thread::spawn(move || {
            while !webhook_done_clone.load(std::sync::atomic::Ordering::Acquire) {
                if let Ok(Some(mut req)) = webhook_server.recv_timeout(Duration::from_millis(100)) {
                    let mut body = Vec::new();
                    let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                    webhook_received_clone
                        .lock()
                        .expect("webhook lock")
                        .push(String::from_utf8_lossy(&body).to_string());
                    req.respond(Response::from_string("ok")).expect("respond");
                }
            }
        });

        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_STDOUT", Some("cargo publish".to_string())),
            ("SHIPPER_CARGO_STDERR", Some(String::new())),
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
        ]);
        temp_env::with_vars(env_vars, || {
            let registry_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/beta/2.0.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let mut ws = planned_workspace(td.path(), registry_server.base_url.clone());
            ws.plan.packages = vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("alpha/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("beta/Cargo.toml"),
                    regime: None,
                },
            ];
            ws.plan.dependencies =
                BTreeMap::from([("beta".to_string(), vec!["alpha".to_string()])]);

            let state_dir = td.path().join(".shipper");
            state::save_state(
                &state_dir,
                &ExecutionState {
                    state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    attempt_history: Vec::new(),
                    packages: BTreeMap::from([(
                        "alpha@1.0.0".to_string(),
                        PackageProgress {
                            name: "alpha".to_string(),
                            version: "1.0.0".to_string(),
                            attempts: 1,
                            state: PackageState::Published,
                            last_updated_at: Utc::now(),
                        },
                    )]),
                },
            )
            .expect("save resume state");

            let mut opts = default_opts(state_dir);
            opts.parallel.enabled = true;
            opts.resume_from = Some("beta".to_string());
            opts.webhook = crate::webhook::WebhookConfig {
                url: webhook_url.clone(),
                ..Default::default()
            };

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert_eq!(receipt.packages.len(), 1);
            registry_server.join();
        });

        thread::sleep(Duration::from_secs(2));
        webhook_done.store(true, std::sync::atomic::Ordering::Release);
        webhook_handle.join().expect("webhook thread");

        let received = webhook_received.lock().expect("webhook lock");
        let completed = received
            .iter()
            .map(|payload| serde_json::from_str::<serde_json::Value>(payload).expect("JSON"))
            .find_map(|payload| {
                let legacy = payload
                    .get("extra")
                    .and_then(|extra| extra.get("legacy"))
                    .or_else(|| payload.get("legacy"))?;
                ((legacy.get("event")).and_then(serde_json::Value::as_str)
                    == Some("publish_completed"))
                .then(|| legacy.clone())
            })
            .unwrap_or_else(|| panic!("completion webhook; payloads: {received:?}"));

        assert_eq!(completed["total_packages"], 2);
        assert_eq!(completed["success_count"], 2);
        assert_eq!(completed["skipped_count"], 0);
        assert_eq!(completed["failure_count"], 0);
        assert_eq!(completed["result"], "success");
    }

    #[test]
    #[serial]
    fn run_publish_resume_scope_webhook_reports_whole_plan_partial_result() {
        for parallel in [false, true] {
            let td = tempdir().expect("tempdir");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = td.path().join("cargo.log");
            let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
            let webhook_url = format!("http://{}", webhook_server.server_addr());
            let (completed_tx, completed_rx) = std::sync::mpsc::sync_channel(1);
            let webhook_handle = thread::spawn(move || {
                while let Ok(Some(mut request)) =
                    webhook_server.recv_timeout(Duration::from_secs(3))
                {
                    let mut body = String::new();
                    std::io::Read::read_to_string(request.as_reader(), &mut body)
                        .expect("webhook body");
                    let payload: serde_json::Value =
                        serde_json::from_str(&body).expect("webhook JSON");
                    request
                        .respond(Response::from_string("ok"))
                        .expect("respond");
                    let legacy = payload
                        .get("extra")
                        .and_then(|extra| extra.get("legacy"))
                        .or_else(|| payload.get("legacy"));
                    if let Some(completed) =
                        legacy.filter(|value| value["event"] == "publish_completed")
                    {
                        completed_tx
                            .send(completed.clone())
                            .expect("completion receiver");
                        break;
                    }
                }
            });
            let mut env_vars = fake_program_env_vars(&bin);
            env_vars.extend([
                ("SHIPPER_CARGO_EXIT", Some("71".into())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.display().to_string()),
                ),
            ]);
            temp_env::with_vars(env_vars, || {
                let registry = spawn_registry_server(
                    BTreeMap::from([(
                        "/api/v1/crates/beta/0.1.0".into(),
                        vec![(200, "{}".into())],
                    )]),
                    1,
                );
                let mut ws = planned_workspace(td.path(), registry.base_url.clone());
                ws.plan.packages = ["alpha", "beta"]
                    .into_iter()
                    .map(|name| PlannedPackage {
                        name: name.into(),
                        version: "0.1.0".into(),
                        manifest_path: td.path().join(name).join("Cargo.toml"),
                        regime: None,
                    })
                    .collect();
                ws.plan.dependencies = BTreeMap::from([("beta".into(), vec!["alpha".into()])]);
                let mut opts = default_opts(td.path().join(".shipper"));
                opts.parallel.enabled = parallel;
                opts.resume_from = Some("beta".into());
                opts.webhook = crate::webhook::WebhookConfig {
                    url: webhook_url,
                    ..Default::default()
                };
                let mut reporter = CollectingReporter::default();
                let outcome =
                    run_publish_with_state(&ws, &opts, &mut reporter).expect("scoped publish");
                assert_eq!(
                    outcome.receipt.execution_result,
                    ExecutionResult::PartialFailure
                );
                assert_eq!(outcome.receipt.packages.len(), 1);
                assert_eq!(
                    outcome.state.packages["alpha@0.1.0"].state,
                    PackageState::Pending
                );
                assert_eq!(outcome.state.packages["alpha@0.1.0"].attempts, 0);
                assert!(
                    fs::read_to_string(&cargo_log)
                        .unwrap_or_default()
                        .is_empty()
                );
                registry.join();
            });
            let completed = completed_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("completion webhook");
            webhook_handle.join().expect("webhook worker");
            assert_eq!(completed["result"], "partial_failure");
            assert_eq!(completed["total_packages"], 2);
            assert_eq!(completed["success_count"], 0);
            assert_eq!(completed["skipped_count"], 1);
            assert_eq!(completed["failure_count"], 1);
        }
    }

    #[test]
    #[serial]
    fn run_publish_adds_missing_package_entries_to_existing_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");
            let existing = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: ws.plan.plan_id.clone(),
                registry: ws.plan.registry.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attempt_history: Vec::new(),
                packages: BTreeMap::new(),
            };
            state::save_state(&state_dir, &existing).expect("save");

            let opts = default_opts(PathBuf::from(".shipper"));
            let mut reporter = CollectingReporter::default();
            let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

            let st = state::load_state(&state_dir)
                .expect("load")
                .expect("exists");
            assert!(st.packages.contains_key("demo@0.1.0"));
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_marks_published_after_successful_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.verify_timeout = Duration::from_millis(200);
            opts.verify_poll_interval = Duration::from_millis(1);

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
            assert!(matches!(receipt.packages[0].state, PackageState::Published));
            let persisted =
                events::EventLog::read_from_file(&events::events_path(&td.path().join(".shipper")))
                    .expect("read events");
            assert!(
                persisted
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PackageUploaded))
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn readiness_success_reports_actual_polls_after_errors_in_publish_and_uploaded_resume() {
        for (parallel, resume_uploaded) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let td = tempdir().expect("tempdir");
            let bin = td.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = td.path().join("cargo.log");
            let mut env_vars = fake_program_env_vars(&bin);
            env_vars.extend([
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().to_string()),
                ),
            ]);
            temp_env::with_vars(env_vars, || {
                let mut statuses = vec![404];
                if resume_uploaded {
                    statuses.push(500);
                }
                statuses.extend([500, 500, 200]);
                let expected_requests = statuses.len();
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        statuses
                            .into_iter()
                            .map(|status| (status, "{}".to_string()))
                            .collect(),
                    )]),
                    expected_requests,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let state_dir = td.path().join(".shipper");
                let mut opts = default_opts(state_dir.clone());
                opts.parallel.enabled = parallel;
                opts.max_attempts = 1;
                if resume_uploaded {
                    opts.readiness.max_total_wait = Duration::ZERO;
                    run_publish(&ws, &opts, &mut CollectingReporter::default()).expect_err(
                        "successful upload with unknown readiness must retain Uploaded",
                    );
                    let retained = state::load_state(&state_dir).expect("load").expect("state");
                    assert!(matches!(
                        retained.packages["demo@0.1.0"].state,
                        PackageState::Uploaded
                    ));
                    assert!(!state::receipt_path(&state_dir).exists());
                }
                opts.readiness.max_total_wait = Duration::from_secs(2);
                let receipt = if resume_uploaded {
                    run_resume(&ws, &opts, &mut CollectingReporter::default())
                } else {
                    run_publish(&ws, &opts, &mut CollectingReporter::default())
                }
                .expect("transient errors recover to visible");
                assert!(matches!(receipt.execution_result, ExecutionResult::Success));
                let evidence = &receipt.packages[0].evidence.readiness_checks;
                assert_eq!(
                    evidence.len(),
                    1,
                    "only the successful third poll is conclusive"
                );
                assert_eq!(evidence[0].attempt, 3);
                assert!(evidence[0].visible);
                let log = events::EventLog::read_from_file(&events::events_path(&state_dir))
                    .expect("events");
                let completions = log
                    .all_events()
                    .iter()
                    .filter_map(|event| match event.event_type {
                        EventType::ReadinessComplete { attempts, .. } => Some(attempts),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    completions,
                    vec![3],
                    "terminal event must report actual polls, not evidence length"
                );
                assert!(
                    !log.all_events().iter().any(|event| matches!(
                        event.event_type,
                        EventType::ReadinessPoll { visible: false, .. }
                    )),
                    "failed probes must not fabricate negative observations"
                );
                assert_eq!(
                    std::fs::read_to_string(&cargo_log)
                        .expect("cargo log")
                        .lines()
                        .count(),
                    1,
                    "Uploaded resume verifies without invoking Cargo again"
                );
                assert_eq!(
                    server.seen.lock().expect("requests").len(),
                    expected_requests
                );
                server.join();
            });
        }
    }

    #[test]
    #[serial]
    fn run_publish_treats_failed_cargo_as_published_if_registry_shows_version() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout while uploading".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert_eq!(receipt.packages.len(), 1);
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                assert_eq!(receipt.packages[0].attempts, 1);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_retries_on_retryable_failures() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout talking to server".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (200, "{}".to_string()),
                        ],
                    )]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 2;
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                assert_eq!(receipt.packages[0].attempts, 2);
                assert!(reporter.warns.iter().any(|w| w.contains("next attempt in")));
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_errors_when_cargo_command_cannot_start() {
        let td = tempdir().expect("tempdir");
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(404, "{}".to_string())],
            )]),
            1,
        );

        let ws = planned_workspace(td.path(), server.base_url.clone());
        let missing = td.path().join("no-cargo-here");
        temp_env::with_vars(
            vec![(
                "SHIPPER_CARGO_BIN",
                Some(missing.to_str().expect("utf8").to_string()),
            )],
            || {
                let opts = default_opts(PathBuf::from(".shipper"));
                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_returns_error_on_permanent_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (404, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("permanent failure"));

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert!(matches!(
                    pkg.state,
                    PackageState::Failed {
                        class: ErrorClass::Permanent,
                        ..
                    }
                ));
                server.join();
            },
        );
    }

    fn assert_unverified_accepted_upload(state_dir: &Path, key: &str, cargo_log: &Path) {
        let state = state::load_state(state_dir).expect("load").expect("exists");
        let package = state.packages.get(key).expect("package");
        assert_eq!(package.state, PackageState::Uploaded);
        assert_eq!(package.attempts, 1);
        let history: Vec<_> = state
            .attempt_history
            .iter()
            .filter(|attempt| attempt.package == package.name && attempt.version == package.version)
            .collect();
        let [accepted] = history.as_slice() else {
            panic!("expected a single accepted attempt for {key}");
        };
        assert_eq!(accepted.attempt, 1);
        assert!(accepted.error_class.is_none());
        assert!(accepted.next_attempt_at.is_none());
        let calls = fs::read_to_string(cargo_log).expect("mock Cargo calls");
        assert_eq!(
            calls
                .lines()
                .filter(|call| {
                    call.split_whitespace().next() == Some("publish")
                        && call.split_whitespace().any(|arg| arg == package.name)
                })
                .count(),
            1
        );
        let path = crate::state::events::events_path(state_dir);
        let log = crate::state::events::EventLog::read_from_file(&path).expect("events");
        let events = log.all_events();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.package == key
                    && matches!(event.event_type, EventType::PackageUploaded))
                .count(),
            1
        );
        assert!(events.iter().any(|event| event.package == key
            && matches!(event.event_type, EventType::ReadinessTimeout { .. })));
        assert!(!events.iter().any(|event| event.package == key
            && matches!(event.event_type, EventType::PackagePublished { .. })));
        assert!(!events.iter().any(|event| matches!(
            event.event_type,
            EventType::ExecutionFinished {
                result: ExecutionResult::Success
            }
        )));
        let replay = crate::state::rebuild::rebuild_state_from_events(
            &path,
            crate::state::rebuild::StateRebuildOptions::new(state.registry.clone()),
        )
        .expect("replay events");
        assert_eq!(replay.packages[key].state, PackageState::Uploaded);
        assert_eq!(replay.attempt_history, state.attempt_history);
    }

    #[test]
    #[serial]
    fn run_publish_retains_accepted_upload_without_registry_visibility() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo.log");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || {
                // Initial visibility remains absent; the next invocation can verify it.
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (200, "{}".to_string()),
                        ],
                    )]),
                    4,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 1;
                opts.readiness.max_total_wait = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("version not observed on registry"));

                assert_unverified_accepted_upload(
                    &td.path().join(".shipper"),
                    "demo@0.1.0",
                    &cargo_log,
                );
                let original_calls = fs::read(&cargo_log).expect("initial mock Cargo calls");
                let receipt = run_resume(&ws, &opts, &mut CollectingReporter::default())
                    .expect("positive visibility can finish the accepted upload");
                assert_eq!(receipt.packages[0].state, PackageState::Published);
                assert_eq!(receipt.packages[0].attempts, 1);
                assert_eq!(
                    fs::read(&cargo_log).expect("resumed mock Cargo calls"),
                    original_calls
                );
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_recovers_on_final_registry_check_after_ambiguous_verify() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 1;
                opts.readiness.max_total_wait = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                server.join();
            },
        );
    }

    #[test]
    fn run_publish_errors_on_plan_mismatch_without_force_resume() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("does not match current plan_id"));
    }

    #[test]
    fn run_publish_allows_forced_resume_with_plan_mismatch() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.force_resume = true;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        // Shared terminal gate records already-complete packages in the receipt.
        assert_eq!(receipt.packages.len(), 1);
        assert_eq!(receipt.packages[0].name, "demo");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("forcing resume with mismatched plan_id"))
        );
    }

    #[test]
    fn run_resume_errors_when_state_is_missing() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let err = run_resume(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("no existing state found"));
    }

    #[test]
    fn resume_retry_ceiling_reports_first_blocker_in_plan_order() {
        let td = tempdir().expect("tempdir");
        let mut ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let mut first = ws.plan.packages[0].clone();
        first.name = "zeta".to_string();
        let mut second = first.clone();
        second.name = "alpha".to_string();
        ws.plan.packages = vec![first, second];

        let failed = |name: &str, attempts| PackageProgress {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            attempts,
            state: PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "registry confirmed NotPublished".to_string(),
            },
            last_updated_at: Utc::now(),
        };
        let mut state = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages: BTreeMap::from([
                ("alpha@0.1.0".to_string(), failed("alpha", 1)),
                ("zeta@0.1.0".to_string(), failed("zeta", 3)),
            ]),
        };
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;

        let error = reject_exhausted_resume_retry_ceiling(&ws, &opts, &state)
            .expect_err("both packages exhaust the requested ceiling");
        assert_eq!(
            classify_resume_retry_budget(&error),
            Some(ResumeRetryBudgetClassification {
                package: "zeta@0.1.0".to_string(),
                current_attempts: 3,
                requested_max_attempts: 1,
                minimum_max_attempts: Some(4),
            }),
            "the plan order, not state-map order, owns deterministic reporting"
        );

        opts.resume_from = Some("alpha".to_string());
        let selected_error = reject_exhausted_resume_retry_ceiling(&ws, &opts, &state)
            .expect_err("the selected package also exhausts the ceiling");
        assert_eq!(
            classify_resume_retry_budget(&selected_error),
            Some(ResumeRetryBudgetClassification {
                package: "alpha@0.1.0".to_string(),
                current_attempts: 1,
                requested_max_attempts: 1,
                minimum_max_attempts: Some(2),
            }),
            "packages before --resume-from must not participate in admission"
        );

        let evidence_paths = [
            td.path().join("state.json"),
            td.path().join("events.jsonl"),
            td.path().join("reconciliation.json"),
            td.path().join("cargo-and-registry.log"),
        ];
        for (index, path) in evidence_paths.iter().enumerate() {
            std::fs::write(path, format!("parallel-admission-sentinel-{index}"))
                .expect("write admission sentinel");
        }
        let evidence_before = evidence_paths
            .iter()
            .map(|path| std::fs::read(path).expect("read admission sentinel"))
            .collect::<Vec<_>>();
        opts.parallel.enabled = true;
        let same_level_error = reject_exhausted_resume_retry_ceiling(&ws, &opts, &state)
            .expect_err("parallel resume selects the target's entire dependency level");
        assert_eq!(
            classify_resume_retry_budget(&same_level_error),
            Some(ResumeRetryBudgetClassification {
                package: "zeta@0.1.0".to_string(),
                current_attempts: 3,
                requested_max_attempts: 1,
                minimum_max_attempts: Some(4),
            }),
            "the earlier exhausted sibling in the selected level must block deterministically"
        );
        for (path, before) in evidence_paths.iter().zip(evidence_before) {
            assert_eq!(
                std::fs::read(path).expect("read sentinel after parallel rejection"),
                before,
                "parallel admission rejection must precede state, event, reconciliation, Cargo, and registry mutation"
            );
        }

        opts.resume_from = None;
        opts.parallel.enabled = false;
        opts.max_attempts = u32::MAX;
        let zeta = state.packages.get_mut("zeta@0.1.0").expect("zeta progress");
        zeta.attempts = u32::MAX;
        let overflow_error = reject_exhausted_resume_retry_ceiling(&ws, &opts, &state)
            .expect_err("the maximum representable ceiling cannot increase");
        assert_eq!(
            classify_resume_retry_budget(&overflow_error),
            Some(ResumeRetryBudgetClassification {
                package: "zeta@0.1.0".to_string(),
                current_attempts: u32::MAX,
                requested_max_attempts: u32::MAX,
                minimum_max_attempts: None,
            })
        );
    }

    #[test]
    fn run_resume_runs_publish_when_state_exists() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let receipt = run_resume(&ws, &opts, &mut reporter).expect("resume");
        // Already-terminal packages are included via the shared terminal gate.
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
    }

    // Preflight-specific tests

    fn preflight_pkg(name: &str, is_new_crate: bool) -> PreflightPackage {
        PreflightPackage {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            already_published: false,
            is_new_crate,
            auth_type: Some(AuthType::Token),
            ownership_verified: true,
            dry_run_passed: true,
            dry_run_output: None,
        }
    }

    #[test]
    fn estimate_preflight_duration_accounts_for_crates_io_first_publish_burst() {
        let packages: Vec<_> = (0..6)
            .map(|i| preflight_pkg(&format!("crate-{i}"), true))
            .collect();

        let estimate = preflight::duration::estimate_preflight_duration("crates-io", &packages)
            .expect("crates.io profile should estimate");

        assert_eq!(estimate.registry_profile, "crates-io");
        assert_eq!(estimate.first_publish_count, 6);
        assert_eq!(estimate.update_count, 0);
        assert_eq!(estimate.minimum_registry_pacing, Duration::from_mins(10));
        assert!(
            estimate
                .notes
                .iter()
                .any(|note| note.contains("documented registry pacing"))
        );
    }

    #[test]
    fn estimate_preflight_duration_is_none_for_unknown_registry() {
        let packages = vec![preflight_pkg("demo", true)];

        assert!(preflight::duration::estimate_preflight_duration("private", &packages).is_none());
    }

    #[test]
    fn preflight_report_serializes_correctly() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Proven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: true,
                dry_run_output: None,
            }],
            timestamp: Utc::now(),
            estimated_publish_duration: None,
            dry_run_output: None,
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let parsed: PreflightReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, report.plan_id);
        assert_eq!(parsed.token_detected, report.token_detected);
        assert_eq!(parsed.finishability, report.finishability);
        assert_eq!(parsed.packages.len(), 1);
    }

    #[test]
    fn finishability_proven_when_all_checks_pass() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Proven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: true,
                dry_run_output: None,
            }],
            timestamp: Utc::now(),
            estimated_publish_duration: None,
            dry_run_output: None,
        };

        assert_eq!(report.finishability, Finishability::Proven);
    }

    #[test]
    fn finishability_not_proven_when_ownership_unverified() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::NotProven,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: true,
                auth_type: Some(AuthType::Token),
                ownership_verified: false,
                dry_run_passed: true,
                dry_run_output: None,
            }],
            timestamp: Utc::now(),
            estimated_publish_duration: None,
            dry_run_output: None,
        };

        assert_eq!(report.finishability, Finishability::NotProven);
    }

    #[test]
    fn finishability_failed_when_dry_run_fails() {
        let report = PreflightReport {
            plan_id: "test-plan".to_string(),
            token_detected: true,
            finishability: Finishability::Failed,
            packages: vec![PreflightPackage {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                already_published: false,
                is_new_crate: false,
                auth_type: Some(AuthType::Token),
                ownership_verified: true,
                dry_run_passed: false,
                dry_run_output: None,
            }],
            timestamp: Utc::now(),
            estimated_publish_duration: None,
            dry_run_output: None,
        };

        assert_eq!(report.finishability, Finishability::Failed);
    }

    #[test]
    fn preflight_package_serializes_correctly() {
        let pkg = PreflightPackage {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            already_published: false,
            is_new_crate: true,
            auth_type: Some(AuthType::Token),
            ownership_verified: true,
            dry_run_passed: true,
            dry_run_output: None,
        };

        let json = serde_json::to_string(&pkg).expect("serialize");
        let parsed: PreflightPackage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, pkg.name);
        assert_eq!(parsed.version, pkg.version);
        assert_eq!(parsed.already_published, pkg.already_published);
        assert_eq!(parsed.is_new_crate, pkg.is_new_crate);
        assert_eq!(parsed.auth_type, pkg.auth_type);
        assert_eq!(parsed.ownership_verified, pkg.ownership_verified);
        assert_eq!(parsed.dry_run_passed, pkg.dry_run_passed);
    }

    #[test]
    fn auth_type_serializes_correctly() {
        let token_auth = AuthType::Token;
        let tp_auth = AuthType::TrustedPublishing;
        let unknown_auth = AuthType::Unknown;

        let json_token = serde_json::to_string(&token_auth).expect("serialize");
        let parsed_token: AuthType = serde_json::from_str(&json_token).expect("deserialize");
        assert_eq!(parsed_token, AuthType::Token);

        let json_tp = serde_json::to_string(&tp_auth).expect("serialize");
        let parsed_tp: AuthType = serde_json::from_str(&json_tp).expect("deserialize");
        assert_eq!(parsed_tp, AuthType::TrustedPublishing);

        let json_unknown = serde_json::to_string(&unknown_auth).expect("serialize");
        let parsed_unknown: AuthType = serde_json::from_str(&json_unknown).expect("deserialize");
        assert_eq!(parsed_unknown, AuthType::Unknown);
    }

    // Integration tests for preflight scenarios

    #[test]
    #[serial]
    fn preflight_with_all_packages_already_published() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                // Mock registry: version already exists (200)
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.skip_ownership_check = true;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(report.packages[0].already_published);
                assert!(!report.packages[0].is_new_crate);
                assert!(report.packages[0].dry_run_passed);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_new_crates() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                // Mock registry: crate doesn't exist (404 for both crate and version)
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.skip_ownership_check = true;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(!report.packages[0].already_published);
                assert!(report.packages[0].is_new_crate);
                assert!(report.packages[0].dry_run_passed);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_ownership_verification_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
                // Mock registry: version doesn't exist, crate exists, ownership check fails with 403
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/owners".to_string(),
                            vec![(403, "{}".to_string())],
                        ),
                    ]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.skip_ownership_check = false;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(!report.packages[0].ownership_verified);
                // Should be NotProven because ownership is unverified
                assert_eq!(report.finishability, Finishability::NotProven);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_with_dry_run_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                ("SHIPPER_CARGO_STDERR", Some("dry-run failed".to_string())),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.skip_ownership_check = true;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(!report.packages[0].dry_run_passed);
                // Should be Failed because dry-run failed
                assert_eq!(report.finishability, Finishability::Failed);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_strict_ownership_requires_token() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "CARGO_HOME",
                    Some(td.path().to_str().expect("utf8").to_string()),
                ),
                ("CARGO_REGISTRY_TOKEN", None),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None),
            ],
            || {
                let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.strict_ownership = true;
                // No token set

                let mut reporter = CollectingReporter::default();
                let err = run_preflight(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(
                    format!("{err:#}").contains("strict ownership requested but no token found")
                );
            },
        );
    }

    #[test]
    #[serial]
    fn preflight_finishability_proven_with_all_checks_pass() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
                // Mock registry: version doesn't exist, crate exists, ownership succeeds
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/owners".to_string(),
                            vec![(200, r#"{"users":[]}"#.to_string())],
                        ),
                    ]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.allow_dirty = true;
                opts.skip_ownership_check = false;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert_eq!(report.packages.len(), 1);
                assert!(report.packages[0].ownership_verified);
                assert!(report.packages[0].dry_run_passed);
                assert_eq!(report.finishability, Finishability::Proven);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn test_fast_policy_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        // Deliberately set cargo to fail â€” if dry-run runs, it would fail
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("1".to_string()))],
            || {
                // Only need version_exists + check_new_crate
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.policy = crate::types::PublishPolicy::Fast;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                // dry_run_passed should be true (skipped), not false (cargo would have failed)
                assert!(report.packages[0].dry_run_passed);
                // ownership_verified should be false (skipped by Fast policy)
                assert!(!report.packages[0].ownership_verified);
                // Finishability is NotProven because ownership unverified
                assert_eq!(report.finishability, Finishability::NotProven);
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("skipping dry-run"))
                );
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn test_balanced_policy_skips_ownership() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
                // Only need version_exists + check_new_crate (no ownership endpoint)
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.policy = crate::types::PublishPolicy::Balanced;
                opts.skip_ownership_check = false; // would check in Safe, but Balanced overrides

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                // ownership_verified false (Balanced skips ownership)
                assert!(!report.packages[0].ownership_verified);
                // dry_run_passed true (Balanced still runs dry-run)
                assert!(report.packages[0].dry_run_passed);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn test_safe_policy_runs_all_checks() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                ("CARGO_REGISTRY_TOKEN", Some("fake-token".to_string())),
            ],
            || {
                // Need version_exists + check_new_crate + ownership
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo/owners".to_string(),
                            vec![(200, r#"{"users":[]}"#.to_string())],
                        ),
                    ]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.policy = crate::types::PublishPolicy::Safe;
                opts.skip_ownership_check = false;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                // All checks ran
                assert!(report.packages[0].dry_run_passed);
                assert!(report.packages[0].ownership_verified);
                assert_eq!(report.finishability, Finishability::Proven);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn test_verify_mode_none_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        // Set cargo to fail â€” if dry-run ran, it would fail
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("1".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.verify_mode = crate::types::VerifyMode::None;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                // dry_run_passed is true because verify_mode=None skips it
                assert!(report.packages[0].dry_run_passed);
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("skipping dry-run"))
                );
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn test_verify_mode_package_runs_per_package() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(404, "{}".to_string())],
                        ),
                    ]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.verify_mode = crate::types::VerifyMode::Package;

                let mut reporter = CollectingReporter::default();
                let report = run_preflight(&ws, &opts, &mut reporter).expect("preflight");

                assert!(report.packages[0].dry_run_passed);
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("per-package dry-run"))
                );
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn resume_from_uploaded_skips_cargo_publish_and_reaches_published() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(args_log.to_str().expect("utf8").to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            // First request (early check) returns 404, second (readiness) returns 200
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );

            let ws = planned_workspace(td.path(), server.base_url.clone());
            let state_dir = td.path().join(".shipper");

            // Pre-create state with Uploaded + attempts=1
            let mut packages = std::collections::BTreeMap::new();
            packages.insert(
                "demo@0.1.0".to_string(),
                PackageProgress {
                    name: "demo".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Uploaded,
                    last_updated_at: Utc::now(),
                },
            );
            let st = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: ws.plan.plan_id.clone(),
                registry: ws.plan.registry.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attempt_history: Vec::new(),
                packages,
            };
            state::save_state(&state_dir, &st).expect("save");

            let opts = default_opts(PathBuf::from(".shipper"));
            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Package should reach Published via the readiness verification path
            assert_eq!(receipt.packages.len(), 1);
            assert!(
                matches!(receipt.packages[0].state, PackageState::Published),
                "expected Published, got {:?}",
                receipt.packages[0].state
            );

            // Cargo publish should NOT have been invoked
            // (args_log should not exist or be empty â€” no cargo publish calls)
            let cargo_invoked = args_log.exists()
                && fs::read_to_string(&args_log)
                    .unwrap_or_default()
                    .contains("publish");
            assert!(
                !cargo_invoked,
                "cargo publish should not have been invoked on resume from Uploaded"
            );

            // Verify reporter got the resume message
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|i| i.contains("resuming from uploaded")
                        || i.contains("already published")
                        || i.contains("already complete"))
            );

            // Verify the readiness path was exercised
            assert!(
                reporter.infos.iter().any(|i| i.contains("verifying")
                    || i.contains("visible")
                    || i.contains("readiness")),
                "expected readiness verification to be exercised, reporter infos: {:?}",
                reporter.infos
            );

            server.join();
        });
    }

    #[test]
    #[serial]
    fn test_resume_from_skips_initial_packages() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "SHIPPER_CARGO_ARGS_LOG",
                Some(args_log.to_str().expect("utf8").to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            // Mock registry for two packages
            // We expect 2 requests total (pkg2 exists? then pkg2 readiness)
            // pkg1 is skipped because of resume_from
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/pkg1/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/pkg2/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            // Update plan to have two packages
            ws.plan.packages = vec![
                PlannedPackage {
                    name: "pkg1".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: td.path().join("pkg1/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "pkg2".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: td.path().join("pkg2/Cargo.toml"),
                    regime: None,
                },
            ];

            let mut opts = default_opts(PathBuf::from(".shipper"));
            // Resume from second package
            opts.resume_from = Some("pkg2".to_string());

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Should only have 1 package in receipt (pkg2)
            assert_eq!(receipt.packages.len(), 1);
            assert_eq!(receipt.packages[0].name, "pkg2");

            // Cargo log should only contain publish for pkg2
            let log = std::fs::read_to_string(&args_log).expect("read log");
            assert!(!log.contains("pkg1"));
            assert!(log.contains("pkg2"));

            server.join();
        });
    }

    // â”€â”€ Additional coverage tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    fn init_state_creates_pending_entries_for_all_packages() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let st = init_state(&ws, &state_dir).expect("init");
        assert_eq!(st.plan_id, "plan-demo");
        assert_eq!(st.packages.len(), 1);
        let progress = st.packages.get("demo@0.1.0").expect("pkg");
        assert_eq!(progress.name, "demo");
        assert_eq!(progress.version, "0.1.0");
        assert_eq!(progress.attempts, 0);
        assert!(matches!(progress.state, PackageState::Pending));
    }

    #[test]
    fn init_state_persists_state_to_disk() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let _ = init_state(&ws, &state_dir).expect("init");
        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert_eq!(loaded.plan_id, "plan-demo");
        assert!(loaded.packages.contains_key("demo@0.1.0"));
    }

    #[test]
    fn init_state_with_multi_package_plan() {
        let td = tempdir().expect("tempdir");
        let mut ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        ws.plan.packages = vec![
            PlannedPackage {
                name: "alpha".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: td.path().join("alpha/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "beta".to_string(),
                version: "2.0.0".to_string(),
                manifest_path: td.path().join("beta/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "gamma".to_string(),
                version: "0.3.0".to_string(),
                manifest_path: td.path().join("gamma/Cargo.toml"),
                regime: None,
            },
        ];
        let state_dir = td.path().join(".shipper");

        let st = init_state(&ws, &state_dir).expect("init");
        assert_eq!(st.packages.len(), 3);
        assert!(st.packages.contains_key("alpha@1.0.0"));
        assert!(st.packages.contains_key("beta@2.0.0"));
        assert!(st.packages.contains_key("gamma@0.3.0"));
        for progress in st.packages.values() {
            assert_eq!(progress.attempts, 0);
            assert!(matches!(progress.state, PackageState::Pending));
        }
    }

    #[test]
    fn run_publish_errors_on_invalid_resume_from_target() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let webhook_server = Server::http("127.0.0.1:0").expect("webhook server");
        let webhook_url = format!("http://{}", webhook_server.server_addr());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.resume_from = Some("nonexistent-package".to_string());
        opts.webhook = crate::webhook::WebhookConfig {
            url: webhook_url,
            ..Default::default()
        };

        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        assert!(format!("{err:#}").contains("resume-from package"));
        assert!(format!("{err:#}").contains("not found in publish plan"));
        assert!(
            webhook_server
                .recv_timeout(Duration::from_millis(200))
                .expect("check webhook server")
                .is_none(),
            "preparation failure must not emit PublishStarted"
        );
    }

    #[test]
    #[serial]
    fn run_publish_writes_execution_events() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

            let events_path = td.path().join(".shipper").join("events.jsonl");
            let log =
                crate::state::events::EventLog::read_from_file(&events_path).expect("read events");
            let events = log.all_events();

            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::ExecutionStarted))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PlanCreated { .. }))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::PackageSkipped { .. }))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.event_type, EventType::ExecutionFinished { .. }))
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_receipt_contains_evidence_after_success() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([
            ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
            (
                "CARGO_REGISTRY_TOKEN",
                Some("release-secret-token".to_string()),
            ),
            (
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                Some("https://example.invalid/oidc".to_string()),
            ),
            (
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                Some("oidc-request-token".to_string()),
            ),
        ]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            assert_eq!(receipt.receipt_version, "shipper.receipt.v2");
            assert_eq!(receipt.plan_id, "plan-demo");
            assert_eq!(receipt.registry.name, "crates-io");
            assert_eq!(receipt.packages.len(), 1);
            assert!(matches!(receipt.packages[0].state, PackageState::Published));
            assert_eq!(receipt.packages[0].attempts, 1);
            assert!(!receipt.packages[0].evidence.attempts.is_empty());
            assert_eq!(receipt.packages[0].evidence.attempts[0].attempt_number, 1);
            assert_eq!(receipt.packages[0].evidence.attempts[0].exit_code, 0);
            let auth_evidence = receipt
                .auth_evidence
                .as_ref()
                .expect("receipt records auth evidence");
            assert_eq!(auth_evidence.schema_version, "shipper.auth_evidence.v1");
            assert_eq!(auth_evidence.registry, "crates-io");
            assert_eq!(
                auth_evidence.auth_mode,
                AuthEvidenceMode::CargoTokenWithOidcContext
            );
            assert!(auth_evidence.token_detected);
            assert!(auth_evidence.oidc_request_url_present);
            assert!(auth_evidence.oidc_request_token_present);
            let receipt_json = serde_json::to_string(&receipt).expect("serialize receipt");
            assert!(!receipt_json.contains("release-secret-token"));
            assert!(!receipt_json.contains("oidc-request-token"));
            let state = state::load_state(&td.path().join(".shipper"))
                .expect("load state")
                .expect("state exists");
            assert_eq!(state.attempt_history.len(), 1);
            assert_eq!(state.attempt_history[0].package, "demo");
            assert_eq!(state.attempt_history[0].version, "0.1.0");
            assert_eq!(state.attempt_history[0].attempt, 1);
            assert_eq!(state.attempt_history[0].max_attempts, opts.max_attempts);
            assert!(state.attempt_history[0].error_class.is_none());
            assert!(state.attempt_history[0].next_attempt_at.is_none());
            let events = events::EventLog::read_from_file(&td.path().join(".shipper/events.jsonl"))
                .expect("read events");
            let event_auth_evidence = events
                .all_events()
                .iter()
                .find_map(|event| match &event.event_type {
                    EventType::AuthEvidenceRecorded { evidence } => Some(evidence),
                    _ => None,
                })
                .expect("auth evidence event");
            assert_eq!(event_auth_evidence, auth_evidence);
            assert!(
                events
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::PackageUploaded))
            );
            assert!(
                events
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::ReadinessPoll { .. }))
            );
            let readiness_started = events
                .all_events()
                .iter()
                .position(|event| matches!(event.event_type, EventType::ReadinessStarted { .. }))
                .expect("production readiness envelope starts before polling");
            let readiness_complete = events
                .all_events()
                .iter()
                .position(|event| matches!(event.event_type, EventType::ReadinessComplete { .. }))
                .expect("production readiness envelope completes after polling");
            assert!(
                readiness_started < readiness_complete,
                "production readiness envelope must open before it closes"
            );
            assert!(
                events
                    .all_events()
                    .iter()
                    .any(|event| matches!(event.event_type, EventType::ReadinessComplete { .. }))
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_receipt_persisted_to_disk() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

            let state_dir = td.path().join(".shipper");
            let receipt_path = state::receipt_path(&state_dir);
            assert!(receipt_path.exists());

            let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
            let parsed: Receipt = serde_json::from_str(&receipt_json).expect("parse receipt");
            assert_eq!(parsed.plan_id, "plan-demo");
            assert_eq!(parsed.receipt_version, "shipper.receipt.v2");
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_dirty_git_fails_when_not_allowed() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_GIT_CLEAN", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.allow_dirty = false;

            let mut reporter = CollectingReporter::default();
            let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("dirty") || msg.contains("uncommitted") || msg.contains("git"),
                "unexpected error: {msg}"
            );
        });
    }

    #[test]
    #[serial]
    fn run_publish_state_attempts_counter_increments_on_retry() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("rate limit exceeded".to_string()),
                ),
            ],
            || {
                // Three negative version probes (precheck + two reconciliations)
                // and one crate-exists probe keep retry evidence conclusive.
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/demo/0.1.0".to_string(),
                            vec![
                                (404, "{}".to_string()),
                                (404, "{}".to_string()),
                                (404, "{}".to_string()),
                            ],
                        ),
                        (
                            "/api/v1/crates/demo".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                    ]),
                    4,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 2;
                opts.readiness.max_total_wait = Duration::ZERO;
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let _ = run_publish(&ws, &opts, &mut reporter);

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert_eq!(pkg.attempts, 2, "expected 2 attempts");
                assert_eq!(st.attempt_history.len(), 2);
                assert_eq!(st.attempt_history[0].attempt, 1);
                assert_eq!(
                    st.attempt_history[0].error_class,
                    Some(ErrorClass::Ambiguous)
                );
                assert!(st.attempt_history[0].next_attempt_at.is_some());
                assert_eq!(st.attempt_history[1].attempt, 2);
                assert_eq!(
                    st.attempt_history[1].error_class,
                    Some(ErrorClass::Ambiguous)
                );
                assert!(st.attempt_history[1].next_attempt_at.is_none());
                let events =
                    events::EventLog::read_from_file(&td.path().join(".shipper/events.jsonl"))
                        .expect("read events");
                assert!(
                    events.all_events().iter().any(|event| matches!(
                        event.event_type,
                        EventType::RateLimitObserved { .. }
                    ))
                );
                assert!(
                    events
                        .all_events()
                        .iter()
                        .any(|event| matches!(event.event_type, EventType::RetryScheduled { .. }))
                );
                assert!(
                    events
                        .all_events()
                        .iter()
                        .any(|event| matches!(event.event_type, EventType::PublishWaiting { .. }))
                );
                assert_eq!(server.seen.lock().expect("requests").len(), 4);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_permanent_failure_emits_failed_event() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (404, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let _ = run_publish(&ws, &opts, &mut reporter);

                let events_path = td.path().join(".shipper").join("events.jsonl");
                let log = crate::state::events::EventLog::read_from_file(&events_path)
                    .expect("read events");
                let events = log.all_events();

                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PackageFailed { .. }))
                );
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PackageAttempted { .. }))
                );
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_multi_package_first_published_second_skipped() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/alpha/1.0.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/beta/2.0.0".to_string(),
                        vec![(200, "{}".to_string())],
                    ),
                ]),
                3,
            );
            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            ws.plan.packages = vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("alpha/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("beta/Cargo.toml"),
                    regime: None,
                },
            ];
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            assert_eq!(receipt.packages.len(), 2);
            assert!(matches!(receipt.packages[0].state, PackageState::Published));
            assert!(matches!(
                receipt.packages[1].state,
                PackageState::Skipped { .. }
            ));
            assert_eq!(receipt.packages[0].name, "alpha");
            assert_eq!(receipt.packages[1].name, "beta");
            server.join();
        });
    }

    #[test]
    fn backoff_delay_linear_strategy() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);
        let d1 = backoff_delay(base, max, 1, crate::retry::RetryStrategyType::Linear, 0.0);
        let d3 = backoff_delay(base, max, 3, crate::retry::RetryStrategyType::Linear, 0.0);
        let d20 = backoff_delay(base, max, 20, crate::retry::RetryStrategyType::Linear, 0.0);

        assert_eq!(d1, Duration::from_millis(100));
        assert!(d3 > d1);
        assert!(d20 <= max, "linear delay should be capped at max");
    }

    #[test]
    fn backoff_delay_constant_strategy() {
        let base = Duration::from_millis(200);
        let max = Duration::from_secs(1);
        let d1 = backoff_delay(base, max, 1, crate::retry::RetryStrategyType::Constant, 0.0);
        let d5 = backoff_delay(base, max, 5, crate::retry::RetryStrategyType::Constant, 0.0);
        let d10 = backoff_delay(
            base,
            max,
            10,
            crate::retry::RetryStrategyType::Constant,
            0.0,
        );

        assert_eq!(d1, base);
        assert_eq!(d5, base);
        assert_eq!(d10, base);
    }

    #[test]
    fn backoff_delay_immediate_strategy() {
        let base = Duration::from_millis(200);
        let max = Duration::from_secs(1);
        let d1 = backoff_delay(
            base,
            max,
            1,
            crate::retry::RetryStrategyType::Immediate,
            0.0,
        );
        let d5 = backoff_delay(
            base,
            max,
            5,
            crate::retry::RetryStrategyType::Immediate,
            0.0,
        );

        assert_eq!(d1, Duration::ZERO);
        assert_eq!(d5, Duration::ZERO);
    }

    #[test]
    fn backoff_delay_exponential_zero_jitter_is_deterministic() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(10);
        let d1a = backoff_delay(
            base,
            max,
            1,
            crate::retry::RetryStrategyType::Exponential,
            0.0,
        );
        let d1b = backoff_delay(
            base,
            max,
            1,
            crate::retry::RetryStrategyType::Exponential,
            0.0,
        );
        assert_eq!(d1a, d1b);
        assert_eq!(d1a, base);
    }

    #[test]
    fn classify_cargo_failure_rate_limit() {
        let (class, _msg) = classify_cargo_failure("HTTP 429 too many requests", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_cargo_failure_timeout() {
        let (class, _msg) = classify_cargo_failure("timeout talking to server", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_cargo_failure_service_unavailable() {
        let (class, _msg) = classify_cargo_failure("HTTP 503 service unavailable", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_cargo_failure_auth_failure() {
        let (class, _msg) = classify_cargo_failure("permission denied", "");
        assert_eq!(class, ErrorClass::Permanent);
    }

    #[test]
    fn classify_cargo_failure_unknown_error_is_ambiguous() {
        let (class, _msg) = classify_cargo_failure("something totally unexpected", "");
        assert_eq!(class, ErrorClass::Ambiguous);
    }

    #[test]
    fn short_state_covers_all_variants() {
        assert_eq!(short_state(&PackageState::Pending), "pending");
        assert_eq!(short_state(&PackageState::Uploaded), "uploaded");
        assert_eq!(short_state(&PackageState::Published), "published");
        assert_eq!(
            short_state(&PackageState::Skipped {
                reason: "already published".to_string()
            }),
            "skipped"
        );
        assert_eq!(
            short_state(&PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "timeout".to_string()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "unknown".to_string()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&PackageState::Ambiguous {
                message: "not sure".to_string()
            }),
            "ambiguous"
        );
    }

    #[test]
    fn pkg_key_formats_correctly() {
        assert_eq!(pkg_key("my-crate", "1.2.3"), "my-crate@1.2.3");
        assert_eq!(pkg_key("a", "0.0.1"), "a@0.0.1");
        assert_eq!(pkg_key("foo_bar-baz", "10.20.30"), "foo_bar-baz@10.20.30");
    }

    #[test]
    #[serial]
    fn run_publish_skipped_package_receipt_has_empty_evidence() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            assert_eq!(receipt.packages.len(), 1);
            assert!(matches!(
                receipt.packages[0].state,
                PackageState::Skipped { .. }
            ));
            assert!(
                receipt.packages[0].evidence.attempts.is_empty(),
                "skipped packages should have no attempt evidence"
            );
            assert!(
                receipt.packages[0].evidence.readiness_checks.is_empty(),
                "skipped packages should have no readiness evidence"
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_execution_result_is_success_when_all_published() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Check that the receipt event log was written
            assert!(receipt.event_log_path.exists());
            let log = crate::state::events::EventLog::read_from_file(&receipt.event_log_path)
                .expect("events");
            let finish_events: Vec<_> = log
                .all_events()
                .iter()
                .filter(|e| matches!(e.event_type, EventType::ExecutionFinished { .. }))
                .collect();
            assert_eq!(finish_events.len(), 1);
            if let EventType::ExecutionFinished { result } = &finish_events[0].event_type {
                assert!(
                    matches!(result, ExecutionResult::Success),
                    "expected Success, got {result:?}"
                );
            }
            server.join();
        });
    }

    #[test]
    fn run_publish_force_without_predecessor_preserves_completed_state() {
        // Explicit force is accepted when no other invocation owns the guard.
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        // Pre-create state with all packages published
        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: ws.plan.plan_id.clone(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.force = true;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert_eq!(receipt.packages.len(), 1);
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
    }

    #[test]
    #[serial]
    fn run_publish_resume_from_skips_before_and_warns() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([
                    (
                        "/api/v1/crates/alpha/1.0.0".to_string(),
                        vec![(404, "{}".to_string())],
                    ),
                    (
                        "/api/v1/crates/beta/2.0.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    ),
                ]),
                2,
            );

            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            ws.plan.packages = vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("alpha/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("beta/Cargo.toml"),
                    regime: None,
                },
            ];
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.resume_from = Some("beta".to_string());

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Only beta should be in receipt
            assert_eq!(receipt.packages.len(), 1);
            assert_eq!(receipt.packages[0].name, "beta");

            // Alpha was pending, so it should produce a warning about skipping
            assert!(
                reporter
                    .warns
                    .iter()
                    .any(|w| w.contains("skipping") && w.contains("before resume point")),
                "expected warning about skipping alpha, got: {:?}",
                reporter.warns
            );
            server.join();
        });
    }

    #[test]
    #[serial]
    fn run_publish_resume_from_already_done_skipped_silently() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("0".to_string()))]);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/beta/2.0.0".to_string(),
                    vec![(404, "{}".to_string()), (200, "{}".to_string())],
                )]),
                2,
            );

            let mut ws = planned_workspace(td.path(), server.base_url.clone());
            ws.plan.packages = vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: td.path().join("alpha/Cargo.toml"),
                    regime: None,
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: td.path().join("beta/Cargo.toml"),
                    regime: None,
                },
            ];

            // Pre-create state with alpha already published
            let state_dir = td.path().join(".shipper");
            let mut packages = std::collections::BTreeMap::new();
            packages.insert(
                "alpha@1.0.0".to_string(),
                PackageProgress {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    last_updated_at: Utc::now(),
                },
            );
            let st = ExecutionState {
                state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: ws.plan.plan_id.clone(),
                registry: ws.plan.registry.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attempt_history: Vec::new(),
                packages,
            };
            state::save_state(&state_dir, &st).expect("save");

            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.resume_from = Some("beta".to_string());

            let mut reporter = CollectingReporter::default();
            let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Alpha should be silently skipped (already complete), beta published
            assert_eq!(receipt.packages.len(), 1);
            assert_eq!(receipt.packages[0].name, "beta");

            // Alpha should produce an info about "already complete" not a warning
            assert!(
                reporter
                    .infos
                    .iter()
                    .any(|i| i.contains("already complete") && i.contains("alpha")),
                "expected info about alpha being already complete, got: {:?}",
                reporter.infos
            );
            server.join();
        });
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn update_state_transitions_correctly() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        // Pending -> Uploaded
        update_state(&mut st, &state_dir, key, PackageState::Uploaded).expect("update");
        assert!(matches!(
            st.packages.get(key).unwrap().state,
            PackageState::Uploaded
        ));

        // Uploaded -> Published
        update_state(&mut st, &state_dir, key, PackageState::Published).expect("update");
        assert!(matches!(
            st.packages.get(key).unwrap().state,
            PackageState::Published
        ));

        // Verify persisted to disk
        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert!(matches!(
            loaded.packages.get(key).unwrap().state,
            PackageState::Published
        ));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn update_state_to_failed() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        let failed = PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "auth failure".to_string(),
        };
        update_state(&mut st, &state_dir, key, failed).expect("update");

        let pkg = st.packages.get(key).unwrap();
        match &pkg.state {
            PackageState::Failed { class, message } => {
                assert_eq!(*class, ErrorClass::Permanent);
                assert_eq!(message, "auth failure");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn update_state_to_skipped() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        let skipped = PackageState::Skipped {
            reason: "already published".to_string(),
        };
        update_state(&mut st, &state_dir, key, skipped).expect("update");

        match &st.packages.get(key).unwrap().state {
            PackageState::Skipped { reason } => {
                assert_eq!(reason, "already published");
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[test]
    fn receipt_serialization_roundtrip() {
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-test-123".to_string(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages: vec![
                PackageReceipt {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    duration_ms: 1234,
                    evidence: crate::types::PackageEvidence {
                        attempts: vec![AttemptEvidence {
                            attempt_number: 1,
                            command: "cargo publish -p alpha".to_string(),
                            exit_code: 0,
                            stdout_tail: "Uploading alpha".to_string(),
                            stderr_tail: String::new(),
                            timestamp: Utc::now(),
                            duration: Duration::from_millis(500),
                        }],
                        readiness_checks: vec![ReadinessEvidence {
                            attempt: 1,
                            visible: true,
                            timestamp: Utc::now(),
                            delay_before: Duration::from_millis(100),
                        }],
                    },
                    compromised_at: None,
                    compromised_by: None,
                    superseded_by: None,
                },
                PackageReceipt {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    attempts: 0,
                    state: PackageState::Skipped {
                        reason: "already published".to_string(),
                    },
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    duration_ms: 10,
                    evidence: crate::types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                    compromised_at: None,
                    compromised_by: None,
                    superseded_by: None,
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: environment::collect_environment_fingerprint(),
            auth_evidence: None,
            execution_result: crate::types::ExecutionResult::Success,
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        let parsed: Receipt = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, receipt.plan_id);
        assert_eq!(parsed.receipt_version, receipt.receipt_version);
        assert_eq!(parsed.packages.len(), 2);
        assert!(matches!(parsed.packages[0].state, PackageState::Published));
        assert!(matches!(
            parsed.packages[1].state,
            PackageState::Skipped { .. }
        ));
        assert_eq!(parsed.packages[0].evidence.attempts.len(), 1);
        assert_eq!(parsed.packages[0].evidence.readiness_checks.len(), 1);
    }

    #[test]
    fn execution_state_serialization_roundtrip() {
        let mut packages = BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "timeout".to_string(),
                },
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-serde-test".to_string(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };

        let json = serde_json::to_string(&st).expect("serialize");
        let parsed: ExecutionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, st.plan_id);
        assert_eq!(parsed.packages.len(), 1);
        let pkg = parsed.packages.get("demo@0.1.0").expect("pkg");
        assert_eq!(pkg.attempts, 3);
        match &pkg.state {
            PackageState::Failed { class, message } => {
                assert_eq!(*class, ErrorClass::Retryable);
                assert_eq!(message, "timeout");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn collecting_reporter_tracks_all_message_types() {
        let mut reporter = CollectingReporter::default();
        reporter.info("info-1");
        reporter.info("info-2");
        reporter.warn("warn-1");
        reporter.error("error-1");
        reporter.error("error-2");
        reporter.error("error-3");

        assert_eq!(reporter.infos.len(), 2);
        assert_eq!(reporter.warns.len(), 1);
        assert_eq!(reporter.errors.len(), 3);
        assert_eq!(reporter.infos[0], "info-1");
        assert_eq!(reporter.warns[0], "warn-1");
        assert_eq!(reporter.errors[2], "error-3");
    }

    #[test]
    #[serial]
    fn verify_published_disabled_does_single_check() {
        // When readiness is disabled, it still does one version_exists check
        let server = spawn_registry_server(
            std::collections::BTreeMap::from([(
                "/api/v1/crates/demo/0.1.0".to_string(),
                vec![(200, "{}".to_string())],
            )]),
            1,
        );
        let reg = test_registry_client(Registry {
            name: "crates-io".to_string(),
            api_base: server.base_url.clone(),
            index_base: None,
        })
        .expect("client");

        let config = crate::types::ReadinessConfig {
            enabled: false,
            method: crate::types::ReadinessMethod::Api,
            initial_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(10),
            max_total_wait: Duration::from_millis(0),
            poll_interval: Duration::from_millis(1),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let mut reporter = CollectingReporter::default();
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let mut event_log = events::EventLog::new();
        let (ok, evidence) = verify_published(
            &reg,
            "demo",
            "0.1.0",
            &config,
            &mut reporter,
            &mut event_log,
            &events_path,
            "demo@0.1.0",
        )
        .expect("verify");
        assert!(
            ok,
            "disabled readiness with 200 response should return true"
        );
        assert_eq!(
            evidence.len(),
            1,
            "disabled readiness does exactly one check"
        );
        assert!(evidence[0].visible);
        server.join();
    }

    #[test]
    #[serial]
    fn run_publish_already_published_packages_skipped_in_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );
            let ws = planned_workspace(td.path(), server.base_url.clone());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let _ = run_publish(&ws, &opts, &mut reporter).expect("publish");

            // Verify state file reflects skipped
            let st = state::load_state(&td.path().join(".shipper"))
                .expect("load")
                .expect("exists");
            let pkg = st.packages.get("demo@0.1.0").expect("pkg");
            assert!(
                matches!(pkg.state, PackageState::Skipped { .. }),
                "expected Skipped in state, got {:?}",
                pkg.state
            );
            server.join();
        });
    }

    #[test]
    fn policy_effects_default_options() {
        let opts = default_opts(PathBuf::from(".shipper"));
        let effects = policy_effects(&opts);
        // Default policy is Balanced, which runs dry-run but skips ownership by default
        // (skip_ownership_check=true in default_opts)
        assert!(effects.run_dry_run);
    }

    // â”€â”€ Retry logic edge-case tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    #[serial]
    fn run_publish_zero_max_attempts_skips_publish_loop() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                // Registry says version doesn't exist, so engine enters the publish loop
                // with max_attempts=0 â†’ loop body never executes â†’ package stays Pending
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string())],
                    )]),
                    1,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 0;

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert_eq!(receipt.packages.len(), 1);
                assert!(matches!(receipt.packages[0].state, PackageState::Pending));

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert_eq!(pkg.attempts, 0, "no attempts should have been made");
                assert!(matches!(pkg.state, PackageState::Pending));
                server.join();
            },
        );
    }

    #[test]
    fn sequential_scheduler_uses_finite_package_timeout() {
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.parallel.enabled = false;
        opts.parallel.per_package_timeout = Duration::from_secs(17);

        assert_eq!(
            execute_package::sequential_cargo_timeout(&opts),
            Some(Duration::from_secs(17))
        );
    }

    #[test]
    #[serial]
    fn run_publish_max_retries_exceeded_marks_failed() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("HTTP 503 service unavailable".to_string()),
                ),
            ],
            || {
                // Initial precheck + three post-failure checks + one final poll.
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                        ],
                    )]),
                    5,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 3;
                opts.readiness.max_total_wait = Duration::ZERO;
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("failed"));

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert_eq!(pkg.attempts, 3, "should have exhausted all 3 attempts");
                assert!(
                    matches!(pkg.state, PackageState::Failed { .. }),
                    "expected Failed, got {:?}",
                    pkg.state
                );
                match &pkg.state {
                    PackageState::Failed { class, .. } => {
                        assert_eq!(
                            *class,
                            ErrorClass::Retryable,
                            "failed class should preserve retry classification"
                        );
                    }
                    _ => unreachable!("unreachable due guard above"),
                }
                assert_eq!(server.seen.lock().expect("requests").len(), 5);
                server.join();
            },
        );
    }

    #[test]
    #[serial]
    fn run_publish_single_attempt_succeeds_on_first_try() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 1;

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert_eq!(receipt.packages[0].attempts, 1);
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                server.join();
            },
        );
    }

    // â”€â”€ State persistence: Uploaded vs Published transitions â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn state_transition_pending_to_uploaded_persists() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        update_state(&mut st, &state_dir, key, PackageState::Uploaded).expect("update");

        // Verify in-memory
        assert!(matches!(
            st.packages.get(key).unwrap().state,
            PackageState::Uploaded
        ));

        // Verify on disk
        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert!(matches!(
            loaded.packages.get(key).unwrap().state,
            PackageState::Uploaded
        ));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn state_transition_uploaded_to_published_persists() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        // Pending -> Uploaded -> Published
        update_state(&mut st, &state_dir, key, PackageState::Uploaded).expect("upload");
        update_state(&mut st, &state_dir, key, PackageState::Published).expect("publish");

        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert!(matches!(
            loaded.packages.get(key).unwrap().state,
            PackageState::Published
        ));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn state_transition_pending_to_failed_persists() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";

        let failed = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "service unavailable".to_string(),
        };
        update_state(&mut st, &state_dir, key, failed).expect("update");

        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        match &loaded.packages.get(key).unwrap().state {
            PackageState::Failed { class, message } => {
                assert_eq!(*class, ErrorClass::Retryable);
                assert_eq!(message, "service unavailable");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn state_updated_at_advances_on_transition() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());

        let mut st = init_state(&ws, &state_dir).expect("init");
        let key = "demo@0.1.0";
        let initial_updated = st.updated_at;

        // Small sleep to ensure time difference
        thread::sleep(Duration::from_millis(10));

        update_state(&mut st, &state_dir, key, PackageState::Uploaded).expect("update");
        assert!(st.updated_at > initial_updated);

        let pkg = st.packages.get(key).unwrap();
        assert!(pkg.last_updated_at >= initial_updated);
    }

    // â”€â”€ Error classification tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    fn classify_cargo_failure_connection_refused_is_retryable() {
        let (class, _) = classify_cargo_failure("connection refused", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_cargo_failure_500_is_retryable() {
        let (class, _) = classify_cargo_failure("HTTP 500 internal server error", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_cargo_failure_version_exists_is_permanent() {
        let (class, _) = classify_cargo_failure("crate version `0.1.0` is already uploaded", "");
        assert_eq!(class, ErrorClass::Permanent);
    }

    #[test]
    fn classify_cargo_failure_empty_output_is_ambiguous() {
        let (class, _) = classify_cargo_failure("", "");
        assert_eq!(class, ErrorClass::Ambiguous);
    }

    #[test]
    fn classify_cargo_failure_message_is_nonempty() {
        let (_, msg) = classify_cargo_failure("timeout talking to server", "");
        assert!(!msg.is_empty(), "error message should be nonempty");
    }

    #[test]
    fn classify_cargo_failure_stderr_vs_stdout() {
        // Some errors appear in stdout, some in stderr
        let (class_stderr, _) = classify_cargo_failure("HTTP 429 too many requests", "");
        let (class_stdout, _) = classify_cargo_failure("", "HTTP 429 too many requests");
        assert_eq!(class_stderr, ErrorClass::Retryable);
        // stdout-only message should also be classified
        assert!(
            class_stdout == ErrorClass::Retryable || class_stdout == ErrorClass::Ambiguous,
            "expected Retryable or Ambiguous from stdout, got {class_stdout:?}"
        );
    }

    // â”€â”€ State machine transition tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// Helper: build a multi-package workspace for state machine tests.
    fn multi_package_workspace(
        workspace_root: &Path,
        api_base: String,
        packages: Vec<(&str, &str)>,
    ) -> PlannedWorkspace {
        PlannedWorkspace {
            workspace_root: workspace_root.to_path_buf(),
            plan: ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "plan-sm-test".to_string(),
                created_at: Utc::now(),
                registry: Registry {
                    name: "crates-io".to_string(),
                    index_base: Some(api_base.clone()),
                    api_base,
                },
                packages: packages
                    .iter()
                    .map(|(name, ver)| PlannedPackage {
                        name: name.to_string(),
                        version: ver.to_string(),
                        manifest_path: workspace_root.join(*name).join("Cargo.toml"),
                        regime: None,
                    })
                    .collect(),
                dependencies: std::collections::BTreeMap::new(),
            },
            skipped: vec![],
        }
    }

    // 1. Pending â†’ Published (happy path, single crate)
    #[test]
    #[serial]
    fn sm_pending_to_published_happy_path() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let opts = default_opts(PathBuf::from(".shipper"));

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                assert_eq!(receipt.packages.len(), 1);
                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                assert_eq!(receipt.packages[0].attempts, 1);

                // Verify state on disk matches
                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert!(matches!(pkg.state, PackageState::Published));
                server.join();
            },
        );
    }

    // 2. Pending â†’ Uploaded â†’ Verified (two-phase with readiness check)
    #[test]
    #[serial]
    fn sm_pending_uploaded_verified_two_phase() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                // First 404 = not yet published, second 200 = visible after readiness
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let opts = default_opts(PathBuf::from(".shipper"));

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                // Verify the publish-then-verify flow happened
                assert!(reporter.infos.iter().any(|i| i.contains("publishing")));
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("verifying") || i.contains("visible"))
                );
                server.join();
            },
        );
    }

    // 3. Pending â†’ Failed (permanent error, no retry)
    #[test]
    #[serial]
    fn sm_pending_to_failed_permanent_no_retry() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (404, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 5; // should not retry on permanent error
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("permanent failure"));

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                // Should have only attempted once (permanent = no retry)
                assert_eq!(pkg.attempts, 1);
                assert!(matches!(
                    pkg.state,
                    PackageState::Failed {
                        class: ErrorClass::Permanent,
                        ..
                    }
                ));
                server.join();
            },
        );
    }

    // 4. Pending â†’ Uploaded â†’ Failed (upload ok, verify failed)
    #[test]
    #[serial]
    fn sm_uploaded_then_verify_failed() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo.log");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || {
                // cargo succeeds, but registry never shows the version
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![
                            (404, "{}".to_string()), // initial version_exists
                            (404, "{}".to_string()), // readiness check
                            (404, "{}".to_string()), // final chance
                        ],
                    )]),
                    3,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 1;
                opts.readiness.max_total_wait = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("failed"));

                assert_unverified_accepted_upload(
                    &td.path().join(".shipper"),
                    "demo@0.1.0",
                    &cargo_log,
                );
                server.join();
            },
        );
    }

    // 5. Multiple packages: first succeeds, second fails â†’ partial progress saved
    #[test]
    #[serial]
    fn sm_multi_package_partial_progress() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo.log");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(cargo_log.to_string_lossy().into_owned()),
                ),
            ],
            || {
                // alpha: 404 then 200 (publishes ok)
                // beta: 404 then always 404 (readiness never passes)
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/alpha/1.0.0".to_string(),
                            vec![(404, "{}".to_string()), (200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/beta/2.0.0".to_string(),
                            vec![
                                (404, "{}".to_string()), // initial
                                (404, "{}".to_string()), // readiness
                                (404, "{}".to_string()), // final
                            ],
                        ),
                    ]),
                    5,
                );
                let ws = multi_package_workspace(
                    td.path(),
                    server.base_url.clone(),
                    vec![("alpha", "1.0.0"), ("beta", "2.0.0")],
                );
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 1;
                opts.readiness.max_total_wait = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("beta"));

                // Verify partial progress saved: alpha is Published
                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let alpha = st.packages.get("alpha@1.0.0").expect("alpha");
                assert!(
                    matches!(alpha.state, PackageState::Published),
                    "alpha should be Published, got {:?}",
                    alpha.state
                );
                assert_unverified_accepted_upload(
                    &td.path().join(".shipper"),
                    "beta@2.0.0",
                    &cargo_log,
                );
                server.join();
            },
        );
    }

    // 6. Resume from partial state â€” only remaining packages are attempted
    // 5b. Sequential scheduler synchronizes state on mid-run failures.
    #[test]
    #[serial]
    fn sm_sequential_scheduler_restores_state_on_mid_run_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        write_fake_tools(&bin);
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.max_attempts = 1;
        opts.readiness.max_total_wait = Duration::from_millis(0);
        write_fake_git(&bin);

        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
                // alpha: Uploaded on disk, registry says already visible (published);
                // beta: pending + failed cargo publish -> final state is Failed.
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/alpha/1.0.0".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/beta/2.0.0".to_string(),
                            vec![(404, "{}".to_string()), (404, "{}".to_string())],
                        ),
                    ]),
                    4,
                );
                let ws = multi_package_workspace(
                    td.path(),
                    server.base_url.clone(),
                    vec![("alpha", "1.0.0"), ("beta", "2.0.0")],
                );
                let state_dir = td.path().join(".shipper");
                let mut st = init_state(&ws, &state_dir).expect("init state");
                st.packages.insert(
                    "alpha@1.0.0".to_string(),
                    PackageProgress {
                        name: "alpha".to_string(),
                        version: "1.0.0".to_string(),
                        attempts: 1,
                        state: PackageState::Uploaded,
                        last_updated_at: Utc::now(),
                    },
                );
                let reg = test_registry_client(ws.plan.registry.clone()).expect("registry");
                let mut reporter = CollectingReporter::default();
                let mut event_log = events::EventLog::new();
                let events_path = events::events_path(&state_dir);

                let result = execute_package::run_sequential_scheduler(
                    &ws,
                    &opts,
                    &mut st,
                    &state_dir,
                    &reg,
                    &mut event_log,
                    &events_path,
                    &mut reporter,
                );
                let err = result.expect_err("scheduler should fail");
                let persisted = state::load_state(&state_dir)
                    .expect("load state")
                    .expect("state exists");

                assert!(
                    err.to_string().contains("beta"),
                    "expected failure to mention beta, got {err}"
                );
                assert!(
                    matches!(
                        st.packages.get("beta@2.0.0"),
                        Some(PackageProgress {
                            state: PackageState::Failed { .. },
                            ..
                        })
                    ),
                    "expected beta to be failed after stop (in-memory), got {:?}",
                    st.packages
                        .get("beta@2.0.0")
                        .expect("beta package state")
                        .state
                );
                assert!(
                    matches!(
                        st.packages.get("alpha@1.0.0"),
                        Some(PackageProgress {
                            state: PackageState::Published,
                            ..
                        })
                    ),
                    "expected alpha to remain published after partial progress, got {:?}",
                    st.packages
                        .get("alpha@1.0.0")
                        .expect("alpha package state")
                        .state
                );
                assert!(
                    matches!(
                        persisted.packages.get("beta@2.0.0"),
                        Some(PackageProgress {
                            state: PackageState::Failed { .. },
                            ..
                        })
                    ),
                    "expected beta to be failed after stop (on disk), got {:?}",
                    persisted
                        .packages
                        .get("beta@2.0.0")
                        .expect("beta package state")
                        .state
                );
                assert!(
                    matches!(
                        persisted.packages.get("alpha@1.0.0"),
                        Some(PackageProgress {
                            state: PackageState::Published,
                            ..
                        })
                    ),
                    "expected persisted alpha to remain published, got {:?}",
                    persisted
                        .packages
                        .get("alpha@1.0.0")
                        .expect("alpha package state")
                        .state
                );

                server.join();
            },
        );
    }

    // 5c. Sequential scheduler restores event-log ownership when skip write fails.
    #[test]
    #[serial]
    fn sm_sequential_scheduler_restores_event_log_on_skip_write_failure() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        fs::create_dir_all(&state_dir).expect("state dir");

        let events_path = events::events_path(&state_dir);
        fs::write(&events_path, "").expect("events file");
        let mut events_permissions = fs::metadata(&events_path)
            .expect("events metadata")
            .permissions();
        #[cfg(windows)]
        {
            events_permissions.set_readonly(true);
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            events_permissions.set_mode(0o444);
        }
        fs::set_permissions(&events_path, events_permissions).expect("events permissions");

        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let mut st = init_state(&ws, &state_dir).expect("init state");
        st.packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let reg = test_registry_client(ws.plan.registry.clone()).expect("registry");
        let opts = default_opts(PathBuf::from(".shipper"));
        let mut event_log = events::EventLog::new();
        let mut reporter = CollectingReporter::default();

        let result = execute_package::run_sequential_scheduler(
            &ws,
            &opts,
            &mut st,
            &state_dir,
            &reg,
            &mut event_log,
            &events_path,
            &mut reporter,
        );
        let err = result.expect_err("skip event write should fail");
        assert!(
            err.to_string()
                .contains("failed to write package-skip event")
                || err.to_string().contains("failed to write events")
                || err.to_string().contains("failed to open events file"),
            "expected events write failure, got {err}"
        );
        assert!(
            st.packages.get("demo@0.1.0").expect("demo package").state == PackageState::Published,
            "state should survive synchronized copy back"
        );
        assert!(
            event_log
                .all_events()
                .iter()
                .any(|event| matches!(event.event_type, EventType::PackageSkipped { .. })),
            "caller should still own the skipped event after write failure"
        );
    }

    #[test]
    #[serial]
    fn sm_resume_from_partial_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(args_log.to_str().expect("utf8").to_string()),
                ),
            ],
            || {
                // beta: 404 then 200 (publishes ok)
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/beta/2.0.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = multi_package_workspace(
                    td.path(),
                    server.base_url.clone(),
                    vec![("alpha", "1.0.0"), ("beta", "2.0.0")],
                );
                let state_dir = td.path().join(".shipper");

                // Pre-create state: alpha already Published
                let mut packages = std::collections::BTreeMap::new();
                packages.insert(
                    "alpha@1.0.0".to_string(),
                    PackageProgress {
                        name: "alpha".to_string(),
                        version: "1.0.0".to_string(),
                        attempts: 1,
                        state: PackageState::Published,
                        last_updated_at: Utc::now(),
                    },
                );
                packages.insert(
                    "beta@2.0.0".to_string(),
                    PackageProgress {
                        name: "beta".to_string(),
                        version: "2.0.0".to_string(),
                        attempts: 0,
                        state: PackageState::Pending,
                        last_updated_at: Utc::now(),
                    },
                );
                let st = ExecutionState {
                    state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    attempt_history: Vec::new(),
                    packages,
                };
                state::save_state(&state_dir, &st).expect("save");

                let opts = default_opts(PathBuf::from(".shipper"));
                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                // alpha should be skipped (already complete)
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("alpha") && i.contains("already complete"))
                );

                // alpha (already terminal) + beta (published this run)
                assert_eq!(receipt.packages.len(), 2);
                let beta = receipt
                    .packages
                    .iter()
                    .find(|p| p.name == "beta")
                    .expect("beta receipt");
                assert!(matches!(beta.state, PackageState::Published));
                let alpha = receipt
                    .packages
                    .iter()
                    .find(|p| p.name == "alpha")
                    .expect("alpha receipt");
                assert!(matches!(alpha.state, PackageState::Published));

                // cargo publish should only have been called for beta
                let log = fs::read_to_string(&args_log).unwrap_or_default();
                assert!(
                    !log.contains("alpha"),
                    "alpha should not have been published"
                );
                assert!(log.contains("beta"), "beta should have been published");

                server.join();
            },
        );
    }

    // 7. Plan ID mismatch on resume â€” verify rejection
    #[test]
    fn sm_plan_id_mismatch_rejected() {
        let td = tempdir().expect("tempdir");
        let ws = multi_package_workspace(
            td.path(),
            "http://127.0.0.1:9".to_string(),
            vec![("demo", "0.1.0")],
        );
        let state_dir = td.path().join(".shipper");

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "completely-different-plan".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let opts = default_opts(PathBuf::from(".shipper"));
        let mut reporter = CollectingReporter::default();
        let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("does not match current plan_id"), "got: {msg}");
    }

    // 8. Empty package list â€” verify graceful handling
    #[test]
    fn sm_empty_package_list_graceful() {
        let td = tempdir().expect("tempdir");
        let ws = multi_package_workspace(
            td.path(),
            "http://127.0.0.1:9".to_string(),
            vec![], // no packages
        );
        let opts = default_opts(PathBuf::from(".shipper"));

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        assert!(receipt.packages.is_empty());
    }

    // 9. Dry-run mode: cargo publishes but readiness check prevents registering
    //    We test that when no_verify is set, the publish still proceeds normally.
    //    (The engine doesn't have an explicit dry-run mode separate from no_verify;
    //    "dry-run" is a preflight concept. We test that no_verify flag is respected.)
    #[test]
    #[serial]
    fn sm_no_verify_flag_respected() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(args_log.to_str().expect("utf8").to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.no_verify = true;

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
                assert!(matches!(receipt.packages[0].state, PackageState::Published));

                // Verify cargo was called with the right flags
                let log = fs::read_to_string(&args_log).unwrap_or_default();
                assert!(
                    log.contains("publish"),
                    "cargo publish should have been called"
                );
                server.join();
            },
        );
    }

    // 10. Max retries exceeded â€” verify proper error with attempt count
    #[test]
    #[serial]
    fn sm_max_retries_exceeded_attempt_count() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout talking to server".to_string()),
                ),
            ],
            || {
                // Initial precheck + three post-failure checks + one final poll.
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                            (404, "{}".to_string()),
                        ],
                    )]),
                    5,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.max_attempts = 3;
                opts.readiness.max_total_wait = Duration::ZERO;
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let err = run_publish(&ws, &opts, &mut reporter).expect_err("must fail");
                assert!(format!("{err:#}").contains("failed"));

                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert_eq!(pkg.attempts, 3, "should have made exactly 3 attempts");
                assert!(matches!(pkg.state, PackageState::Failed { .. }));

                // Verify reporter logged all retry attempts
                let attempt_msgs: Vec<_> = reporter
                    .infos
                    .iter()
                    .filter(|i| i.contains("attempt"))
                    .collect();
                assert!(
                    attempt_msgs.len() >= 3,
                    "expected at least 3 attempt messages, got {}: {:?}",
                    attempt_msgs.len(),
                    attempt_msgs
                );
                assert_eq!(server.seen.lock().expect("requests").len(), 5);
                server.join();
            },
        );
    }

    // 11. Timeout during publish â€” verify state preservation
    #[test]
    #[serial]
    fn sm_timeout_preserves_state() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("timeout while uploading".to_string()),
                ),
            ],
            || {
                // cargo fails with timeout, but registry shows the version exists
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let mut opts = default_opts(PathBuf::from(".shipper"));
                opts.base_delay = Duration::from_millis(0);
                opts.max_delay = Duration::from_millis(0);

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                // Even though cargo reported failure, registry check found the version
                assert!(
                    matches!(receipt.packages[0].state, PackageState::Published),
                    "expected Published after timeout recovery, got {:?}",
                    receipt.packages[0].state
                );

                // State file on disk should also reflect Published
                let st = state::load_state(&td.path().join(".shipper"))
                    .expect("load")
                    .expect("exists");
                let pkg = st.packages.get("demo@0.1.0").expect("pkg");
                assert!(matches!(pkg.state, PackageState::Published));
                server.join();
            },
        );
    }

    // 12. Concurrent package independence â€” parallel packages don't affect each other
    //     (sequential mode: verify each package state is independent)
    #[test]
    #[serial]
    fn sm_package_independence_sequential() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/alpha/1.0.0".to_string(),
                            vec![(200, "{}".to_string())], // already published
                        ),
                        (
                            "/api/v1/crates/beta/2.0.0".to_string(),
                            vec![(404, "{}".to_string()), (200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/gamma/3.0.0".to_string(),
                            vec![(200, "{}".to_string())], // already published
                        ),
                    ]),
                    4,
                );
                let ws = multi_package_workspace(
                    td.path(),
                    server.base_url.clone(),
                    vec![("alpha", "1.0.0"), ("beta", "2.0.0"), ("gamma", "3.0.0")],
                );
                let opts = default_opts(PathBuf::from(".shipper"));

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                assert_eq!(receipt.packages.len(), 3);
                // alpha: skipped (already published)
                assert!(
                    matches!(receipt.packages[0].state, PackageState::Skipped { .. }),
                    "alpha should be Skipped, got {:?}",
                    receipt.packages[0].state
                );
                // beta: published
                assert!(
                    matches!(receipt.packages[1].state, PackageState::Published),
                    "beta should be Published, got {:?}",
                    receipt.packages[1].state
                );
                // gamma: skipped (already published)
                assert!(
                    matches!(receipt.packages[2].state, PackageState::Skipped { .. }),
                    "gamma should be Skipped, got {:?}",
                    receipt.packages[2].state
                );
                server.join();
            },
        );
    }

    // 13. Resume from Uploaded state skips cargo publish, goes straight to verify
    #[test]
    #[serial]
    fn sm_resume_from_uploaded_skips_cargo() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let args_log = td.path().join("cargo_args.txt");
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("0".to_string())),
                (
                    "SHIPPER_CARGO_ARGS_LOG",
                    Some(args_log.to_str().expect("utf8").to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (200, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let state_dir = td.path().join(".shipper");

                // Pre-create state with Uploaded
                let mut packages = std::collections::BTreeMap::new();
                packages.insert(
                    "demo@0.1.0".to_string(),
                    PackageProgress {
                        name: "demo".to_string(),
                        version: "0.1.0".to_string(),
                        attempts: 1,
                        state: PackageState::Uploaded,
                        last_updated_at: Utc::now(),
                    },
                );
                let st = ExecutionState {
                    state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    attempt_history: Vec::new(),
                    packages,
                };
                state::save_state(&state_dir, &st).expect("save");

                let opts = default_opts(PathBuf::from(".shipper"));
                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                assert!(matches!(receipt.packages[0].state, PackageState::Published));
                assert!(
                    reporter
                        .infos
                        .iter()
                        .any(|i| i.contains("resuming from uploaded"))
                );

                // cargo publish should NOT have been called
                let cargo_called = args_log.exists()
                    && fs::read_to_string(&args_log)
                        .unwrap_or_default()
                        .contains("publish");
                assert!(
                    !cargo_called,
                    "cargo publish should not run on resume from Uploaded"
                );
                server.join();
            },
        );
    }

    // 14. Failed package produces correct event log entries
    #[test]
    #[serial]
    fn sm_failed_package_event_log() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![
                ("SHIPPER_CARGO_EXIT", Some("1".to_string())),
                (
                    "SHIPPER_CARGO_STDERR",
                    Some("permission denied".to_string()),
                ),
            ],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([(
                        "/api/v1/crates/demo/0.1.0".to_string(),
                        vec![(404, "{}".to_string()), (404, "{}".to_string())],
                    )]),
                    2,
                );
                let ws = planned_workspace(td.path(), server.base_url.clone());
                let opts = default_opts(PathBuf::from(".shipper"));

                let mut reporter = CollectingReporter::default();
                let _ = run_publish(&ws, &opts, &mut reporter);

                let events_path = td.path().join(".shipper").join("events.jsonl");
                let log = crate::state::events::EventLog::read_from_file(&events_path)
                    .expect("read events");
                let events = log.all_events();

                // Must have: ExecutionStarted, PlanCreated, PackageStarted, PackageAttempted, PackageFailed
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::ExecutionStarted))
                );
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PlanCreated { .. }))
                );
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PackageStarted { .. }))
                );
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PackageAttempted { .. }))
                );
                assert!(
                    events
                        .iter()
                        .any(|e| matches!(e.event_type, EventType::PackageFailed { .. }))
                );
                server.join();
            },
        );
    }

    // 15. State file persists across transitions with correct version
    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn sm_state_version_preserved_through_transitions() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = multi_package_workspace(
            td.path(),
            "http://127.0.0.1:9".to_string(),
            vec![("alpha", "1.0.0"), ("beta", "2.0.0")],
        );

        let mut st = init_state(&ws, &state_dir).expect("init");
        assert_eq!(
            st.state_version,
            crate::state::execution_state::CURRENT_STATE_VERSION
        );

        // Transition alpha: Pending â†’ Uploaded â†’ Published
        update_state(&mut st, &state_dir, "alpha@1.0.0", PackageState::Uploaded).expect("update");
        update_state(&mut st, &state_dir, "alpha@1.0.0", PackageState::Published).expect("update");

        // Transition beta: Pending â†’ Failed
        update_state(
            &mut st,
            &state_dir,
            "beta@2.0.0",
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "denied".to_string(),
            },
        )
        .expect("update");

        let loaded = state::load_state(&state_dir)
            .expect("load")
            .expect("exists");
        assert_eq!(
            loaded.state_version,
            crate::state::execution_state::CURRENT_STATE_VERSION
        );
        assert!(matches!(
            loaded.packages.get("alpha@1.0.0").unwrap().state,
            PackageState::Published
        ));
        assert!(matches!(
            loaded.packages.get("beta@2.0.0").unwrap().state,
            PackageState::Failed { .. }
        ));
    }

    // 16. Snapshot: receipt after successful multi-package publish
    #[test]
    #[serial]
    fn sm_snapshot_receipt_multi_package() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        with_test_env(
            &bin,
            vec![("SHIPPER_CARGO_EXIT", Some("0".to_string()))],
            || {
                let server = spawn_registry_server(
                    std::collections::BTreeMap::from([
                        (
                            "/api/v1/crates/alpha/1.0.0".to_string(),
                            vec![(404, "{}".to_string()), (200, "{}".to_string())],
                        ),
                        (
                            "/api/v1/crates/beta/2.0.0".to_string(),
                            vec![(200, "{}".to_string())],
                        ),
                    ]),
                    3,
                );
                let ws = multi_package_workspace(
                    td.path(),
                    server.base_url.clone(),
                    vec![("alpha", "1.0.0"), ("beta", "2.0.0")],
                );
                let opts = default_opts(PathBuf::from(".shipper"));

                let mut reporter = CollectingReporter::default();
                let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");

                // Snapshot the package states (redact timestamps)
                let snapshot: Vec<String> = receipt
                    .packages
                    .iter()
                    .map(|p| {
                        format!(
                            "name={} version={} attempts={} state={}",
                            p.name,
                            p.version,
                            p.attempts,
                            short_state(&p.state)
                        )
                    })
                    .collect();
                insta::assert_debug_snapshot!("sm_receipt_multi_package", snapshot);
                server.join();
            },
        );
    }

    // 17. Snapshot: state after partial failure
    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn sm_snapshot_state_partial_failure() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let ws = multi_package_workspace(
            td.path(),
            "http://127.0.0.1:9".to_string(),
            vec![("alpha", "1.0.0"), ("beta", "2.0.0"), ("gamma", "3.0.0")],
        );

        let mut st = init_state(&ws, &state_dir).expect("init");

        // alpha: Published, beta: Uploaded (in-progress), gamma: still Pending
        update_state(&mut st, &state_dir, "alpha@1.0.0", PackageState::Published).expect("update");
        update_state(&mut st, &state_dir, "beta@2.0.0", PackageState::Uploaded).expect("update");

        let snapshot: Vec<String> = st
            .packages
            .iter()
            .map(|(k, v)| {
                format!(
                    "key={} attempts={} state={}",
                    k,
                    v.attempts,
                    short_state(&v.state)
                )
            })
            .collect();
        insta::assert_debug_snapshot!("sm_state_partial_failure", snapshot);
    }

    // 18. Force resume with plan mismatch proceeds with warning
    #[test]
    fn sm_force_resume_with_mismatch() {
        let td = tempdir().expect("tempdir");
        let ws = multi_package_workspace(
            td.path(),
            "http://127.0.0.1:9".to_string(),
            vec![("demo", "0.1.0")],
        );
        let state_dir = td.path().join(".shipper");

        // Create state with different plan_id but all packages Published
        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st = ExecutionState {
            state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "old-plan-id".to_string(),
            registry: ws.plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: Vec::new(),
            packages,
        };
        state::save_state(&state_dir, &st).expect("save");

        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.force_resume = true;

        let mut reporter = CollectingReporter::default();
        let receipt = run_publish(&ws, &opts, &mut reporter).expect("publish");
        // Terminal gate (#153) produces a resume-skip receipt for the already-
        // complete package rather than omitting it from the receipt list.
        assert_eq!(receipt.packages.len(), 1);
        assert_eq!(receipt.packages[0].name, "demo");
        assert!(matches!(receipt.packages[0].state, PackageState::Published));
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("forcing resume with mismatched plan_id"))
        );
        assert!(
            reporter
                .infos
                .iter()
                .any(|i| i.contains("already complete") && i.contains("demo")),
            "expected already-complete narration, got: {:?}",
            reporter.infos
        );
    }

    // â”€â”€ Snapshot tests with insta for engine state â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    fn snapshot_init_state_single_package() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        let state_dir = td.path().join(".shipper");

        let st = init_state(&ws, &state_dir).expect("init");

        // Redact timestamps for deterministic snapshots
        let snapshot: Vec<String> = st
            .packages
            .iter()
            .map(|(k, v)| {
                format!(
                    "key={} name={} version={} attempts={} state={}",
                    k,
                    v.name,
                    v.version,
                    v.attempts,
                    short_state(&v.state)
                )
            })
            .collect();
        insta::assert_debug_snapshot!("init_state_single_package", snapshot);
    }

    #[test]
    fn snapshot_init_state_multi_package() {
        let td = tempdir().expect("tempdir");
        let mut ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        ws.plan.packages = vec![
            PlannedPackage {
                name: "alpha".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: td.path().join("alpha/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "beta".to_string(),
                version: "2.0.0".to_string(),
                manifest_path: td.path().join("beta/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "gamma".to_string(),
                version: "0.3.0".to_string(),
                manifest_path: td.path().join("gamma/Cargo.toml"),
                regime: None,
            },
        ];
        let state_dir = td.path().join(".shipper");

        let st = init_state(&ws, &state_dir).expect("init");

        let snapshot: Vec<String> = st
            .packages
            .iter()
            .map(|(k, v)| {
                format!(
                    "key={} name={} version={} attempts={} state={}",
                    k,
                    v.name,
                    v.version,
                    v.attempts,
                    short_state(&v.state)
                )
            })
            .collect();
        insta::assert_debug_snapshot!("init_state_multi_package", snapshot);
    }

    #[test]
    #[expect(
        deprecated,
        reason = "legacy compatibility coverage for quarantined state-only helper"
    )]
    fn snapshot_state_after_transitions() {
        let td = tempdir().expect("tempdir");
        let mut ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
        ws.plan.packages = vec![
            PlannedPackage {
                name: "alpha".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: td.path().join("alpha/Cargo.toml"),
                regime: None,
            },
            PlannedPackage {
                name: "beta".to_string(),
                version: "2.0.0".to_string(),
                manifest_path: td.path().join("beta/Cargo.toml"),
                regime: None,
            },
        ];
        let state_dir = td.path().join(".shipper");
        let mut st = init_state(&ws, &state_dir).expect("init");

        // Simulate: alpha published, beta failed
        update_state(&mut st, &state_dir, "alpha@1.0.0", PackageState::Published).expect("update");
        update_state(
            &mut st,
            &state_dir,
            "beta@2.0.0",
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth failure".to_string(),
            },
        )
        .expect("update");

        let snapshot: Vec<String> = st
            .packages
            .iter()
            .map(|(k, v)| {
                format!(
                    "key={} attempts={} state={}",
                    k,
                    v.attempts,
                    short_state(&v.state)
                )
            })
            .collect();
        insta::assert_debug_snapshot!("state_after_mixed_transitions", snapshot);
    }

    #[test]
    fn snapshot_error_class_classification_matrix() {
        let cases = vec![
            ("HTTP 429 too many requests", ""),
            ("timeout talking to server", ""),
            ("HTTP 503 service unavailable", ""),
            ("connection refused", ""),
            ("HTTP 500 internal server error", ""),
            ("permission denied", ""),
            ("crate version `0.1.0` is already uploaded", ""),
            ("something totally unexpected", ""),
            ("", ""),
        ];

        let snapshot: Vec<String> = cases
            .iter()
            .map(|(stderr, stdout)| {
                let (class, msg) = classify_cargo_failure(stderr, stdout);
                format!(
                    "stderr={:50} class={:10} msg={}",
                    format!("{stderr:?}"),
                    format!("{class:?}"),
                    msg
                )
            })
            .collect();
        insta::assert_debug_snapshot!("error_classification_matrix", snapshot);
    }

    // â”€â”€ Proptest: state transition invariants â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    mod engine_proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_error_class() -> impl Strategy<Value = ErrorClass> {
            prop_oneof![
                Just(ErrorClass::Retryable),
                Just(ErrorClass::Permanent),
                Just(ErrorClass::Ambiguous),
            ]
        }

        fn arb_package_state() -> impl Strategy<Value = PackageState> {
            prop_oneof![
                Just(PackageState::Pending),
                Just(PackageState::Uploaded),
                Just(PackageState::Published),
                ".*".prop_map(|r| PackageState::Skipped { reason: r }),
                (arb_error_class(), ".*").prop_map(|(c, m)| PackageState::Failed {
                    class: c,
                    message: m
                }),
                ".*".prop_map(|m| PackageState::Ambiguous { message: m }),
            ]
        }

        proptest! {
            /// update_state always persists new_state to disk
            #[test]
            #[expect(deprecated, reason = "legacy compatibility coverage for quarantined state-only helper")]
            fn update_state_always_persists(new_state in arb_package_state()) {
                let td = tempdir().expect("tempdir");
                let state_dir = td.path().join(".shipper");
                let ws = planned_workspace(td.path(), "http://127.0.0.1:9".to_string());
                let mut st = init_state(&ws, &state_dir).expect("init");
                let key = "demo@0.1.0";

                update_state(&mut st, &state_dir, key, new_state.clone()).expect("update");

                // In-memory matches
                assert_eq!(st.packages.get(key).unwrap().state, new_state);

                // On-disk matches
                let loaded = state::load_state(&state_dir)
                    .expect("load")
                    .expect("exists");
                assert_eq!(loaded.packages.get(key).unwrap().state, new_state);
            }

            /// short_state never panics on any PackageState variant
            #[test]
            fn short_state_never_panics(state in arb_package_state()) {
                let label = short_state(&state);
                assert!(!label.is_empty());
            }

            /// pkg_key is deterministic and reversible
            #[test]
            fn pkg_key_deterministic(
                name in "[a-z][a-z0-9_-]{0,19}",
                version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}"
            ) {
                let key1 = pkg_key(&name, &version);
                let key2 = pkg_key(&name, &version);
                assert_eq!(key1, key2);
                assert!(key1.contains('@'));
                assert!(key1.starts_with(&name));
                assert!(key1.ends_with(&version));
            }

            /// backoff_delay is always bounded by [0, max + jitter headroom]
            #[test]
            fn backoff_delay_bounded(
                base_ms in 1u64..5000,
                max_ms in 100u64..30000,
                attempt in 1u32..50,
                jitter in 0.0f64..1.0,
            ) {
                let base = Duration::from_millis(base_ms.min(max_ms));
                let max = Duration::from_millis(max_ms);

                let delay = backoff_delay(
                    base,
                    max,
                    attempt,
                    crate::retry::RetryStrategyType::Exponential,
                    jitter,
                );

                // With jitter, delay can be at most max * (1 + jitter)
                let upper_bound_ms = (max_ms as f64 * (1.0 + jitter)).ceil() as u64 + 1;
                assert!(
                    delay.as_millis() <= upper_bound_ms as u128,
                    "delay {}ms exceeded upper bound {}ms (base={}ms, max={}ms, attempt={}, jitter={})",
                    delay.as_millis(), upper_bound_ms, base_ms, max_ms, attempt, jitter
                );
            }

            /// ExecutionState roundtrips through JSON for arbitrary states
            #[test]
            fn execution_state_roundtrip(
                attempts in 0u32..100,
                state in arb_package_state()
            ) {
                let mut packages = BTreeMap::new();
                packages.insert(
                    "test@1.0.0".to_string(),
                    PackageProgress {
                        name: "test".to_string(),
                        version: "1.0.0".to_string(),
                        attempts,
                        state,
                        last_updated_at: Utc::now(),
                    },
                );
                let st = ExecutionState {
                    state_version: crate::state::execution_state::CURRENT_STATE_VERSION.to_string(),
                    plan_id: "plan-proptest".to_string(),
                    registry: Registry {
                        name: "crates-io".to_string(),
                        api_base: "https://crates.io".to_string(),
                        index_base: None,
                    },
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    attempt_history: Vec::new(),
                    packages,
                };

                let json = serde_json::to_string(&st).expect("serialize");
                let parsed: ExecutionState = serde_json::from_str(&json).expect("deserialize");
                assert_eq!(parsed.packages.len(), 1);
                let pkg = parsed.packages.get("test@1.0.0").unwrap();
                assert_eq!(pkg.attempts, attempts);
            }
        }
    }

    // â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    // run_rehearsal (#97 PR 2) â€” phase-2 preflight against an alt registry
    // â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn read_events_raw(state_dir: &Path) -> Vec<serde_json::Value> {
        let path = events::events_path(state_dir);
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("events.jsonl must parse"))
            .collect()
    }

    fn event_discriminator(event: &serde_json::Value) -> Option<String> {
        event
            .get("event_type")
            .and_then(|et| et.get("type"))
            .and_then(|t| t.as_str())
            .map(str::to_owned)
    }

    #[test]
    #[serial]
    fn run_rehearsal_errors_when_no_rehearsal_registry_configured() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let opts = default_opts(PathBuf::from(".shipper"));

            let mut reporter = CollectingReporter::default();
            let err = run_rehearsal(&ws, &opts, &mut reporter).expect_err("must fail");
            let msg = format!("{err:#}");
            assert!(msg.contains("no rehearsal registry"), "err was: {msg}");
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_errors_when_rehearsal_equals_live_target() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            // Point rehearsal at the same registry name as the live target.
            opts.rehearsal_registry = Some("crates-io".to_string());
            opts.registries = vec![Registry {
                name: "crates-io".to_string(),
                api_base: "http://127.0.0.1:2".to_string(),
                index_base: Some("http://127.0.0.1:2".to_string()),
            }];

            let mut reporter = CollectingReporter::default();
            let err = run_rehearsal(&ws, &opts, &mut reporter).expect_err("must fail");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("registry name matches the live target"),
                "err was: {msg}"
            );
            assert!(
                !td.path().join(".shipper").exists(),
                "same-name rejection must precede rehearsal evidence"
            );
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_rejects_authority_alias_before_state_cargo_or_network() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let cargo_log = td.path().join("cargo.log");
        let state_dir = td.path().join("evidence");

        let server = Server::http("127.0.0.1:0").expect("server");
        let base_url = format!("http://{}", server.server_addr());
        let network = thread::spawn(move || match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(request)) => {
                request
                    .respond(Response::from_string("{}"))
                    .expect("respond");
                true
            }
            Ok(None) | Err(_) => false,
        });

        with_test_env(
            &bin,
            vec![(
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().into_owned()),
            )],
            || {
                let mut ws = planned_workspace(td.path(), base_url.clone());
                ws.plan.registry.name = "live".to_string();
                let mut opts = default_opts(state_dir.clone());
                opts.registry_policies.insert(
                    "live".to_string(),
                    RegistryTrustOptions {
                        allow_private: false,
                        allow_loopback: true,
                    },
                );
                opts.rehearsal_registry = Some("alias".to_string());
                opts.registries = vec![Registry {
                    name: "alias".to_string(),
                    api_base: format!("{base_url}/different-path"),
                    index_base: Some(format!("{base_url}/different-index-path")),
                }];

                let mut reporter = CollectingReporter::default();
                let error = run_rehearsal(&ws, &opts, &mut reporter)
                    .expect_err("same-authority alias must fail");
                assert!(error.to_string().contains("authority overlaps"));
                assert!(!state_dir.exists(), "guard must precede state mutation");
                assert!(
                    !cargo_log.exists(),
                    "guard must precede Cargo publish/install"
                );
            },
        );

        assert!(
            !network.join().expect("network observer"),
            "guard must precede registry requests"
        );
    }

    #[test]
    #[serial]
    fn run_rehearsal_rejects_differently_named_crates_io_alias() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let mut ws = planned_workspace(td.path(), "https://registry.example".into());
            ws.plan.registry.name = "live".to_string();
            ws.plan.registry.index_base = Some("https://index.registry.example".to_string());
            let state_dir = td.path().join("evidence");
            let mut opts = default_opts(state_dir.clone());
            opts.rehearsal_registry = Some("public-alias".to_string());
            opts.registries = vec![Registry {
                name: "public-alias".to_string(),
                api_base: "https://rehearsal.crates.io:444/api".to_string(),
                index_base: Some("https://index.rehearsal.crates.io:444/index".to_string()),
            }];

            let mut reporter = CollectingReporter::default();
            let error = run_rehearsal(&ws, &opts, &mut reporter)
                .expect_err("crates.io family alias must fail");
            assert!(error.to_string().contains("crates.io family"));
            assert!(!state_dir.exists());
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_errors_when_registry_name_not_in_config() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.rehearsal_registry = Some("bogus-registry".to_string());
            // opts.registries is empty â€” bogus-registry won't resolve.

            let mut reporter = CollectingReporter::default();
            let err = run_rehearsal(&ws, &opts, &mut reporter).expect_err("must fail");
            let msg = format!("{err:#}");
            assert!(msg.contains("is not configured"), "err was: {msg}");
            assert!(
                !td.path().join(".shipper").exists(),
                "missing catalog entry must fail before rehearsal evidence"
            );
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_skip_flag_returns_without_running() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.rehearsal_registry = Some("rehearsal".to_string());
            opts.rehearsal_skip = true;

            let mut reporter = CollectingReporter::default();
            let outcome =
                run_rehearsal(&ws, &opts, &mut reporter).expect("skip path should not error");
            assert!(!outcome.passed, "skip should not claim a pass");
            assert_eq!(outcome.packages_published, 0);
            assert!(outcome.summary.contains("skipped"));
            // Skip path must not write events â€” nothing to audit.
            let events_path = events::events_path(&td.path().join(".shipper"));
            assert!(
                !events_path.exists(),
                "skip path must not create events.jsonl"
            );
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_happy_path_emits_started_published_complete_events() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let live_server = Server::http("127.0.0.1:0").expect("live server");
            let live_url = format!("http://{}", live_server.server_addr());
            let live_network =
                thread::spawn(
                    move || match live_server.recv_timeout(Duration::from_secs(1)) {
                        Ok(Some(request)) => {
                            request
                                .respond(Response::from_string("{}"))
                                .expect("respond");
                            true
                        }
                        Ok(None) | Err(_) => false,
                    },
                );
            // Rehearsal-registry mock: returns 404 for the preflight lookup
            // (not here) and 200 for the post-publish visibility check.
            let rehearsal_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );

            let mut ws = planned_workspace(td.path(), live_url);
            ws.plan.registry.name = "live".to_string();
            ws.plan.registry.index_base = Some(ws.plan.registry.api_base.clone());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.registry_policies.insert(
                "live".to_string(),
                RegistryTrustOptions {
                    allow_private: false,
                    allow_loopback: true,
                },
            );
            opts.rehearsal_registry = Some("rehearsal".to_string());
            opts.registries = vec![Registry {
                name: "rehearsal".to_string(),
                api_base: rehearsal_server.base_url.clone(),
                index_base: Some(rehearsal_server.base_url.clone()),
            }];

            let mut reporter = CollectingReporter::default();
            let outcome = run_rehearsal(&ws, &opts, &mut reporter).expect("rehearse");
            assert!(outcome.passed, "outcome: {outcome:?}");
            assert_eq!(outcome.packages_published, 1);

            let events = read_events_raw(&td.path().join(".shipper"));
            let types: Vec<String> = events.iter().filter_map(event_discriminator).collect();
            assert!(
                types.contains(&"rehearsal_started".to_string()),
                "types: {types:?}"
            );
            assert!(
                types.contains(&"registry_policy_applied".to_string()),
                "rehearsal must persist the applied registry trust posture: {types:?}"
            );
            assert!(
                types.contains(&"rehearsal_package_published".to_string()),
                "types: {types:?}"
            );
            assert!(
                types.contains(&"rehearsal_complete".to_string()),
                "types: {types:?}"
            );

            // RehearsalComplete must carry passed=true for the happy path.
            let complete = events
                .iter()
                .find(|e| event_discriminator(e).as_deref() == Some("rehearsal_complete"))
                .expect("RehearsalComplete event");
            assert_eq!(
                complete["event_type"]["passed"].as_bool(),
                Some(true),
                "complete event: {complete}"
            );

            rehearsal_server.join();
            assert!(
                !live_network.join().expect("live network observer"),
                "rehearsal must not dispatch requests to the live registry"
            );
        });
    }

    // â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    // enforce_rehearsal_gate (#97 PR 3)
    // â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    fn write_rehearsal_receipt(
        state_dir: &Path,
        plan_id: &str,
        passed: bool,
    ) -> crate::state::rehearsal::RehearsalReceipt {
        let receipt = crate::state::rehearsal::RehearsalReceipt {
            schema_version: crate::state::rehearsal::CURRENT_REHEARSAL_VERSION.to_string(),
            plan_id: plan_id.to_string(),
            registry: "rehearsal".to_string(),
            passed,
            packages_attempted: 1,
            packages_published: if passed { 1 } else { 0 },
            summary: if passed {
                "rehearsed 1 package successfully".into()
            } else {
                "rehearsal failed".into()
            },
            started_at: Utc::now(),
            completed_at: Utc::now(),
        };
        crate::state::rehearsal::save_rehearsal(state_dir, &receipt).expect("write");
        receipt
    }

    #[test]
    fn gate_is_dormant_when_rehearsal_registry_is_none() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let opts = default_opts(PathBuf::from(".shipper"));
        // opts.rehearsal_registry is None by default.
        let mut reporter = CollectingReporter::default();
        rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect("gate dormant");
    }

    #[test]
    fn gate_proceeds_with_warning_when_skip_is_set() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.rehearsal_registry = Some("rehearsal".into());
        opts.rehearsal_skip = true;
        let mut reporter = CollectingReporter::default();
        rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect("skip bypass");
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("--skip-rehearsal")),
            "warns: {:?}",
            reporter.warns
        );
    }

    #[test]
    fn gate_refuses_when_no_receipt_exists() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.rehearsal_registry = Some("rehearsal".into());
        let mut reporter = CollectingReporter::default();
        let err =
            rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("no rehearsal receipt was found"), "err: {msg}");
        assert!(
            msg.contains("shipper rehearse --registries rehearsal --rehearsal-registry rehearsal"),
            "err should hint fix: {msg}"
        );
    }

    #[test]
    fn gate_refuses_on_plan_id_mismatch() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.rehearsal_registry = Some("rehearsal".into());

        write_rehearsal_receipt(td.path(), "some-other-plan", true);

        let mut reporter = CollectingReporter::default();
        let err =
            rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("stale"), "err: {msg}");
        assert!(
            msg.contains(&ws.plan.plan_id),
            "err should reference current plan_id: {msg}"
        );
    }

    #[test]
    fn gate_refuses_on_failing_receipt() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.rehearsal_registry = Some("rehearsal".into());

        write_rehearsal_receipt(td.path(), &ws.plan.plan_id, false);

        let mut reporter = CollectingReporter::default();
        let err =
            rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("did NOT pass"), "err: {msg}");
    }

    #[test]
    fn gate_passes_on_fresh_passing_receipt() {
        let td = tempdir().expect("tempdir");
        let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
        let mut opts = default_opts(PathBuf::from(".shipper"));
        opts.rehearsal_registry = Some("rehearsal".into());

        write_rehearsal_receipt(td.path(), &ws.plan.plan_id, true);

        let mut reporter = CollectingReporter::default();
        rehearsal::enforce_gate(&ws, &opts, td.path(), &mut reporter).expect("fresh pass");
        assert!(
            reporter.infos.iter().any(|i| i.contains("passing receipt")),
            "infos: {:?}",
            reporter.infos
        );
    }

    /// End-to-end: `run_publish` refuses to run when rehearsal is required
    /// but no receipt exists. This is the gate's actual contract â€” the
    /// finer-grained tests above exercise the gate helper in isolation;
    /// this one confirms run_publish wires it in correctly.
    #[test]
    #[serial]
    fn run_publish_refuses_without_rehearsal_when_required() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let state_dir = td.path().join(".shipper");
            let mut opts = default_opts(state_dir);
            opts.rehearsal_registry = Some("rehearsal".into());

            let mut reporter = CollectingReporter::default();
            let err = run_publish(&ws, &opts, &mut reporter).expect_err("gate must bail");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("rehearsal is required") || msg.contains("no rehearsal receipt"),
                "expected gate error, got: {msg}"
            );
        });
    }

    /// #97 PR 4 â€” smoke-install happy path. --smoke-install names a
    /// crate in the plan; fake cargo returns 0 for the install call;
    /// rehearsal emits smoke-check events and passes.
    #[test]
    #[serial]
    fn run_rehearsal_smoke_install_happy_path_emits_succeeded_event() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let rehearsal_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );

            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.rehearsal_registry = Some("rehearsal".to_string());
            opts.registries = vec![Registry {
                name: "rehearsal".to_string(),
                api_base: rehearsal_server.base_url.clone(),
                index_base: Some(rehearsal_server.base_url.clone()),
            }];
            opts.rehearsal_smoke_install = Some("demo".to_string());

            let mut reporter = CollectingReporter::default();
            let outcome = run_rehearsal(&ws, &opts, &mut reporter).expect("rehearse");
            assert!(outcome.passed, "outcome: {outcome:?}");

            let events = read_events_raw(&td.path().join(".shipper"));
            let types: Vec<String> = events.iter().filter_map(event_discriminator).collect();
            assert!(
                types.contains(&"rehearsal_smoke_check_started".to_string()),
                "types: {types:?}"
            );
            assert!(
                types.contains(&"rehearsal_smoke_check_succeeded".to_string()),
                "types: {types:?}"
            );
            rehearsal_server.join();
        });
    }

    /// #97 PR 4 â€” smoke-install named a crate not in the plan. Warn-only
    /// path: rehearsal itself still passes (publish was fine) but the
    /// reporter surfaces the misconfiguration so the operator can fix it.
    #[test]
    #[serial]
    fn run_rehearsal_smoke_install_missing_target_warns_without_failing() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let env_vars = fake_program_env_vars(&bin);
        temp_env::with_vars(env_vars, || {
            let rehearsal_server = spawn_registry_server(
                std::collections::BTreeMap::from([(
                    "/api/v1/crates/demo/0.1.0".to_string(),
                    vec![(200, "{}".to_string())],
                )]),
                1,
            );

            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.rehearsal_registry = Some("rehearsal".to_string());
            opts.registries = vec![Registry {
                name: "rehearsal".to_string(),
                api_base: rehearsal_server.base_url.clone(),
                index_base: Some(rehearsal_server.base_url.clone()),
            }];
            opts.rehearsal_smoke_install = Some("nonexistent".to_string());

            let mut reporter = CollectingReporter::default();
            let outcome = run_rehearsal(&ws, &opts, &mut reporter).expect("rehearse");
            assert!(outcome.passed);
            assert!(
                reporter
                    .warns
                    .iter()
                    .any(|w| w.contains("not in the rehearsal plan")),
                "warns: {:?}",
                reporter.warns
            );
            rehearsal_server.join();
        });
    }

    #[test]
    #[serial]
    fn run_rehearsal_cargo_failure_emits_package_failed_and_marks_not_passed() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        write_fake_tools(&bin);
        let mut env_vars = fake_program_env_vars(&bin);
        env_vars.extend([("SHIPPER_CARGO_EXIT", Some("101".to_string()))]);
        temp_env::with_vars(env_vars, || {
            // Rehearsal registry is never hit because cargo fails.
            let rehearsal_server = spawn_registry_server(std::collections::BTreeMap::new(), 0);

            let ws = planned_workspace(td.path(), "http://127.0.0.1:1".into());
            let mut opts = default_opts(PathBuf::from(".shipper"));
            opts.rehearsal_registry = Some("rehearsal".to_string());
            opts.registries = vec![Registry {
                name: "rehearsal".to_string(),
                api_base: rehearsal_server.base_url.clone(),
                index_base: Some(rehearsal_server.base_url.clone()),
            }];

            let mut reporter = CollectingReporter::default();
            let outcome = run_rehearsal(&ws, &opts, &mut reporter).expect("rehearse");
            assert!(!outcome.passed);
            assert_eq!(outcome.packages_published, 0);

            let events = read_events_raw(&td.path().join(".shipper"));
            let types: Vec<String> = events.iter().filter_map(event_discriminator).collect();
            assert!(
                types.contains(&"rehearsal_package_failed".to_string()),
                "types: {types:?}"
            );
            assert!(
                types.contains(&"rehearsal_complete".to_string()),
                "types: {types:?}"
            );

            let complete = events
                .iter()
                .find(|e| event_discriminator(e).as_deref() == Some("rehearsal_complete"))
                .expect("RehearsalComplete");
            assert_eq!(complete["event_type"]["passed"].as_bool(), Some(false));
            rehearsal_server.join();
        });
    }
}

/// Wave-based parallel publishing engine.
pub mod parallel;

/// Plan-yank: reverse-topological containment plan from a receipt (#98 PR 2).
pub mod plan_yank;

/// Fix-forward: supersession plan from a compromised receipt (#98 PR 3).
pub mod fix_forward;

/// Remediation dry-run artifact planning.
pub mod remediation;
