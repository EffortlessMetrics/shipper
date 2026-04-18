//! Environment information and fingerprint helpers.
//!
//! `EnvironmentInfo` + `collect`, tool-version capture, and the short
//! pipe-separated `get_environment_fingerprint` form.

use std::collections::BTreeMap;
use std::env;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{CiEnvironment, detect_environment};

/// Environment information collected for fingerprinting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EnvironmentInfo {
    /// Detected CI environment
    pub(crate) ci_environment: CiEnvironment,
    /// Operating system
    pub(crate) os: String,
    /// Architecture
    pub(crate) arch: String,
    /// Rust version
    pub(crate) rust_version: String,
    /// Cargo version
    pub(crate) cargo_version: String,
    /// Collected environment variables (sanitized)
    pub(crate) env_vars: BTreeMap<String, String>,
    /// Timestamp of collection
    pub(crate) collected_at: DateTime<Utc>,
}

impl EnvironmentInfo {
    /// Collect current environment information.
    pub(crate) fn collect() -> Result<Self> {
        let ci_environment = detect_environment();
        let os = env::consts::OS.to_string();
        let arch = env::consts::ARCH.to_string();

        let rust_version = get_rust_version().unwrap_or_else(|_| "unknown".to_string());
        let cargo_version = get_cargo_version().unwrap_or_else(|_| "unknown".to_string());
        let env_vars = collect_env_vars();

        Ok(Self {
            ci_environment,
            os,
            arch,
            rust_version,
            cargo_version,
            env_vars,
            collected_at: Utc::now(),
        })
    }

    /// Generate a pipe-separated fingerprint string.
    pub(crate) fn fingerprint(&self) -> String {
        let mut components = Vec::new();
        components.push(format!("ci:{}", self.ci_environment));
        components.push(format!("os:{}", self.os));
        components.push(format!("arch:{}", self.arch));
        components.push(format!("rust:{}", self.rust_version));
        components.push(format!("cargo:{}", self.cargo_version));

        for (key, value) in &self.env_vars {
            components.push(format!("{}:{}", key, value));
        }

        components.join("|")
    }
}

/// Get a quick environment fingerprint (pipe-separated string form).
pub(crate) fn get_environment_fingerprint() -> String {
    let ci = detect_environment();
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let rust = get_rust_version().unwrap_or_else(|_| "unknown".to_string());

    format!("{}|{}|{}|{}", ci, os, arch, rust)
}

/// Extract the second whitespace-separated token from tool-version output.
///
/// Used to turn `"rustc 1.92.0 (...)"` into `"1.92.0"`. Returns `None` if the
/// input has fewer than two tokens.
pub(super) fn normalize_tool_version(raw: &str) -> Option<String> {
    raw.split_whitespace().nth(1).map(ToOwned::to_owned)
}

/// Run `rustc --version` and return its trimmed stdout.
pub(crate) fn get_rust_version() -> Result<String> {
    let output = std::process::Command::new("rustc")
        .args(["--version"])
        .output()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        Err(anyhow::anyhow!("failed to get rust version"))
    }
}

/// Run `cargo --version` and return its trimmed stdout.
pub(crate) fn get_cargo_version() -> Result<String> {
    let output = std::process::Command::new("cargo")
        .args(["--version"])
        .output()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        Err(anyhow::anyhow!("failed to get cargo version"))
    }
}

