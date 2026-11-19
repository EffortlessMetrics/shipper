//! Registry authentication check.

use anyhow::Result;
use serde::Serialize;

use shipper_core::plan;
use shipper_core::registry::{RegistryPolicy, ValidatedRegistry};
use shipper_core::types::{AuthType, Registry, RuntimeOptions};

use crate::doctor::findings::{Finding, FindingLevel};

#[derive(Debug, Serialize)]
pub(in crate::doctor) struct AuthCheck {
    pub auth_type: &'static str,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<AuthCheck> {
    let check = inspect(ws, opts)?;
    println!("auth_type: {}", check.auth_type);
    Ok(check)
}

pub(in crate::doctor) fn inspect(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> Result<AuthCheck> {
    let auth_type = shipper_core::auth::detect_auth_type(&ws.plan.registry.name)?;
    let auth_label = match auth_type {
        Some(AuthType::Token) => "token (detected)",
        Some(AuthType::TrustedPublishing) if ws.plan.registry.name == "crates-io" => {
            "trusted (detected)"
        }
        Some(AuthType::TrustedPublishing) => {
            "selected-registry auth unproven (OIDC environment detected)"
        }
        Some(AuthType::Unknown) if ws.plan.registry.name == "crates-io" => "unknown",
        Some(AuthType::Unknown) => "selected-registry auth unproven (incomplete environment)",
        None if ws.plan.registry.name == "crates-io" => "NONE FOUND (set CARGO_REGISTRY_TOKEN)",
        None => "NONE FOUND (selected-registry token missing)",
    };

    let mut findings = Vec::new();
    if auth_type.is_none() {
        findings.push(missing_auth_finding(ws, opts, auth_label));
    } else if ws.plan.registry.name != "crates-io"
        && matches!(
            auth_type,
            Some(AuthType::TrustedPublishing | AuthType::Unknown)
        )
    {
        findings.push(non_crates_oidc_finding(ws, auth_type.as_ref(), auth_label));
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
    if ws.plan.registry.name == "crates-io" {
        findings.extend(trusted_publishing_workflow_findings(ws, auth_type));
    }
    Ok(AuthCheck {
        auth_type: auth_label,
        findings,
    })
}

fn non_crates_oidc_finding(
    ws: &plan::PlannedWorkspace,
    auth_type: Option<&AuthType>,
    auth_label: &str,
) -> Finding {
    let trusted = auth_type == Some(&AuthType::TrustedPublishing);
    let (id, title, why_it_matters) = if trusted {
        (
            "selected-registry-auth-not-proven",
            "selected registry auth is not proven",
            "OIDC request variables do not establish that the selected registry accepts that identity or that Cargo has a token for it",
        )
    } else {
        (
            "selected-registry-auth-environment-incomplete",
            "selected registry auth environment is incomplete",
            "partial OIDC request variables do not establish any usable authentication method for the selected registry",
        )
    };
    Finding {
        id,
        severity: FindingLevel::Blocked,
        status: FindingLevel::Blocked,
        title,
        why_it_matters,
        evidence: trusted_publishing_evidence(auth_label, &ws.plan.registry.name),
        try_next: vec![
            "confirm the selected registry's supported authentication method with its operator",
            "configure the selected registry token through Cargo's registry-specific token interface",
            "rerun `shipper doctor` and `shipper preflight` for the same selected registry",
        ],
        docs: Some("docs/how-to/rehearse-against-an-alt-registry.md"),
    }
}

fn missing_auth_finding(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    auth_label: &str,
) -> Finding {
    let registry = &ws.plan.registry;
    let selected_policy = opts.registry_policies.get(&registry.name);
    let allow_loopback = selected_policy.is_some_and(|policy| policy.allow_loopback);
    let loopback_endpoint = registry_uses_only_loopback_endpoints(registry);
    let explicit_loopback_rehearsal = explicit_loopback_rehearsal(registry, allow_loopback);
    let base_evidence = trusted_publishing_evidence(auth_label, &registry.name);
    let posture_evidence = format!(
        "{}; selected_registry_allow_loopback: {}; loopback_endpoint: {}; live_auth_proven: false",
        base_evidence, allow_loopback, loopback_endpoint,
    );

    if explicit_loopback_rehearsal {
        Finding {
            id: "registry-auth-not-proven",
            severity: FindingLevel::Warning,
            status: FindingLevel::Warning,
            title: "registry auth is not proven for this loopback rehearsal",
            why_it_matters: "the selected registry explicitly permits loopback rehearsal traffic, but that trust choice does not prove anonymous access, live credentials, ownership, or publish authorization",
            evidence: posture_evidence,
            try_next: vec![
                "continue only with the isolated loopback registry and an intercepted or fake Cargo process",
                "treat this warning as rehearsal-only evidence, not proof that live registry auth is ready",
                "run `shipper doctor` separately against the intended live registry before any live preflight or publish",
            ],
            docs: Some("docs/how-to/rehearse-against-an-alt-registry.md"),
        }
    } else {
        let crates_io = registry.name == "crates-io";
        Finding {
            id: "registry-auth-missing",
            severity: FindingLevel::Blocked,
            status: FindingLevel::Blocked,
            title: if crates_io {
                "crates.io auth is missing"
            } else {
                "selected registry auth is missing"
            },
            why_it_matters: "ownership checks and live publish require registry credentials before Shipper can prove or execute a release",
            evidence: if crates_io {
                base_evidence
            } else {
                posture_evidence
            },
            try_next: if crates_io {
                vec![
                    "run `cargo login <token>` for local token auth",
                    "configure Trusted Publishing with `permissions: id-token: write` and `rust-lang/crates-io-auth-action@v1`",
                    "rerun `shipper doctor` and `shipper preflight`",
                ]
            } else {
                vec![
                    "configure a Cargo token for the selected registry",
                    "confirm the selected registry's authentication and ownership requirements",
                    "rerun `shipper doctor` and `shipper preflight`",
                ]
            },
            docs: Some(if crates_io {
                "docs/how-to/run-in-github-actions.md"
            } else {
                "docs/how-to/rehearse-against-an-alt-registry.md"
            }),
        }
    }
}

fn explicit_loopback_rehearsal(registry: &Registry, allow_loopback: bool) -> bool {
    registry.name != "crates-io"
        && allow_loopback
        && registry_uses_only_loopback_endpoints(registry)
}

fn registry_uses_only_loopback_endpoints(registry: &Registry) -> bool {
    ValidatedRegistry::new(registry.clone(), RegistryPolicy::secure()).is_err()
        && ValidatedRegistry::new(
            registry.clone(),
            RegistryPolicy::secure().with_loopback(true),
        )
        .is_ok()
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
    fn loopback_posture_requires_every_endpoint_to_be_loopback() {
        let loopback = Registry {
            name: "local".to_string(),
            api_base: "http://127.0.0.1:8080".to_string(),
            index_base: Some("http://127.0.0.1:8080/index".to_string()),
        };
        assert!(registry_uses_only_loopback_endpoints(&loopback));

        let live = Registry {
            name: "live".to_string(),
            api_base: "https://registry.example.test".to_string(),
            index_base: Some("https://index.example.test".to_string()),
        };
        assert!(!registry_uses_only_loopback_endpoints(&live));

        let mixed = Registry {
            name: "mixed".to_string(),
            api_base: "http://127.0.0.1:8080".to_string(),
            index_base: Some("https://index.example.test".to_string()),
        };
        assert!(!registry_uses_only_loopback_endpoints(&mixed));
    }

    #[test]
    fn loopback_endpoint_or_name_never_implies_rehearsal_auth_posture() {
        let mut registry = Registry {
            name: "local".to_string(),
            api_base: "http://127.0.0.1:8080".to_string(),
            index_base: Some("http://127.0.0.1:8080/index".to_string()),
        };
        assert!(!explicit_loopback_rehearsal(&registry, false));

        registry.name = "crates-io".to_string();
        assert!(!explicit_loopback_rehearsal(&registry, true));

        registry.name = "local".to_string();
        registry.api_base = "https://registry.example.test".to_string();
        registry.index_base = Some("https://index.example.test".to_string());
        assert!(!explicit_loopback_rehearsal(&registry, true));
    }

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
