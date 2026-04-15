//! Environment fingerprinting for shipper.
//!
//! Absorbed from the former `shipper-environment` microcrate. Provides
//! CI detection and environment fingerprinting for reproducible publish
//! operations and receipt evidence.
//!
//! The public entry points mirror the former crate's API but are
//! `pub(crate)`; the module is crate-private. See `CLAUDE.md` in this
//! folder for layer rules.

use std::env;

use serde::{Deserialize, Serialize};

pub(crate) mod ci;
pub(crate) mod fingerprint;

pub(crate) use fingerprint::EnvironmentInfo;

use crate::types::EnvironmentFingerprint;

/// Detected CI environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CiEnvironment {
    /// GitHub Actions
    GitHubActions,
    /// GitLab CI
    GitLabCI,
    /// CircleCI
    CircleCI,
    /// Travis CI
    TravisCI,
    /// Azure Pipelines
    AzurePipelines,
    /// Jenkins
    Jenkins,
    /// Bitbucket Pipelines
    BitbucketPipelines,
    /// No CI detected (local)
    #[default]
    Local,
}

impl std::fmt::Display for CiEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiEnvironment::GitHubActions => write!(f, "GitHub Actions"),
            CiEnvironment::GitLabCI => write!(f, "GitLab CI"),
            CiEnvironment::CircleCI => write!(f, "CircleCI"),
            CiEnvironment::TravisCI => write!(f, "Travis CI"),
            CiEnvironment::AzurePipelines => write!(f, "Azure Pipelines"),
            CiEnvironment::Jenkins => write!(f, "Jenkins"),
            CiEnvironment::BitbucketPipelines => write!(f, "Bitbucket Pipelines"),
            CiEnvironment::Local => write!(f, "Local"),
        }
    }
}

/// Detect the current CI environment.
pub(crate) fn detect_environment() -> CiEnvironment {
    if env::var("GITHUB_ACTIONS").is_ok() {
        return CiEnvironment::GitHubActions;
    }
    if env::var("GITLAB_CI").is_ok() {
        return CiEnvironment::GitLabCI;
    }
    if env::var("CIRCLECI").is_ok() {
        return CiEnvironment::CircleCI;
    }
    if env::var("TRAVIS").is_ok() {
        return CiEnvironment::TravisCI;
    }
    if env::var("TF_BUILD").is_ok() {
        return CiEnvironment::AzurePipelines;
    }
    if env::var("JENKINS_URL").is_ok() {
        return CiEnvironment::Jenkins;
    }
    if env::var("BITBUCKET_BUILD_NUMBER").is_ok() {
        return CiEnvironment::BitbucketPipelines;
    }
    CiEnvironment::Local
}

/// Check if running in any CI environment.
pub(crate) fn is_ci() -> bool {
    detect_environment() != CiEnvironment::Local
}

/// Convert command output like `rustc 1.92.0` into `Some("1.92.0")`.
///
/// Preserves the PR #53 shim helper name for backward reference.
fn normalize_version(raw: &str) -> Option<String> {
    let normalized = raw.split_whitespace().collect::<Vec<_>>();
    if normalized.len() >= 2 {
        Some(normalized[1].to_string())
    } else {
        None
    }
}

