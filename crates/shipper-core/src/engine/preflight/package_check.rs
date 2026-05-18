//! Per-package preflight: registry probes, regime stamping, ownership verification.

use anyhow::Result;
use chrono::Utc;

use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::runtime::policy::PolicyEffects;
use crate::state::events;
use crate::types::{
    AuthType, EventType, PreflightPackage, PublishEvent, PublishRegime, RuntimeOptions, VerifyMode,
};

use super::dry_run::DryRunOutcome;

pub(in crate::engine) struct PackageCheckOutcome {
    pub packages: Vec<PreflightPackage>,
    pub any_ownership_unverified: bool,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) fn check_packages(
    ws: &mut PlannedWorkspace,
    opts: &RuntimeOptions,
    effects: &PolicyEffects,
    reg: &RegistryClient,
    token: Option<&str>,
    token_detected: bool,
    auth_type: &Option<AuthType>,
    dry_run: &DryRunOutcome,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
) -> Result<PackageCheckOutcome> {
    reporter.info("checking packages against registry...");
    let mut packages: Vec<PreflightPackage> = Vec::new();
    let mut any_ownership_unverified = false;

    for p in ws.plan.packages.iter_mut() {
        let already_published = reg.version_exists(&p.name, &p.version)?;
        let is_new_crate = reg.check_new_crate(&p.name)?;

        // #106 PR 1: stamp the detected regime onto the plan so the
        // publish retry loop can consume it without re-querying the
        // registry. Classification is based purely on registry
        // presence here; finer-grained variants (e.g. Patch) can be
        // layered on in later PRs without breaking this contract.
        p.regime = Some(if is_new_crate {
            PublishRegime::FirstPublish
        } else {
            PublishRegime::Update
        });

        if is_new_crate {
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PreflightNewCrateDetected {
                    crate_name: p.name.clone(),
                },
                package: format!("{}@{}", p.name, p.version),
            });
        }

        let (dry_run_passed, dry_run_output) = if opts.verify_mode == VerifyMode::Package {
            dry_run
                .per_package
                .get(&p.name)
                .cloned()
                .unwrap_or((true, None))
        } else {
            (
                dry_run.workspace_passed,
                Some(dry_run.workspace_output.clone()),
            )
        };

        let ownership_verified = verify_ownership(
            p.name.as_str(),
            is_new_crate,
            effects,
            reg,
            token,
            token_detected,
            reporter,
        )?;

        event_log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightOwnershipCheck {
                crate_name: p.name.clone(),
                verified: ownership_verified,
            },
            package: format!("{}@{}", p.name, p.version),
        });

        if !ownership_verified {
            any_ownership_unverified = true;
        }

        packages.push(PreflightPackage {
            name: p.name.clone(),
            version: p.version.clone(),
            already_published,
            is_new_crate,
            auth_type: auth_type.clone(),
            ownership_verified,
            dry_run_passed,
            dry_run_output,
        });
    }

    Ok(PackageCheckOutcome {
        packages,
        any_ownership_unverified,
    })
}

#[allow(clippy::too_many_arguments)]
fn verify_ownership(
    name: &str,
    is_new_crate: bool,
    effects: &PolicyEffects,
    reg: &RegistryClient,
    token: Option<&str>,
    token_detected: bool,
    reporter: &mut dyn Reporter,
) -> Result<bool> {
    if !(token_detected && effects.check_ownership) {
        return Ok(false);
    }
    let Some(token) = token else {
        return Ok(false);
    };

    if effects.strict_ownership {
        if is_new_crate {
            // New crates have no owners endpoint; skip ownership check
            reporter.info(&format!("{name}: new crate, skipping ownership check"));
            Ok(false)
        } else {
            // In strict mode, ownership errors are fatal
            reg.list_owners(name, token)?;
            Ok(true)
        }
    } else {
        let result = reg.verify_ownership(name, token).unwrap_or_default();
        if !result {
            reporter.warn(&format!(
                "owners preflight failed for {name}; continuing (non-strict mode)"
            ));
        }
        Ok(result)
    }
}
