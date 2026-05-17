//! Registry reachability check.

use anyhow::Result;
use serde::Serialize;

use shipper_core::engine::Reporter;
use shipper_core::plan;

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
    reporter: &mut dyn Reporter,
) -> Result<Vec<Finding>> {
    reporter.info("checking registry connectivity...");
    let check = inspect(ws)?;
    if let Some(error) = &check.registry_error {
        reporter.warn(&format!("registry_reachable: false ({error})"));
    }
    println!("registry_reachable: {}", check.registry_reachable);
    println!("index_base: {}", check.index_base);
    Ok(check.findings)
}

pub(in crate::doctor) fn inspect(ws: &plan::PlannedWorkspace) -> Result<ConnectivityCheck> {
    let reg_client = shipper_core::registry::RegistryClient::new(ws.plan.registry.clone())?;

    let mut findings = Vec::new();
    let mut registry_error = None;
    let registry_reachable = match reg_client.crate_exists("serde") {
        Ok(_) => true,
        Err(e) => {
            let error = redact_diagnostic_value(&format!("{e:#}"));
            let evidence = format!("registry_reachable: false ({error})");
            registry_error = Some(error);
            findings.push(Finding {
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
            });
            false
        }
    };

    let index_base = redact_diagnostic_value(&ws.plan.registry.get_index_base());

    Ok(ConnectivityCheck {
        registry_reachable,
        registry_error,
        index_base,
        findings,
    })
}