/// Collect an allowlist of CI-related environment variables.
pub(super) fn collect_env_vars() -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();

    let ci_vars = [
        "CI",
        "GITHUB_REF",
        "GITHUB_SHA",
        "GITHUB_REPOSITORY",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_NUMBER",
        "GITLAB_CI_PIPELINE_ID",
        "CIRCLE_BUILD_NUM",
        "CIRCLE_BRANCH",
        "TRAVIS_BUILD_NUMBER",
        "TRAVIS_BRANCH",
        "BUILD_BUILDID",
        "BUILD_NUMBER",
        "BITBUCKET_BRANCH",
        "BITBUCKET_COMMIT",
    ];

    for var in ci_vars {
        if let Ok(value) = env::var(var) {
            vars.insert(var.to_string(), value);
        }
    }

    vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ── normalize_tool_version ──

    #[test]
    fn normalize_tool_version_typical_rustc() {
        assert_eq!(
            normalize_tool_version("rustc 1.75.0 (82e1608df 2023-12-21)"),
            Some("1.75.0".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_typical_cargo() {
        assert_eq!(
            normalize_tool_version("cargo 1.75.0 (1d8b05cdd 2023-11-20)"),
            Some("1.75.0".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_single_word() {
        assert_eq!(normalize_tool_version("rustc"), None);
    }

    #[test]
    fn normalize_tool_version_empty_string() {
        assert_eq!(normalize_tool_version(""), None);
    }

    #[test]
    fn normalize_tool_version_whitespace_only() {
        assert_eq!(normalize_tool_version("   "), None);
    }

    #[test]
    fn normalize_tool_version_two_tokens() {
        assert_eq!(
            normalize_tool_version("rustc 1.80.0"),
            Some("1.80.0".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_nightly() {
        assert_eq!(
            normalize_tool_version("rustc 1.83.0-nightly (abc123 2024-01-01)"),
            Some("1.83.0-nightly".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_beta() {
        assert_eq!(
            normalize_tool_version("rustc 1.82.0-beta.3 (def456 2024-02-01)"),
            Some("1.82.0-beta.3".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_prerelease_with_hyphen() {
        assert_eq!(
            normalize_tool_version("shipper 0.3.0-rc.1"),
            Some("0.3.0-rc.1".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_many_tokens() {
        assert_eq!(
            normalize_tool_version("rustc 1.80.0 (abc 2024-01-01) extra stuff"),
            Some("1.80.0".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_tabs_and_extra_spaces() {
        assert_eq!(
            normalize_tool_version("cargo  \t  1.80.0   (hash)"),
            Some("1.80.0".to_string())
        );
    }

    #[test]
    fn normalize_tool_version_nightly_alt() {
        assert_eq!(
            normalize_tool_version("rustc 1.82.0-nightly (abc 2024-01-01)"),
            Some("1.82.0-nightly".to_string())
        );
    }

    // ── get_rust_version / get_cargo_version ──

    #[test]
    fn get_rust_version_succeeds() {
        let version = get_rust_version().expect("rustc should be available");
        assert!(version.starts_with("rustc"));
    }

    #[test]
    fn get_cargo_version_succeeds() {
        let version = get_cargo_version().expect("cargo should be available");
        assert!(version.starts_with("cargo"));
    }

    // ── collect_env_vars ──

    #[test]
    #[serial]
    fn collect_env_vars_captures_ci_var() {
        temp_env::with_var("GITHUB_REF", Some("refs/heads/main"), || {
            let vars = collect_env_vars();
            assert_eq!(
                vars.get("GITHUB_REF").map(String::as_str),
                Some("refs/heads/main")
            );
        });
    }

    #[test]
    #[serial]
    fn collect_env_vars_omits_unset_vars() {
        temp_env::with_var("CIRCLE_BUILD_NUM", None::<&str>, || {
            let vars = collect_env_vars();
            assert!(!vars.contains_key("CIRCLE_BUILD_NUM"));
        });
    }

    #[test]
    #[serial]
    fn collect_env_vars_captures_multiple() {
        temp_env::with_vars(
            [
                ("GITHUB_SHA", Some("abc123")),
                ("GITHUB_REPOSITORY", Some("owner/repo")),
            ],
            || {
                let vars = collect_env_vars();
                assert_eq!(vars.get("GITHUB_SHA").map(String::as_str), Some("abc123"));
                assert_eq!(
                    vars.get("GITHUB_REPOSITORY").map(String::as_str),
                    Some("owner/repo")
                );
            },
        );
    }

    #[test]
    #[serial]
    fn collect_env_vars_does_not_capture_cargo_home() {
        temp_env::with_var("CARGO_HOME", Some("/custom/cargo/home"), || {
            let vars = collect_env_vars();
            assert!(!vars.contains_key("CARGO_HOME"));
        });
    }

    #[test]
    #[serial]
    fn collect_env_vars_empty_value_still_captured() {
        temp_env::with_var("GITHUB_SHA", Some(""), || {
            let vars = collect_env_vars();
            assert_eq!(vars.get("GITHUB_SHA").map(String::as_str), Some(""));
        });
    }

    #[test]
    #[serial]
    fn collect_env_vars_all_known_keys() {
        let all_keys = [
            "CI",
            "GITHUB_REF",
            "GITHUB_SHA",
            "GITHUB_REPOSITORY",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_NUMBER",
            "GITLAB_CI_PIPELINE_ID",
            "CIRCLE_BUILD_NUM",
            "CIRCLE_BRANCH",
            "TRAVIS_BUILD_NUMBER",
            "TRAVIS_BRANCH",
            "BUILD_BUILDID",
            "BUILD_NUMBER",
            "BITBUCKET_BRANCH",
            "BITBUCKET_COMMIT",
        ];
        let overrides: Vec<(&str, Option<&str>)> =
            all_keys.iter().map(|&k| (k, Some("test_val"))).collect();
        temp_env::with_vars(overrides, || {
            let vars = collect_env_vars();
            for key in &all_keys {
                assert!(vars.contains_key(*key), "expected key {key} to be captured");
            }
            assert_eq!(vars.len(), all_keys.len());
        });
    }

    #[test]
    #[serial]
    fn collect_env_vars_with_unusual_cargo_home() {
        temp_env::with_var("CARGO_HOME", Some("/path with spaces/café/.cargo"), || {
            let info = EnvironmentInfo::collect();
            assert!(info.is_ok());
        });
    }

    // ── EnvironmentInfo ──

    #[test]
    fn environment_info_fingerprint_format() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };

        let fp = info.fingerprint();
        assert!(fp.contains("ci:Local"));
        assert!(fp.contains("os:linux"));
        assert!(fp.contains("arch:x86_64"));
        assert!(fp.contains("rust:1.70.0"));
        assert!(fp.contains("cargo:1.70.0"));
    }

    #[test]
    fn environment_info_fingerprint_includes_env_vars() {
        let mut vars = BTreeMap::new();
        vars.insert("GITHUB_SHA".to_string(), "abc123".to_string());
        vars.insert("GITHUB_REF".to_string(), "refs/heads/main".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: vars,
            collected_at: Utc::now(),
        };

        let fp = info.fingerprint();
        assert!(fp.contains("ci:GitHub Actions"));
        assert!(fp.contains("GITHUB_SHA:abc123"));
        assert!(fp.contains("GITHUB_REF:refs/heads/main"));
    }

    #[test]
    fn environment_info_fingerprint_pipe_separated() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "windows".to_string(),
            arch: "aarch64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };

        let fp = info.fingerprint();
        assert_eq!(fp.matches('|').count(), 4);
    }

    #[test]
    fn environment_info_collect_succeeds() {
        let info = EnvironmentInfo::collect().expect("collect should succeed");
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        assert!(!info.rust_version.is_empty());
    }

    #[test]
    fn environment_info_collect_os_matches_consts() {
        let info = EnvironmentInfo::collect().unwrap();
        assert_eq!(info.os, env::consts::OS);
        assert_eq!(info.arch, env::consts::ARCH);
    }

    #[test]
    fn environment_info_collect_all_fields_populated() {
        let info = EnvironmentInfo::collect().expect("collect should succeed");
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        assert!(!info.rust_version.is_empty());
        assert!(!info.cargo_version.is_empty());
        assert!(info.collected_at <= Utc::now());
    }

    #[test]
    fn environment_info_fingerprint_with_empty_strings() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: String::new(),
            arch: String::new(),
            rust_version: String::new(),
            cargo_version: String::new(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };
        let fp = info.fingerprint();
        assert!(fp.contains("ci:Local"));
        assert!(fp.contains("os:"));
        assert!(fp.contains("arch:"));
    }

    #[test]
    fn environment_info_fingerprint_env_vars_sorted() {
        let mut vars = BTreeMap::new();
        vars.insert("ZZVAR".to_string(), "z".to_string());
        vars.insert("AAVAR".to_string(), "a".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: vars,
            collected_at: Utc::now(),
        };

        let fp = info.fingerprint();
        let aa_pos = fp.find("AAVAR").expect("AAVAR should be in fingerprint");
        let zz_pos = fp.find("ZZVAR").expect("ZZVAR should be in fingerprint");
        assert!(aa_pos < zz_pos, "BTreeMap should maintain sorted order");
    }

    #[test]
    fn environment_info_fingerprint_empty_env_vars() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "test-os".to_string(),
            arch: "test-arch".to_string(),
            rust_version: "unknown".to_string(),
            cargo_version: "unknown".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };
        let fp = info.fingerprint();
        assert_eq!(fp.matches('|').count(), 4);
        assert!(!fp.ends_with('|'));
    }

    #[test]
    fn environment_info_fingerprint_is_deterministic_same_input() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };
        assert_eq!(info.fingerprint(), info.fingerprint());
    }

    #[test]
    fn fingerprint_starts_with_ci_prefix() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };
        assert!(info.fingerprint().starts_with("ci:"));
    }

    // ── get_environment_fingerprint ──

    #[test]
    fn get_environment_fingerprint_has_four_pipe_segments() {
        let fp = get_environment_fingerprint();
        assert!(!fp.is_empty());
        assert_eq!(fp.matches('|').count(), 3);
    }

    #[test]
    fn get_environment_fingerprint_contains_os() {
        let fp = get_environment_fingerprint();
        assert!(fp.contains(env::consts::OS));
        assert!(fp.contains(env::consts::ARCH));
    }

    #[test]
    fn get_environment_fingerprint_segments_are_non_empty() {
        let fp = get_environment_fingerprint();
        for (i, segment) in fp.split('|').enumerate() {
            assert!(!segment.is_empty(), "segment {i} should not be empty");
        }
    }

    #[test]
    #[serial]
    fn get_environment_fingerprint_is_reproducible() {
        let fp1 = get_environment_fingerprint();
        let fp2 = get_environment_fingerprint();
        assert_eq!(fp1, fp2, "same environment should produce same fingerprint");
    }

    #[test]
    #[serial]
    fn fingerprint_not_affected_by_cargo_home() {
        let all_ci = [
            "GITHUB_ACTIONS",
            "GITLAB_CI",
            "CIRCLECI",
            "TRAVIS",
            "TF_BUILD",
            "JENKINS_URL",
            "BITBUCKET_BUILD_NUMBER",
        ];
        let cleared: Vec<(&str, Option<&str>)> = all_ci.iter().map(|&v| (v, None)).collect();
        temp_env::with_vars(cleared, || {
            let fp1 = get_environment_fingerprint();

            temp_env::with_var("CARGO_HOME", Some("/some/other/path"), || {
                let fp2 = get_environment_fingerprint();
                assert_eq!(fp1, fp2, "CARGO_HOME should not affect fingerprint");
            });
        });
    }

    // ── Serialization ──

    #[test]
    fn environment_info_serialization_roundtrip() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };

        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"ci_environment\":\"GitHubActions\""));
        let deserialized: EnvironmentInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.ci_environment, CiEnvironment::GitHubActions);
        assert_eq!(deserialized.os, "linux");
        assert_eq!(deserialized.arch, "x86_64");
    }

    #[test]
    fn environment_info_with_env_vars_serializes() {
        let mut vars = BTreeMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        vars.insert("GITHUB_SHA".to_string(), "abc".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: vars,
            collected_at: Utc::now(),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"GITHUB_SHA\":\"abc\""));
        assert!(json.contains("\"CI\":\"true\""));
    }

    // ── Property-based tests ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_ci_environment() -> impl Strategy<Value = CiEnvironment> {
            prop_oneof![
                Just(CiEnvironment::GitHubActions),
                Just(CiEnvironment::GitLabCI),
                Just(CiEnvironment::CircleCI),
                Just(CiEnvironment::TravisCI),
                Just(CiEnvironment::AzurePipelines),
                Just(CiEnvironment::Jenkins),
                Just(CiEnvironment::BitbucketPipelines),
                Just(CiEnvironment::Local),
            ]
        }

        proptest! {
            #[test]
            fn fingerprint_contains_all_base_components(
                ci_env in arb_ci_environment(),
                os in "[a-z]{1,20}",
                arch in "[a-z0-9_]{1,20}",
                rust_ver in "[0-9]+\\.[0-9]+\\.[0-9]+",
                cargo_ver in "[0-9]+\\.[0-9]+\\.[0-9]+",
            ) {
                let info = EnvironmentInfo {
                    ci_environment: ci_env,
                    os: os.clone(),
                    arch: arch.clone(),
                    rust_version: rust_ver.clone(),
                    cargo_version: cargo_ver.clone(),
                    env_vars: BTreeMap::new(),
                    collected_at: Utc::now(),
                };
                let fp = info.fingerprint();
                let ci_str = format!("ci:{}", ci_env);
                let os_str = format!("os:{}", os);
                let arch_str = format!("arch:{}", arch);
                let rust_str = format!("rust:{}", rust_ver);
                let cargo_str = format!("cargo:{}", cargo_ver);
                prop_assert!(fp.contains(&ci_str));
                prop_assert!(fp.contains(&os_str));
                prop_assert!(fp.contains(&arch_str));
                prop_assert!(fp.contains(&rust_str));
                prop_assert!(fp.contains(&cargo_str));
            }

            #[test]
            fn fingerprint_pipe_count_equals_components_minus_one(
                ci_env in arb_ci_environment(),
                n_vars in 0usize..5,
            ) {
                let mut env_vars = BTreeMap::new();
                for i in 0..n_vars {
                    env_vars.insert(format!("VAR_{i}"), format!("val_{i}"));
                }
                let info = EnvironmentInfo {
                    ci_environment: ci_env,
                    os: "os".to_string(),
                    arch: "arch".to_string(),
                    rust_version: "1.0.0".to_string(),
                    cargo_version: "1.0.0".to_string(),
                    env_vars,
                    collected_at: Utc::now(),
                };
                let fp = info.fingerprint();
                let expected_pipes = 5 + n_vars - 1;
                prop_assert_eq!(fp.matches('|').count(), expected_pipes);
            }

            #[test]
            fn fingerprint_is_deterministic(
                os in "[a-z]{1,10}",
                arch in "[a-z0-9_]{1,10}",
            ) {
                let make = || {
                    let info = EnvironmentInfo {
                        ci_environment: CiEnvironment::Local,
                        os: os.clone(),
                        arch: arch.clone(),
                        rust_version: "1.80.0".to_string(),
                        cargo_version: "1.80.0".to_string(),
                        env_vars: BTreeMap::new(),
                        collected_at: Utc::now(),
                    };
                    info.fingerprint()
                };
                prop_assert_eq!(make(), make());
            }

            #[test]
            fn normalize_tool_version_with_two_tokens_returns_second(
                first in "[a-z]{1,10}",
                second in "[a-z0-9\\.]{1,20}",
            ) {
                let input = format!("{first} {second}");
                let result = normalize_tool_version(&input);
                prop_assert_eq!(result, Some(second));
            }

            #[test]
            fn normalize_tool_version_single_token_returns_none(
                single in "[a-zA-Z0-9_.-]{1,20}",
            ) {
                let result = normalize_tool_version(&single);
                prop_assert_eq!(result, None);
            }

            #[test]
            fn environment_info_serde_roundtrip(
                ci_env in arb_ci_environment(),
                os in "[a-z]{1,10}",
                arch in "[a-z0-9_]{1,10}",
                rust_ver in "[0-9]+\\.[0-9]+\\.[0-9]+",
                cargo_ver in "[0-9]+\\.[0-9]+\\.[0-9]+",
            ) {
                let info = EnvironmentInfo {
                    ci_environment: ci_env,
                    os,
                    arch,
                    rust_version: rust_ver,
                    cargo_version: cargo_ver,
                    env_vars: BTreeMap::new(),
                    collected_at: Utc::now(),
                };
                let json = serde_json::to_string(&info).unwrap();
                let back: EnvironmentInfo = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(info.ci_environment, back.ci_environment);
                prop_assert_eq!(info.os, back.os);
                prop_assert_eq!(info.arch, back.arch);
                prop_assert_eq!(info.rust_version, back.rust_version);
                prop_assert_eq!(info.cargo_version, back.cargo_version);
            }
        }

        proptest! {
            #[test]
            #[serial]
            fn collect_env_vars_never_captures_unknown_keys(
                key in "[A-Z_]{5,15}",
                value in "[a-zA-Z0-9_.-]{1,30}",
            ) {
                let known: std::collections::HashSet<&str> = [
                    "CI", "GITHUB_REF", "GITHUB_SHA", "GITHUB_REPOSITORY",
                    "GITHUB_RUN_ID", "GITHUB_RUN_NUMBER", "GITLAB_CI_PIPELINE_ID",
                    "CIRCLE_BUILD_NUM", "CIRCLE_BRANCH", "TRAVIS_BUILD_NUMBER",
                    "TRAVIS_BRANCH", "BUILD_BUILDID", "BUILD_NUMBER",
                    "BITBUCKET_BRANCH", "BITBUCKET_COMMIT",
                ].into_iter().collect();
                if known.contains(key.as_str()) {
                    return Ok(());
                }
                temp_env::with_vars([(key.as_str(), Some(value.as_str()))], || {
                    let vars = collect_env_vars();
                    prop_assert!(
                        !vars.contains_key(&key),
                        "Unknown key '{}' should not be captured", key
                    );
                    Ok(())
                })?;
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1))]

            #[test]
            fn get_environment_fingerprint_always_has_three_pipes(
                _dummy in 0u8..1,
            ) {
                let fp = get_environment_fingerprint();
                prop_assert_eq!(fp.matches('|').count(), 3);
                prop_assert!(!fp.is_empty());
            }

            #[test]
            fn arbitrary_env_info_produces_nonempty_fingerprint(
                ci_env in arb_ci_environment(),
                os in "\\PC{1,20}",
                arch in "\\PC{1,20}",
                rust_ver in "\\PC{1,30}",
                cargo_ver in "\\PC{1,30}",
            ) {
                let info = EnvironmentInfo {
                    ci_environment: ci_env,
                    os,
                    arch,
                    rust_version: rust_ver,
                    cargo_version: cargo_ver,
                    env_vars: BTreeMap::new(),
                    collected_at: Utc::now(),
                };
                let fp = info.fingerprint();
                prop_assert!(!fp.is_empty());
                prop_assert!(fp.matches('|').count() >= 4);
            }

            #[test]
            fn normalize_prerelease_versions(
                major in 0u32..100,
                minor in 0u32..100,
                patch in 0u32..100,
                pre in "[a-z]{1,8}\\.[0-9]{1,3}",
            ) {
                let ver = format!("{major}.{minor}.{patch}-{pre}");
                let input = format!("rustc {ver}");
                let result = normalize_tool_version(&input);
                prop_assert_eq!(result, Some(ver));
            }
        }
    }
}
