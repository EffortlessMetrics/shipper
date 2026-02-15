use std::process::Command;

use crate::types::EnvironmentFingerprint;

/// Collect environment fingerprint information
pub fn collect_environment_fingerprint() -> EnvironmentFingerprint {
    // Get shipper version from CARGO_PKG_VERSION
    let shipper_version = env!("CARGO_PKG_VERSION").to_string();

    // Get cargo version by running `cargo --version`
    let cargo_version = get_cargo_version();

    // Get rust version by running `rustc --version`
    let rust_version = get_rust_version();

    // Get OS and architecture from std::env::consts
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    EnvironmentFingerprint {
        shipper_version,
        cargo_version,
        rust_version,
        os,
        arch,
    }
}

/// Get cargo version by running `cargo --version`
fn get_cargo_version() -> Option<String> {
    let output = Command::new("cargo").arg("--version").output().ok()?;

    if output.status.success() {
        let version_string = String::from_utf8_lossy(&output.stdout);
        // Parse the version string, e.g., "cargo 1.75.0 (1d8b05cdd 2023-11-20)"
        // We want just the version part like "1.75.0"
        version_string
            .split_whitespace()
            .nth(1)
            .map(|s| s.to_string())
    } else {
        None
    }
}

/// Get rust version by running `rustc --version`
fn get_rust_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;

    if output.status.success() {
        let version_string = String::from_utf8_lossy(&output.stdout);
        // Parse the version string, e.g., "rustc 1.75.0 (82e1608df 2023-12-21)"
        // We want just the version part like "1.75.0"
        version_string
            .split_whitespace()
            .nth(1)
            .map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_environment_fingerprint() {
        let fingerprint = collect_environment_fingerprint();

        // Check that the shipper version is set
        assert!(!fingerprint.shipper_version.is_empty());

        // Check that OS and arch are set
        assert!(!fingerprint.os.is_empty());
        assert!(!fingerprint.arch.is_empty());

        // cargo_version and rust_version are optional, so we just check
        // that they don't panic when accessed
        let _ = fingerprint.cargo_version.as_ref();
        let _ = fingerprint.rust_version.as_ref();
    }

    #[test]
    fn test_get_cargo_version() {
        // This test assumes cargo is available in the environment
        let version = get_cargo_version();
        // We can't assert on the exact version, but we can check it's a valid format if present
        if let Some(v) = version {
            assert!(!v.is_empty());
        }
    }

    #[test]
    fn test_get_rust_version() {
        // This test assumes rustc is available in the environment
        let version = get_rust_version();
        // We can't assert on the exact version, but we can check it's a valid format if present
        if let Some(v) = version {
            assert!(!v.is_empty());
        }
    }

    #[test]
    fn environment_fingerprint_serializes_correctly() {
        let fingerprint = EnvironmentFingerprint {
            shipper_version: "0.2.0".to_string(),
            cargo_version: Some("1.75.0".to_string()),
            rust_version: Some("1.75.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        let json = serde_json::to_string(&fingerprint).expect("serialize");
        let parsed: EnvironmentFingerprint = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.shipper_version, fingerprint.shipper_version);
        assert_eq!(parsed.cargo_version, fingerprint.cargo_version);
        assert_eq!(parsed.rust_version, fingerprint.rust_version);
        assert_eq!(parsed.os, fingerprint.os);
        assert_eq!(parsed.arch, fingerprint.arch);
    }

    #[test]
    fn environment_fingerprint_handles_missing_optional_versions() {
        let fingerprint = EnvironmentFingerprint {
            shipper_version: "0.2.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        let json = serde_json::to_string(&fingerprint).expect("serialize");
        let parsed: EnvironmentFingerprint = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.shipper_version, fingerprint.shipper_version);
        assert_eq!(parsed.cargo_version, None);
        assert_eq!(parsed.rust_version, None);
        assert_eq!(parsed.os, fingerprint.os);
        assert_eq!(parsed.arch, fingerprint.arch);
    }

    #[test]
    fn environment_fingerprint_has_valid_os() {
        let fingerprint = collect_environment_fingerprint();

        // OS should be a valid identifier
        assert!(!fingerprint.os.is_empty());
        assert!(fingerprint.os.len() <= 16); // Typical max length for OS identifiers
    }

    #[test]
    fn environment_fingerprint_has_valid_arch() {
        let fingerprint = collect_environment_fingerprint();

        // Arch should be a valid identifier
        assert!(!fingerprint.arch.is_empty());
        assert!(fingerprint.arch.len() <= 16); // Typical max length for arch identifiers
    }

    #[test]
    fn environment_fingerprint_shipper_version_is_set() {
        let fingerprint = collect_environment_fingerprint();

        // Shipper version should always be set from CARGO_PKG_VERSION
        assert!(!fingerprint.shipper_version.is_empty());
        // Should be a valid semver format (major.minor.patch)
        assert!(
            fingerprint
                .shipper_version
                .chars()
                .all(|c| c.is_numeric() || c == '.')
        );
    }

    #[test]
    fn environment_fingerprint_roundtrip_preserves_all_fields() {
        let original = collect_environment_fingerprint();

        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: EnvironmentFingerprint = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.shipper_version, original.shipper_version);
        assert_eq!(parsed.cargo_version, original.cargo_version);
        assert_eq!(parsed.rust_version, original.rust_version);
        assert_eq!(parsed.os, original.os);
        assert_eq!(parsed.arch, original.arch);
    }
}
