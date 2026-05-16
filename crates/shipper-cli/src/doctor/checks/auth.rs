//! Registry authentication check.

use anyhow::Result;

use shipper_core::plan;
use shipper_core::types::AuthType;

use crate::doctor::findings::{Finding, FindingLevel};

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace) -> Result<Vec<Finding>> {
    let auth_type = shipper_core::auth::detect_auth_type(&ws.plan.registry.name)?;
    let auth_label = match auth_type {
        Some(AuthType::Token) => "token (detected)",
        Some(AuthType::TrustedPublishing) => "trusted (detected)",
        Some(AuthType::Unknown) => "unknown",
        None => "NONE FOUND (set CARGO_REGISTRY_TOKEN)",
    };
    println!("auth_type: {}", auth_label);

    let mut findings = Vec::new();
    if auth_type.is_none() {
        findings.push(Finding {
            id: "registry-auth-missing",
            severity: FindingLevel::Blocked,
            status: FindingLevel::Blocked,
            title: "crates.io auth is missing",
            why_it_matters:
                "ownership checks and live publish require registry credentials before Shipper can prove or execute a release",
            evidence: "auth_type: NONE FOUND (set CARGO_REGISTRY_TOKEN)".to_string(),
            try_next: vec![
                "run `cargo login <token>` for local token auth",
                "configure Trusted Publishing for GitHub Actions releases",
                "rerun `shipper doctor` and `shipper preflight`",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    }
    Ok(findings)
}
