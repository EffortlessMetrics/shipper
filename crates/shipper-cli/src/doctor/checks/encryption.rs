//! State-encryption configuration check.

use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};

#[derive(Debug, serde::Serialize)]
pub(in crate::doctor) struct EncryptionCheck {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_present: Option<bool>,
    pub findings: Vec<Finding>,
}

pub(in crate::doctor) fn check(opts: &RuntimeOptions) -> Vec<Finding> {
    let check = inspect(opts);
    if check.enabled {
        println!();
        println!("encryption: enabled");
        if let Some(key_source) = &check.key_source {
            match key_source.as_str() {
                "config" => println!("encryption_key_source: config"),
                "env" => {
                    if let Some(env_var) = &check.env_var {
                        println!("encryption_key_source: env ({})", env_var);
                    }
                    if let Some(present) = check.key_present {
                        println!("encryption_key_present: {}", present);
                    }
                }
                _ => {}
            }
        }
    }
    check.findings
}

pub(in crate::doctor) fn inspect(opts: &RuntimeOptions) -> EncryptionCheck {
    let mut findings = Vec::new();
    if !opts.encryption.enabled {
        return EncryptionCheck {
            enabled: false,
            key_source: None,
            env_var: None,
            key_present: None,
            findings,
        };
    }

    if opts.encryption.passphrase.is_some() {
        return EncryptionCheck {
            enabled: true,
            key_source: Some("config".to_string()),
            env_var: None,
            key_present: Some(true),
            findings,
        };
    } else if let Some(ref env_var) = opts.encryption.env_var {
        let present = std::env::var(env_var).is_ok();
        if !present {
            findings.push(Finding {
                id: "encryption-key-missing",
                severity: FindingLevel::Blocked,
                status: FindingLevel::Blocked,
                title: "state encryption key is missing",
                why_it_matters:
                    "encrypted state cannot be written or resumed unless the configured key source is present",
                evidence: format!("encryption_key_present: false ({env_var})"),
                try_next: vec![
                    "set the configured encryption environment variable",
                    "or disable state encryption for this run",
                    "rerun `shipper doctor`",
                ],
                docs: Some("docs/configuration.md"),
            });
        }
        return EncryptionCheck {
            enabled: true,
            key_source: Some("env".to_string()),
            env_var: Some(env_var.clone()),
            key_present: Some(present),
            findings,
        };
    }
    EncryptionCheck {
        enabled: true,
        key_source: None,
        env_var: None,
        key_present: None,
        findings,
    }
}
