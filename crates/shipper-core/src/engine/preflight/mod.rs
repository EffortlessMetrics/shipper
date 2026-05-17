//! Preflight pipeline: dry-run, registry probes, ownership, finishability.
//!
//! The public entry points (`engine::run_preflight*`) are thin wrappers that
//! delegate into [`run`]. Phase-specific logic lives in the sibling submodules
//! (`dry_run`, `package_check`, `duration`).

use std::path::Path;

use anyhow::{Result, bail};
use chrono::Utc;

use crate::engine::{Reporter, init_registry_client, policy_effects};
use crate::git;
use crate::ops::auth;
use crate::plan::PlannedWorkspace;
use crate::runtime::execution::resolve_state_dir;
use crate::state::events;
use crate::types::{
    AuthType, EventType, Finishability, PreflightReport, PublishEvent, RuntimeOptions,
};

pub(in crate::engine) mod dry_run;
pub(in crate::engine) mod duration;
pub(in crate::engine) mod package_check;

/// Run-time options that only affect preflight behavior (#100).
///
/// Kept separate from [`RuntimeOptions`] so that CLI-level "just this
/// invocation" toggles (e.g. `--preflight-only`) don't need to thread
/// through every other engine entry point. A `Default` instance
/// preserves historical behavior: reads and appends the authoritative
/// `events.jsonl` log.
#[derive(Debug, Clone, Copy, Default)]
pub struct PreflightRunOptions {
    /// If `true`, the preflight run is session-isolated (#100 /
    /// `shipper preflight --preflight-only`):
    ///
    /// - Does not touch the authoritative `events.jsonl` log; writes
    ///   its events to a sidecar at
    ///   `<state_dir>/preflight-only-<session>.events.jsonl`.
    /// - Does not load or inspect any prior `events.jsonl`; the
    ///   resulting `Finishability` is a fresh read of the current
    ///   workspace + registry, independent of any accumulated publish
    ///   or resume state.
    /// - Never writes `state.json`.
    ///
    /// `false` (the default) preserves the original behavior: the
    /// authoritative append-only `events.jsonl` is extended.
    pub fresh_audit: bool,
}

pub(in crate::engine) fn run(
    ws: &mut PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
    run_opts: PreflightRunOptions,
) -> Result<PreflightReport> {
    let workspace_root = &ws.workspace_root;
    let effects = policy_effects(opts);
    let state_dir = resolve_state_dir(workspace_root, &opts.state_dir);

    let events_path = resolve_events_path(&state_dir, run_opts);

    let mut event_log = events::EventLog::new();

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightStarted,
        package: "all".to_string(),
    });
    flush_events(&event_log, &events_path)?;
    event_log.clear();

    if !opts.allow_dirty {
        reporter.info("checking git cleanliness...");
        git::ensure_git_clean(workspace_root)?;
    }

    reporter.info("initializing registry client...");
    let reg = init_registry_client(ws.plan.registry.clone(), &state_dir)?;

    let token = auth::resolve_token(&ws.plan.registry.name)?;
    let token_detected = token.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
    let auth_type = auth::detect_auth_type_from_token(token.as_deref());
    warn_if_token_auth_overrides_oidc(&ws.plan.registry.name, &auth_type, reporter);

    if effects.strict_ownership && !token_detected {
        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightComplete {
                finishability: Finishability::Failed,
            },
            package: "all".to_string(),
        });
        flush_events(&event_log, &events_path)?;
        bail!(
            "strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)"
        );
    }

    let dry_run_outcome = dry_run::execute(ws, opts, &effects, &state_dir, reporter);

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightWorkspaceVerify {
            passed: dry_run_outcome.workspace_passed,
            output: dry_run_outcome.workspace_output.clone(),
        },
        package: "all".to_string(),
    });

    let check_outcome = package_check::check_packages(
        ws,
        opts,
        &effects,
        &reg,
        token.as_deref(),
        token_detected,
        &auth_type,
        &dry_run_outcome,
        &mut event_log,
        reporter,
    )?;

    let all_dry_run_passed = check_outcome.packages.iter().all(|p| p.dry_run_passed);
    let finishability = if !all_dry_run_passed {
        Finishability::Failed
    } else if check_outcome.any_ownership_unverified {
        Finishability::NotProven
    } else {
        Finishability::Proven
    };

    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PreflightComplete {
            finishability: finishability.clone(),
        },
        package: "all".to_string(),
    });
    flush_events(&event_log, &events_path)?;

    let estimated_publish_duration =
        duration::estimate_preflight_duration(&ws.plan.registry.name, &check_outcome.packages);

    Ok(PreflightReport {
        plan_id: ws.plan.plan_id.clone(),
        token_detected,
        finishability,
        packages: check_outcome.packages,
        timestamp: Utc::now(),
        estimated_publish_duration,
        dry_run_output: if opts.verify_mode == crate::types::VerifyMode::Workspace {
            Some(dry_run_outcome.workspace_output)
        } else {
            None
        },
    })
}

fn warn_if_token_auth_overrides_oidc(
    registry_name: &str,
    auth_type: &Option<AuthType>,
    reporter: &mut dyn Reporter,
) {
    let default_registry = matches!(
        registry_name,
        "" | auth::CRATES_IO_REGISTRY | "crates.io" | "crates_io"
    );
    if !default_registry || auth_type != &Some(AuthType::Token) {
        return;
    }

    let oidc_url_present = std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some();
    let oidc_token_present = std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_TOKEN").is_some();
    if oidc_url_present || oidc_token_present {
        reporter.warn(
            "Trusted Publishing OIDC environment is present, but Shipper is using Cargo token auth. \
             This is allowed as fallback; prefer the short-lived token minted by rust-lang/crates-io-auth-action@v1 for release runs.",
        );
    }
}

/// Resolve the event sink. In `fresh_audit` mode we never touch the
/// authoritative `events.jsonl`; events land in a session-scoped sidecar
/// instead. See [`PreflightRunOptions::fresh_audit`].
fn resolve_events_path(state_dir: &Path, run_opts: PreflightRunOptions) -> std::path::PathBuf {
    if run_opts.fresh_audit {
        let session_id = format!(
            "{}-pid{}",
            Utc::now().format("%Y%m%dT%H%M%S%fZ"),
            std::process::id()
        );
        events::preflight_only_events_path(state_dir, &session_id)
    } else {
        events::events_path(state_dir)
    }
}

fn flush_events(log: &events::EventLog, path: &Path) -> Result<()> {
    log.write_to_file(path)
}