/// Collect a structured environment fingerprint compatible with `shipper` runtime types.
///
/// Preserves the deduped PR #53 shim wrapper logic: delegates to
/// `EnvironmentInfo::collect` and falls back to minimal info on failure,
/// then normalizes the tool version strings via `normalize_version`.
pub(crate) fn collect_environment_fingerprint() -> EnvironmentFingerprint {
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
    use serial_test::serial;

    /// All CI detection variables that `detect_environment` checks.
    const ALL_CI_VARS: &[&str] = &[
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "CIRCLECI",
        "TRAVIS",
        "TF_BUILD",
        "JENKINS_URL",
        "BITBUCKET_BUILD_NUMBER",
    ];

    pub(super) fn ci_env<'a>(
        overrides: &'a [(&'a str, Option<&'a str>)],
    ) -> Vec<(&'a str, Option<&'a str>)> {
        let mut vars: Vec<(&str, Option<&str>)> = ALL_CI_VARS.iter().map(|&v| (v, None)).collect();
        for &(k, v) in overrides {
            if let Some(pos) = vars.iter().position(|(key, _)| *key == k) {
                vars[pos] = (k, v);
            } else {
                vars.push((k, v));
            }
        }
        vars
    }

    // ── CiEnvironment Display ──

    #[test]
    fn ci_environment_display_all_variants() {
        assert_eq!(CiEnvironment::GitHubActions.to_string(), "GitHub Actions");
        assert_eq!(CiEnvironment::GitLabCI.to_string(), "GitLab CI");
        assert_eq!(CiEnvironment::CircleCI.to_string(), "CircleCI");
        assert_eq!(CiEnvironment::TravisCI.to_string(), "Travis CI");
        assert_eq!(CiEnvironment::AzurePipelines.to_string(), "Azure Pipelines");
        assert_eq!(CiEnvironment::Jenkins.to_string(), "Jenkins");
        assert_eq!(
            CiEnvironment::BitbucketPipelines.to_string(),
            "Bitbucket Pipelines"
        );
        assert_eq!(CiEnvironment::Local.to_string(), "Local");
    }

    #[test]
    fn ci_environment_default_is_local() {
        assert_eq!(CiEnvironment::default(), CiEnvironment::Local);
    }

    #[test]
    fn ci_environment_clone_and_copy() {
        let a = CiEnvironment::GitHubActions;
        let b = a;
        #[allow(clippy::clone_on_copy)]
        let c = a.clone();
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    // ── detect_environment ──

    #[test]
    #[serial]
    fn detect_github_actions() {
        temp_env::with_vars(ci_env(&[("GITHUB_ACTIONS", Some("true"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::GitHubActions);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_gitlab_ci() {
        temp_env::with_vars(ci_env(&[("GITLAB_CI", Some("true"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::GitLabCI);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_circleci() {
        temp_env::with_vars(ci_env(&[("CIRCLECI", Some("true"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::CircleCI);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_travis_ci() {
        temp_env::with_vars(ci_env(&[("TRAVIS", Some("true"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::TravisCI);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_azure_pipelines() {
        temp_env::with_vars(ci_env(&[("TF_BUILD", Some("True"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::AzurePipelines);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_jenkins() {
        temp_env::with_vars(
            ci_env(&[("JENKINS_URL", Some("http://jenkins.local"))]),
            || {
                assert_eq!(detect_environment(), CiEnvironment::Jenkins);
                assert!(is_ci());
            },
        );
    }

    #[test]
    #[serial]
    fn detect_bitbucket_pipelines() {
        temp_env::with_vars(ci_env(&[("BITBUCKET_BUILD_NUMBER", Some("42"))]), || {
            assert_eq!(detect_environment(), CiEnvironment::BitbucketPipelines);
            assert!(is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_local_when_no_ci_vars() {
        temp_env::with_vars(ci_env(&[]), || {
            assert_eq!(detect_environment(), CiEnvironment::Local);
            assert!(!is_ci());
        });
    }

    #[test]
    #[serial]
    fn detect_environment_priority_github_over_others() {
        temp_env::with_vars(
            ci_env(&[
                ("GITHUB_ACTIONS", Some("true")),
                ("GITLAB_CI", Some("true")),
            ]),
            || {
                assert_eq!(detect_environment(), CiEnvironment::GitHubActions);
            },
        );
    }

    #[test]
    #[serial]
    fn detect_environment_priority_gitlab_over_later() {
        temp_env::with_vars(
            ci_env(&[
                ("GITLAB_CI", Some("true")),
                ("CIRCLECI", Some("true")),
                ("TRAVIS", Some("true")),
            ]),
            || {
                assert_eq!(detect_environment(), CiEnvironment::GitLabCI);
            },
        );
    }

    #[test]
    #[serial]
    fn detect_environment_priority_circleci_over_travis() {
        temp_env::with_vars(
            ci_env(&[("CIRCLECI", Some("true")), ("TRAVIS", Some("true"))]),
            || {
                assert_eq!(detect_environment(), CiEnvironment::CircleCI);
            },
        );
    }

    #[test]
    #[serial]
    fn detect_environment_with_empty_value() {
        temp_env::with_vars(ci_env(&[("GITHUB_ACTIONS", Some(""))]), || {
            assert_eq!(detect_environment(), CiEnvironment::GitHubActions);
        });
    }

    #[test]
    #[serial]
    fn detect_environment_all_ci_vars_set_picks_github() {
        temp_env::with_vars(
            vec![
                ("GITHUB_ACTIONS", Some("true")),
                ("GITLAB_CI", Some("true")),
                ("CIRCLECI", Some("true")),
                ("TRAVIS", Some("true")),
                ("TF_BUILD", Some("true")),
                ("JENKINS_URL", Some("http://j")),
                ("BITBUCKET_BUILD_NUMBER", Some("1")),
            ],
            || {
                assert_eq!(detect_environment(), CiEnvironment::GitHubActions);
            },
        );
    }

    // ── normalize_version (the PR #53 shim helper) ──

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

    // ── collect_environment_fingerprint (absorbed shim behavior) ──

    #[test]
    fn collect_environment_fingerprint_has_expected_shape() {
        let fp = collect_environment_fingerprint();
        assert!(!fp.shipper_version.is_empty());
        assert!(!fp.os.is_empty());
        assert!(!fp.arch.is_empty());
    }

    #[test]
    fn collect_environment_fingerprint_returns_structured_values() {
        let fp = collect_environment_fingerprint();
        assert!(!fp.shipper_version.is_empty());
        assert!(!fp.os.is_empty());
        assert!(!fp.arch.is_empty());
        assert_eq!(fp.os, env::consts::OS);
        assert_eq!(fp.arch, env::consts::ARCH);
    }

    #[test]
    fn collect_environment_fingerprint_versions_are_normalized() {
        let fp = collect_environment_fingerprint();
        if let Some(ref cv) = fp.cargo_version {
            assert!(!cv.starts_with("cargo"), "should be normalized: {cv}");
        }
        if let Some(ref rv) = fp.rust_version {
            assert!(!rv.starts_with("rustc"), "should be normalized: {rv}");
        }
    }

    #[test]
    fn collect_environment_fingerprint_all_fields_populated() {
        let fp = collect_environment_fingerprint();
        assert!(
            !fp.shipper_version.is_empty(),
            "shipper_version must be set"
        );
        assert!(!fp.os.is_empty(), "os must be set");
        assert!(!fp.arch.is_empty(), "arch must be set");
        assert!(fp.rust_version.is_some(), "rust_version should be Some");
        assert!(fp.cargo_version.is_some(), "cargo_version should be Some");
    }

    #[test]
    fn collect_fingerprint_os_arch_match_std() {
        let fp = collect_environment_fingerprint();
        assert_eq!(fp.os, env::consts::OS);
        assert_eq!(fp.arch, env::consts::ARCH);
    }

    #[test]
    #[serial]
    fn collect_environment_fingerprint_is_reproducible() {
        let fp1 = collect_environment_fingerprint();
        let fp2 = collect_environment_fingerprint();
        assert_eq!(fp1.shipper_version, fp2.shipper_version);
        assert_eq!(fp1.os, fp2.os);
        assert_eq!(fp1.arch, fp2.arch);
        assert_eq!(fp1.rust_version, fp2.rust_version);
        assert_eq!(fp1.cargo_version, fp2.cargo_version);
    }

    #[test]
    fn shipper_version_matches_cargo_pkg_version() {
        let fp = collect_environment_fingerprint();
        assert_eq!(fp.shipper_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn shipper_version_handles_prerelease() {
        let version = env!("CARGO_PKG_VERSION");
        let parts: Vec<&str> = version.split('-').next().unwrap().split('.').collect();
        assert!(
            parts.len() >= 3,
            "version should have major.minor.patch: {version}"
        );
        for part in &parts {
            assert!(
                part.parse::<u32>().is_ok(),
                "non-numeric version component: {part}"
            );
        }
    }

    // ── OS/arch sanity ──

    #[test]
    fn os_is_known_platform() {
        let known = ["windows", "linux", "macos", "freebsd", "openbsd", "netbsd"];
        let os = env::consts::OS;
        assert!(known.contains(&os), "unexpected OS: {os}");
    }

    #[test]
    fn arch_is_known_architecture() {
        let known = [
            "x86_64",
            "x86",
            "aarch64",
            "arm",
            "mips",
            "mips64",
            "powerpc",
            "powerpc64",
            "riscv64",
            "s390x",
        ];
        let arch = env::consts::ARCH;
        assert!(known.contains(&arch), "unexpected arch: {arch}");
    }

    #[test]
    fn ci_environment_debug_impl() {
        let debug = format!("{:?}", CiEnvironment::GitHubActions);
        assert_eq!(debug, "GitHubActions");
    }

    // ── Serialization ──

    #[test]
    fn ci_environment_serialization_all_variants() {
        let variants = [
            (CiEnvironment::GitHubActions, "\"GitHubActions\""),
            (CiEnvironment::GitLabCI, "\"GitLabCI\""),
            (CiEnvironment::CircleCI, "\"CircleCI\""),
            (CiEnvironment::TravisCI, "\"TravisCI\""),
            (CiEnvironment::AzurePipelines, "\"AzurePipelines\""),
            (CiEnvironment::Jenkins, "\"Jenkins\""),
            (CiEnvironment::BitbucketPipelines, "\"BitbucketPipelines\""),
            (CiEnvironment::Local, "\"Local\""),
        ];
        for (variant, expected_json) in variants {
            let json = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(json, expected_json, "serialization of {variant:?}");
            let deserialized: CiEnvironment = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(deserialized, variant);
        }
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

        fn ci_var_for(env: &CiEnvironment) -> Option<&'static str> {
            match env {
                CiEnvironment::GitHubActions => Some("GITHUB_ACTIONS"),
                CiEnvironment::GitLabCI => Some("GITLAB_CI"),
                CiEnvironment::CircleCI => Some("CIRCLECI"),
                CiEnvironment::TravisCI => Some("TRAVIS"),
                CiEnvironment::AzurePipelines => Some("TF_BUILD"),
                CiEnvironment::Jenkins => Some("JENKINS_URL"),
                CiEnvironment::BitbucketPipelines => Some("BITBUCKET_BUILD_NUMBER"),
                CiEnvironment::Local => None,
            }
        }

        proptest! {
            #[test]
            #[serial]
            fn detect_environment_returns_correct_provider_for_any_value(
                ci_env_ in arb_ci_environment().prop_filter(
                    "skip Local - it has no trigger var",
                    |e| *e != CiEnvironment::Local,
                ),
                value in "[a-zA-Z0-9_.-]+",
            ) {
                let var = ci_var_for(&ci_env_).unwrap();
                let pair = (var, Some(value.as_str()));
                let overrides = [pair];
                let env_spec = super::ci_env(&overrides);
                temp_env::with_vars(env_spec, || {
                    prop_assert_eq!(detect_environment(), ci_env_);
                    prop_assert!(is_ci());
                    Ok(())
                })?;
            }

            #[test]
            #[serial]
            fn detect_local_when_all_ci_vars_cleared(
                dummy in "[a-z]*",
            ) {
                let _ = dummy;
                let env_spec = super::ci_env(&[]);
                temp_env::with_vars(env_spec, || {
                    prop_assert_eq!(detect_environment(), CiEnvironment::Local);
                    prop_assert!(!is_ci());
                    Ok(())
                })?;
            }

            #[test]
            #[serial]
            fn setting_single_ci_var_detects_that_provider(
                idx in 0usize..7,
                value in "[a-zA-Z0-9_.-]{1,50}",
            ) {
                let providers = [
                    CiEnvironment::GitHubActions,
                    CiEnvironment::GitLabCI,
                    CiEnvironment::CircleCI,
                    CiEnvironment::TravisCI,
                    CiEnvironment::AzurePipelines,
                    CiEnvironment::Jenkins,
                    CiEnvironment::BitbucketPipelines,
                ];
                let expected = providers[idx];
                let var = ci_var_for(&expected).unwrap();
                let pair = (var, Some(value.as_str()));
                let overrides = [pair];
                let env_spec = super::ci_env(&overrides);
                temp_env::with_vars(env_spec, || {
                    let detected = detect_environment();
                    prop_assert_eq!(detected, expected);
                    Ok(())
                })?;
            }

            #[test]
            fn ci_environment_display_never_panics(ci_env in arb_ci_environment()) {
                let display = format!("{ci_env}");
                prop_assert!(!display.is_empty());
            }

            #[test]
            fn ci_environment_serde_roundtrip(ci_env in arb_ci_environment()) {
                let json = serde_json::to_string(&ci_env).unwrap();
                let back: CiEnvironment = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(ci_env, back);
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1))]

            #[test]
            fn collect_environment_fingerprint_os_arch_are_stable(
                _dummy in 0u8..1,
            ) {
                let fp = collect_environment_fingerprint();
                prop_assert_eq!(fp.os, env::consts::OS);
                prop_assert_eq!(fp.arch, env::consts::ARCH);
                prop_assert!(!fp.shipper_version.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::types::EnvironmentFingerprint;
    use insta::assert_yaml_snapshot;
    use std::collections::BTreeMap;

    #[test]
    fn ci_environment_all_display_variants() {
        let variants: Vec<String> = [
            CiEnvironment::GitHubActions,
            CiEnvironment::GitLabCI,
            CiEnvironment::CircleCI,
            CiEnvironment::TravisCI,
            CiEnvironment::AzurePipelines,
            CiEnvironment::Jenkins,
            CiEnvironment::BitbucketPipelines,
            CiEnvironment::Local,
        ]
        .iter()
        .map(|v| v.to_string())
        .collect();

        assert_yaml_snapshot!(variants);
    }

    #[test]
    fn ci_environment_all_debug_variants() {
        let variants: Vec<String> = [
            CiEnvironment::GitHubActions,
            CiEnvironment::GitLabCI,
            CiEnvironment::CircleCI,
            CiEnvironment::TravisCI,
            CiEnvironment::AzurePipelines,
            CiEnvironment::Jenkins,
            CiEnvironment::BitbucketPipelines,
            CiEnvironment::Local,
        ]
        .iter()
        .map(|v| format!("{v:?}"))
        .collect();

        assert_yaml_snapshot!(variants);
    }

    #[test]
    fn ci_environment_serialization() {
        let variants = [
            CiEnvironment::GitHubActions,
            CiEnvironment::GitLabCI,
            CiEnvironment::CircleCI,
            CiEnvironment::TravisCI,
            CiEnvironment::AzurePipelines,
            CiEnvironment::Jenkins,
            CiEnvironment::BitbucketPipelines,
            CiEnvironment::Local,
        ];

        let serialized: Vec<String> = variants
            .iter()
            .map(|v| serde_json::to_string(v).unwrap())
            .collect();

        assert_yaml_snapshot!(serialized);
    }

    #[test]
    fn environment_info_fingerprint_local_no_vars() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info);
    }

    #[test]
    fn environment_info_fingerprint_github_actions_with_vars() {
        let mut env_vars = BTreeMap::new();
        env_vars.insert("GITHUB_REF".to_string(), "refs/heads/main".to_string());
        env_vars.insert("GITHUB_SHA".to_string(), "abc123def456789".to_string());
        env_vars.insert("GITHUB_REPOSITORY".to_string(), "owner/repo".to_string());
        env_vars.insert("GITHUB_RUN_ID".to_string(), "12345".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars,
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-06-01T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info);
    }

    #[test]
    fn environment_info_fingerprint_string_local() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            rust_version: "1.82.0".to_string(),
            cargo_version: "1.82.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info.fingerprint());
    }

    #[test]
    fn environment_info_fingerprint_string_ci_with_vars() {
        let mut env_vars = BTreeMap::new();
        env_vars.insert("CI".to_string(), "true".to_string());
        env_vars.insert("GITHUB_SHA".to_string(), "deadbeef".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars,
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info.fingerprint());
    }

    #[test]
    fn environment_fingerprint_structured() {
        let fp = EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: Some("1.80.0".to_string()),
            rust_version: Some("1.80.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        assert_yaml_snapshot!(fp);
    }

    #[test]
    fn environment_fingerprint_structured_unknown_versions() {
        let fp = EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "windows".to_string(),
            arch: "aarch64".to_string(),
        };

        assert_yaml_snapshot!(fp);
    }

    #[test]
    fn environment_info_gitlab_ci() {
        let mut env_vars = BTreeMap::new();
        env_vars.insert("GITLAB_CI_PIPELINE_ID".to_string(), "98765".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitLabCI,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.79.0".to_string(),
            cargo_version: "1.79.0".to_string(),
            env_vars,
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-03-15T08:30:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info);
    }

    #[test]
    fn environment_fingerprint_prerelease_version() {
        let fp = EnvironmentFingerprint {
            shipper_version: "0.3.0-rc.1".to_string(),
            cargo_version: Some("1.82.0-nightly".to_string()),
            rust_version: Some("1.82.0-beta.3".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        assert_yaml_snapshot!(fp);
    }

    #[test]
    fn environment_info_windows_aarch64() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::AzurePipelines,
            os: "windows".to_string(),
            arch: "aarch64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info);
    }

    #[test]
    fn environment_info_many_env_vars() {
        let mut env_vars = BTreeMap::new();
        env_vars.insert("CI".to_string(), "true".to_string());
        env_vars.insert("GITHUB_REF".to_string(), "refs/tags/v1.0.0".to_string());
        env_vars.insert("GITHUB_SHA".to_string(), "abcdef1234567890".to_string());
        env_vars.insert("GITHUB_REPOSITORY".to_string(), "owner/repo".to_string());
        env_vars.insert("GITHUB_RUN_ID".to_string(), "999".to_string());
        env_vars.insert("GITHUB_RUN_NUMBER".to_string(), "42".to_string());

        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.80.0".to_string(),
            cargo_version: "1.80.0".to_string(),
            env_vars,
            collected_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        };

        assert_yaml_snapshot!(info.fingerprint());
    }
}
