//! Registry reachability check.

use anyhow::Result;

use shipper_core::engine::Reporter;
use shipper_core::plan;

use crate::doctor::findings::{Finding, FindingLevel};

pub(in crate::doctor) fn check(
    ws: &plan::PlannedWorkspace,
    reporter: &mut dyn Reporter,
) -> Result<Vec<Finding>> {
    reporter.info("checking registry connectivity...");
    let reg_client = shipper_core::registry::RegistryClient::new(ws.plan.registry.clone())?;

    let mut findings = Vec::new();
    match reg_client.crate_exists("serde") {
        Ok(_) => println!("registry_reachable: true"),
        Err(e) => {
            let evidence = format!("registry_reachable: false ({e:#})");
            reporter.warn(&evidence);
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
        }
    }

    let index_base = ws.plan.registry.get_index_base();
    println!("index_base: {}", index_base);

    Ok(findings)
}
