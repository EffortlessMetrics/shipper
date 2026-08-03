//! Registry authentication check.

use anyhow::Result;
use serde::Serialize;

use shipper_core::plan;
use shipper_core::types::AuthType;

use crate::doctor::findings::{Finding, FindingLevel};

#[derive(Debug, Serialize)]
pub(in crate::doctor) struct AuthCheck {
    pub auth_type: &'static str,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(ws: &plan::PlannedWorkspace) -> Result<Vec<Finding>> {
    let check = inspect(ws)?;
    println!("auth_type: {}", check.auth_type);
    Ok(check.findings)
}

pub(in crate::doctor) fn inspect(ws: &plan::PlannedWorkspace) -> Result<AuthCheck> {
    let auth_type = shipper_core::auth::detect_auth_type(&ws.plan.registry.name)?;
    let auth_label = match auth_type {
        Some(AuthType::Token) => "token (detected)",
        Some(AuthType::TrustedPublishing) => "trusted (detected)",
        Some(AuthType::Unknown) => "unknown",
        None => "NONE FOUND (set CARGO_REGISTRY_TOKEN)",
    };

    let mut findings = Vec::new();
    if auth_type.is_none() {
        findings.push(Finding {
            id: "registry-auth-missing",
            severity: FindingLevel::Blocked,
            status: FindingLevel::Blocked,
            title: "crates.io auth is missing",
            why_it_matters:
                "ownership checks and live publish require registry credentials before Shipper can prove or execute a release",
            evidence: trusted_publishing_evidence(auth_label, &ws.plan.registry.name),
            try_next: vec![
                "run `cargo login <token>` for local token auth",
                "configure Trusted Publishing with `permissions: id-token: write` and `rust-lang/crates-io-auth-action@v1`",
                "rerun `shipper doctor` and `shipper preflight`",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    } else if auth_type == Some(AuthType::TrustedPublishing) {
        findings.push(Finding {
            id: "trusted-publishing-token-not-minted",
            severity: FindingLevel::Blocked,
            status: FindingLevel::Blocked,
            title: "Trusted Publishing token exchange is incomplete",
            why_it_matters:
                "GitHub OIDC request variables are present, but Cargo still needs a short-lived registry token before Shipper can prove ownership or publish",
            evidence: trusted_publishing_evidence(
                "trusted (detected)",
                &ws.plan.registry.name,
            ),
            try_next: vec![
                "run `rust-lang/crates-io-auth-action@v1` before invoking Shipper",
                "pass `steps.auth.outputs.token` to Shipper as `CARGO_REGISTRY_TOKEN`",
                "rerun `shipper doctor` and `shipper preflight`",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    } else if auth_type == Some(AuthType::Unknown) {
        findings.push(Finding {
            id: "trusted-publishing-oidc-incomplete",
            severity: FindingLevel::Blocked,
            status: FindingLevel::Blocked,
            title: "Trusted Publishing OIDC environment is incomplete",
            why_it_matters:
                "Trusted Publishing requires both GitHub OIDC request variables; a partial environment cannot mint a crates.io token",
            evidence: trusted_publishing_evidence("unknown", &ws.plan.registry.name),
            try_next: vec![
                "set `permissions: id-token: write` on the release job",
                "run Shipper after the GitHub OIDC request URL and token are both available",
                "or configure an explicit Cargo token fallback before rerunning preflight",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    }
    findings.extend(trusted_publishing_workflow_findings(ws, auth_type));
    Ok(AuthCheck {
        auth_type: auth_label,
        findings,
    })
}

fn trusted_publishing_evidence(auth_label: &str, registry_name: &str) -> String {
    format!(
        "auth_type: {auth_label}; registry_token: {}; oidc_request_url: {}; oidc_request_token: {}",
        token_presence(registry_name),
        env_presence("ACTIONS_ID_TOKEN_REQUEST_URL"),
        env_presence("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
    )
}

fn token_presence(registry_name: &str) -> &'static str {
    let mut blank = false;
    if matches!(registry_name, "" | "crates-io")
        && let Some(value) = std::env::var_os("CARGO_REGISTRY_TOKEN")
    {
        if !value.to_string_lossy().trim().is_empty() {
            return "set";
        }
        blank = true;
    }

    let env_name = format!(
        "CARGO_REGISTRIES_{}_TOKEN",
        registry_name.to_ascii_uppercase().replace('-', "_")
    );
    match std::env::var_os(env_name) {
        Some(value) if !value.to_string_lossy().trim().is_empty() => "set",
        Some(_) => "blank",
        None if blank => "blank",
        None => "missing",
    }
}

fn env_presence(name: &str) -> &'static str {
    match std::env::var(name) {
        Err(_) => "missing",
        Ok(value) if value.trim().is_empty() => "blank",
        Ok(_) => "set",
    }
}

fn presence(is_set: bool) -> &'static str {
    if is_set { "set" } else { "missing" }
}

fn trusted_publishing_workflow_findings(
    ws: &plan::PlannedWorkspace,
    auth_type: Option<AuthType>,
) -> Vec<Finding> {
    let release_workflow = ws
        .workspace_root
        .join(".github")
        .join("workflows")
        .join("release.yml");
    let Ok(content) = std::fs::read_to_string(&release_workflow) else {
        return Vec::new();
    };

    let lower = content.to_ascii_lowercase();
    let mentions_trusted_publishing =
        lower.contains("crates-io-auth-action") || lower.contains("trusted publishing");
    if !mentions_trusted_publishing {
        return Vec::new();
    }

    let id_token_write = lower.contains("id-token: write");
    let auth_action = lower.contains("rust-lang/crates-io-auth-action@v1");
    let release_environment = lower.contains("environment: release");
    let token_fallback = lower.contains("secrets.cargo_registry_token");

    let missing = [
        (!id_token_write).then_some("id-token: write"),
        (!auth_action).then_some("rust-lang/crates-io-auth-action@v1"),
        (!release_environment).then_some("environment: release"),
        (!token_fallback).then_some("secrets.CARGO_REGISTRY_TOKEN fallback"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let mut findings = Vec::new();
    if !missing.is_empty() {
        findings.push(Finding {
            id: "trusted-publishing-workflow-prerequisites",
            severity: FindingLevel::Warning,
            status: FindingLevel::Warning,
            title: "Trusted Publishing workflow prerequisites need review",
            why_it_matters: "Trusted Publishing depends on GitHub OIDC permission, the crates.io auth action, release-environment scope, and an explicit token fallback for incident recovery",
            evidence: format!(
                "release_workflow: {}; id_token_write: {}; crates_io_auth_action: {}; release_environment: {}; token_fallback: {}; missing: {}",
                release_workflow.display(),
                presence(id_token_write),
                presence(auth_action),
                presence(release_environment),
                presence(token_fallback),
                missing.join(", ")
            ),
            try_next: vec![
                "add `permissions: id-token: write` to the release workflow",
                "run `rust-lang/crates-io-auth-action@v1` before publish/preflight",
                "bind publish/rehearsal jobs to the crates.io Trusted Publishing environment",
                "keep `secrets.CARGO_REGISTRY_TOKEN` as an explicit fallback while rollout is advisory",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    }

    if token_fallback && auth_type == Some(AuthType::Token) {
        findings.push(Finding {
            id: "trusted-publishing-token-fallback-configured",
            severity: FindingLevel::Warning,
            status: FindingLevel::Warning,
            title: "Long-lived Cargo token fallback is configured",
            why_it_matters: "Cargo receives both a minted Trusted Publishing token and a fallback secret through the same token interface, so operators need an explicit reminder that a long-lived token path still exists",
            evidence: format!(
                "release_workflow: {}; auth_type: token (detected); token_fallback: set; token_value: redacted",
                release_workflow.display()
            ),
            try_next: vec![
                "prefer the token minted by `rust-lang/crates-io-auth-action@v1`",
                "treat `secrets.CARGO_REGISTRY_TOKEN` as incident fallback only",
                "remove the fallback after Trusted Publishing registration and release rehearsal are proven",
            ],
            docs: Some("docs/how-to/run-in-github-actions.md"),
        });
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn trusted_publishing_evidence_distinguishes_missing_blank_and_set() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", Some("")),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
            ],
            || {
                assert_eq!(
                    trusted_publishing_evidence("unknown", "crates-io"),
                    "auth_type: unknown; registry_token: missing; oidc_request_url: blank; oidc_request_token: set"
                );
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_evidence_reports_missing_oidc_values() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                assert_eq!(
                    trusted_publishing_evidence("unknown", "crates-io"),
                    "auth_type: unknown; registry_token: missing; oidc_request_url: missing; oidc_request_token: missing"
                );
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_evidence_ignores_unrelated_registry_token() {
        let unrelated_token = ["private", "token"].concat();
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                (
                    "CARGO_REGISTRIES_PRIVATE_TOKEN",
                    Some(unrelated_token.as_str()),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                assert_eq!(
                    trusted_publishing_evidence("unknown", "crates-io"),
                    "auth_type: unknown; registry_token: missing; oidc_request_url: missing; oidc_request_token: missing"
                );
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_evidence_preserves_token_resolution_order() {
        let registry_token = ["crates", "io", "token"].concat();
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("   ")),
                (
                    "CARGO_REGISTRIES_CRATES_IO_TOKEN",
                    Some(registry_token.as_str()),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                assert_eq!(
                    trusted_publishing_evidence("unknown", "crates-io"),
                    "auth_type: unknown; registry_token: set; oidc_request_url: missing; oidc_request_token: missing"
                );
            },
        );
    }
}
