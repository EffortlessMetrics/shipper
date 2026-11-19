//! Registry reachability check.

use anyhow::Result;
use serde::Serialize;

use shipper_core::engine::Reporter;
use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};
use crate::doctor::redact_diagnostic_value;

#[derive(Debug, Serialize)]
pub(in crate::doctor) struct ConnectivityCheck {
    pub registry_reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_error: Option<String>,
    pub index_base: String,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<ConnectivityCheck> {
    reporter.info("checking registry connectivity...");
    let check = inspect(ws, opts)?;
    if let Some(error) = &check.registry_error {
        reporter.warn(&format!("registry_reachable: false ({error})"));
    }
    println!("registry_reachable: {}", check.registry_reachable);
    println!("index_base: {}", check.index_base);
    Ok(check)
}

pub(in crate::doctor) fn inspect(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<ConnectivityCheck> {
    let trust = opts.registry_policies.get(&ws.plan.registry.name);
    let policy = shipper_core::registry::RegistryPolicy::secure()
        .with_private(trust.is_some_and(|policy| policy.allow_private))
        .with_loopback(trust.is_some_and(|policy| policy.allow_loopback));
    let registry_error =
        match shipper_core::registry::RegistryClient::with_policy(ws.plan.registry.clone(), policy)
        {
            Ok(reg_client) => reg_client
                .crate_exists("serde")
                .err()
                .map(|error| redact_diagnostic_value(&format!("{error:#}"))),
            Err(error) => Some(redact_diagnostic_value(&format!("{error:#}"))),
        };
    let registry_reachable = registry_error.is_none();
    let findings = registry_error
        .as_ref()
        .map(|error| {
            let evidence = format!("registry_reachable: false ({error})");
            vec![Finding {
                id: "registry-unreachable",
                severity: FindingLevel::Blocked,
                status: FindingLevel::Blocked,
                title: "registry is unreachable",
                why_it_matters:
                    "preflight, publish readiness checks, and reconciliation need registry truth",
                evidence,
                try_next: vec![
                    "check network access to the configured registry",
                    "verify `--registry` and `--api-base` settings",
                    "rerun `shipper doctor` before publishing",
                ],
                docs: Some("docs/failure-modes.md"),
            }]
        })
        .unwrap_or_default();

    let index_base = redact_diagnostic_value(&ws.plan.registry.get_index_base());

    Ok(ConnectivityCheck {
        registry_reachable,
        registry_error,
        index_base,
        findings,
    })
}
