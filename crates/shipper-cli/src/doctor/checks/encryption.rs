//! State-encryption configuration check.

use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};

pub(in crate::doctor) fn check(opts: &RuntimeOptions) -> Vec<Finding> {
    let mut findings = Vec::new();
    if !opts.encryption.enabled {
        return findings;
    }

    println!();
    println!("encryption: enabled");
    if opts.encryption.passphrase.is_some() {
        println!("encryption_key_source: config");
    } else if let Some(ref env_var) = opts.encryption.env_var {
        let present = std::env::var(env_var).is_ok();
        println!("encryption_key_source: env ({})", env_var);
        println!("encryption_key_present: {}", present);
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
    }
    findings
}
