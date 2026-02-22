//! Environment fingerprinting for shipper, backed by `shipper-environment`.
//!
//! The shim keeps `shipper::environment` API-compatible while sharing a
//! centralized implementation from the microcrate.
//! External callers can continue using `collect_environment_fingerprint` and
//! other `shipper::environment` entry points with stable behavior.

use std::env;

use crate::types::EnvironmentFingerprint;

pub use shipper_environment::{
    CiEnvironment, EnvironmentInfo, detect_environment, get_cargo_version, get_ci_branch,
    get_ci_commit_sha, get_environment_fingerprint, get_rust_version, is_ci, is_pull_request,
};

/// Convert command output like `rustc 1.92.0` into `Some("1.92.0")`.
fn normalize_version(raw: &str) -> Option<String> {
    let normalized = raw.split_whitespace().collect::<Vec<_>>();
    if normalized.len() >= 2 {
        Some(normalized[1].to_string())
    } else {
        None
    }
}

/// Collect environment fingerprint details while preserving the existing in-crate
/// API shape.
pub fn collect_environment_fingerprint() -> EnvironmentFingerprint {
    let environment_info = EnvironmentInfo::collect().unwrap_or_else(|_| EnvironmentInfo {
        ci_environment: detect_environment(),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        rust_version: "unknown".to_string(),
        cargo_version: "unknown".to_string(),
        env_vars: std::collections::BTreeMap::new(),
        collected_at: chrono::Utc::now(),
    });

    EnvironmentFingerprint {
        shipper_version: env!("CARGO_PKG_VERSION").to_string(),
        cargo_version: normalize_version(&environment_info.cargo_version),
        rust_version: normalize_version(&environment_info.rust_version),
        os: environment_info.os,
        arch: environment_info.arch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_environment_fingerprint_has_expected_shape() {
        let fingerprint = collect_environment_fingerprint();
        assert!(!fingerprint.shipper_version.is_empty());
        assert!(!fingerprint.os.is_empty());
        assert!(!fingerprint.arch.is_empty());
    }

    #[test]
    fn normalize_version_extracts_numeric_suffix() {
        assert_eq!(
            normalize_version("cargo 1.75.0"),
            Some("1.75.0".to_string())
        );
        assert_eq!(
            normalize_version("rustc 1.72.1"),
            Some("1.72.1".to_string())
        );
        assert_eq!(normalize_version("bad-version"), None);
    }

    #[test]
    fn detect_environment_is_stable() {
        let env = detect_environment();
        assert!(!env.to_string().is_empty());
    }
}
