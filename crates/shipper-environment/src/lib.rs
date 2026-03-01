//! Environment fingerprinting for shipper.
//!
//! This crate provides environment detection and fingerprinting
//! for reproducible publish operations across CI environments.
//!
//! # Example
//!
//! ```
//! use shipper_environment::{CiEnvironment, detect_environment, get_environment_fingerprint};
//!
//! // Detect the current CI environment
//! let env = detect_environment();
//! println!("Running in: {:?}", env);
//!
//! // Get a fingerprint for reproducibility
//! let fingerprint = get_environment_fingerprint();
//! println!("Fingerprint: {}", fingerprint);
//! ```

use std::collections::BTreeMap;
use std::env;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shipper_types::EnvironmentFingerprint;

/// Detected CI environment
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiEnvironment {
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

/// Detect the current CI environment
pub fn detect_environment() -> CiEnvironment {
    // GitHub Actions
    if env::var("GITHUB_ACTIONS").is_ok() {
        return CiEnvironment::GitHubActions;
    }

    // GitLab CI
    if env::var("GITLAB_CI").is_ok() {
        return CiEnvironment::GitLabCI;
    }

    // CircleCI
    if env::var("CIRCLECI").is_ok() {
        return CiEnvironment::CircleCI;
    }

    // Travis CI
    if env::var("TRAVIS").is_ok() {
        return CiEnvironment::TravisCI;
    }

    // Azure Pipelines
    if env::var("TF_BUILD").is_ok() {
        return CiEnvironment::AzurePipelines;
    }

    // Jenkins
    if env::var("JENKINS_URL").is_ok() {
        return CiEnvironment::Jenkins;
    }

    // Bitbucket Pipelines
    if env::var("BITBUCKET_BUILD_NUMBER").is_ok() {
        return CiEnvironment::BitbucketPipelines;
    }

    CiEnvironment::Local
}

/// Check if running in any CI environment
pub fn is_ci() -> bool {
    detect_environment() != CiEnvironment::Local
}

/// Environment information collected for fingerprinting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// Detected CI environment
    pub ci_environment: CiEnvironment,
    /// Operating system
    pub os: String,
    /// Architecture
    pub arch: String,
    /// Rust version
    pub rust_version: String,
    /// Cargo version
    pub cargo_version: String,
    /// Collected environment variables (sanitized)
    pub env_vars: BTreeMap<String, String>,
    /// Timestamp of collection
    pub collected_at: DateTime<Utc>,
}

impl EnvironmentInfo {
    /// Collect current environment information
    pub fn collect() -> Result<Self> {
        let ci_environment = detect_environment();
        let os = env::consts::OS.to_string();
        let arch = env::consts::ARCH.to_string();

        // Get rust version
        let rust_version = get_rust_version().unwrap_or_else(|_| "unknown".to_string());

        // Get cargo version
        let cargo_version = get_cargo_version().unwrap_or_else(|_| "unknown".to_string());

        // Collect relevant environment variables (sanitized)
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

    /// Generate a fingerprint string
    pub fn fingerprint(&self) -> String {
        let mut components = Vec::new();
        components.push(format!("ci:{}", self.ci_environment));
        components.push(format!("os:{}", self.os));
        components.push(format!("arch:{}", self.arch));
        components.push(format!("rust:{}", self.rust_version));
        components.push(format!("cargo:{}", self.cargo_version));

        // Add relevant env vars
        for (key, value) in &self.env_vars {
            components.push(format!("{}:{}", key, value));
        }

        components.join("|")
    }
}

/// Get a quick environment fingerprint
pub fn get_environment_fingerprint() -> String {
    let ci = detect_environment();
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let rust = get_rust_version().unwrap_or_else(|_| "unknown".to_string());

    format!("{}|{}|{}|{}", ci, os, arch, rust)
}

fn normalize_tool_version(raw: &str) -> Option<String> {
    raw.split_whitespace().nth(1).map(ToOwned::to_owned)
}

/// Collect a structured environment fingerprint compatible with `shipper` runtime types.
pub fn collect_environment_fingerprint() -> EnvironmentFingerprint {
    EnvironmentFingerprint {
        shipper_version: env!("CARGO_PKG_VERSION").to_string(),
        cargo_version: get_cargo_version()
            .ok()
            .and_then(|raw| normalize_tool_version(&raw)),
        rust_version: get_rust_version()
            .ok()
            .and_then(|raw| normalize_tool_version(&raw)),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
    }
}

/// Get the Rust version
pub fn get_rust_version() -> Result<String> {
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

/// Get the Cargo version
pub fn get_cargo_version() -> Result<String> {
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

/// Collect relevant environment variables for fingerprinting
fn collect_env_vars() -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();

    // CI-related variables that are safe to include
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

/// Get the current branch name from CI environment
pub fn get_ci_branch() -> Option<String> {
    let env = detect_environment();

    match env {
        CiEnvironment::GitHubActions => env::var("GITHUB_REF_NAME").ok(),
        CiEnvironment::GitLabCI => env::var("CI_COMMIT_REF_NAME").ok(),
        CiEnvironment::CircleCI => env::var("CIRCLE_BRANCH").ok(),
        CiEnvironment::TravisCI => env::var("TRAVIS_BRANCH").ok(),
        CiEnvironment::AzurePipelines => env::var("BUILD_SOURCEBRANCHNAME").ok(),
        CiEnvironment::Jenkins => env::var("GIT_BRANCH").ok(),
        CiEnvironment::BitbucketPipelines => env::var("BITBUCKET_BRANCH").ok(),
        CiEnvironment::Local => None,
    }
}

/// Get the current commit SHA from CI environment
pub fn get_ci_commit_sha() -> Option<String> {
    let env = detect_environment();

    match env {
        CiEnvironment::GitHubActions => env::var("GITHUB_SHA").ok(),
        CiEnvironment::GitLabCI => env::var("CI_COMMIT_SHA").ok(),
        CiEnvironment::CircleCI => env::var("CIRCLE_SHA1").ok(),
        CiEnvironment::TravisCI => env::var("TRAVIS_COMMIT").ok(),
        CiEnvironment::AzurePipelines => env::var("BUILD_SOURCEVERSION").ok(),
        CiEnvironment::Jenkins => env::var("GIT_COMMIT").ok(),
        CiEnvironment::BitbucketPipelines => env::var("BITBUCKET_COMMIT").ok(),
        CiEnvironment::Local => None,
    }
}

/// Check if running on a pull request
pub fn is_pull_request() -> bool {
    let env = detect_environment();

    match env {
        CiEnvironment::GitHubActions => env::var("GITHUB_EVENT_NAME")
            .map(|v| v == "pull_request")
            .unwrap_or(false),
        CiEnvironment::GitLabCI => env::var("CI_MERGE_REQUEST_ID").is_ok(),
        CiEnvironment::CircleCI => env::var("CIRCLE_PULL_REQUEST").is_ok(),
        CiEnvironment::TravisCI => env::var("TRAVIS_PULL_REQUEST")
            .map(|v| v != "false")
            .unwrap_or(false),
        CiEnvironment::AzurePipelines => env::var("BUILD_REASON")
            .map(|v| v == "PullRequest")
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ── Helper: env vars to clear so detection falls through to Local ──

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

    /// Build a `temp_env::with_vars` list that clears all CI vars
    /// plus sets the provided overrides.
    fn ci_env<'a>(overrides: &'a [(&'a str, Option<&'a str>)]) -> Vec<(&'a str, Option<&'a str>)> {
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
        let b = a; // Copy
        #[allow(clippy::clone_on_copy)]
        let c = a.clone(); // Clone
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    // ── detect_environment – one test per CI provider ──

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
        // Only one token → no second element
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

    // ── EnvironmentInfo::fingerprint ──

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
        // Base components: ci, os, arch, rust, cargo = 5 pipe-separated parts
        assert_eq!(fp.matches('|').count(), 4);
    }

    // ── EnvironmentInfo::collect ──

    #[test]
    fn environment_info_collect_succeeds() {
        let info = EnvironmentInfo::collect().expect("collect should succeed");
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        // rust_version may be "unknown" if rustc is missing, but in test env it should work
        assert!(!info.rust_version.is_empty());
    }

    #[test]
    fn environment_info_collect_os_matches_consts() {
        let info = EnvironmentInfo::collect().unwrap();
        assert_eq!(info.os, env::consts::OS);
        assert_eq!(info.arch, env::consts::ARCH);
    }

    // ── get_environment_fingerprint ──

    #[test]
    fn get_environment_fingerprint_has_four_pipe_segments() {
        let fp = get_environment_fingerprint();
        assert!(!fp.is_empty());
        // format: ci|os|arch|rust  → 3 pipes
        assert_eq!(fp.matches('|').count(), 3);
    }

    #[test]
    fn get_environment_fingerprint_contains_os() {
        let fp = get_environment_fingerprint();
        assert!(fp.contains(env::consts::OS));
        assert!(fp.contains(env::consts::ARCH));
    }

    // ── collect_environment_fingerprint ──

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
        // cargo_version and rust_version should be just the semver part, not the full output
        if let Some(ref cv) = fp.cargo_version {
            assert!(!cv.starts_with("cargo"), "should be normalized: {cv}");
        }
        if let Some(ref rv) = fp.rust_version {
            assert!(!rv.starts_with("rustc"), "should be normalized: {rv}");
        }
    }

    // ── get_ci_branch ──

    #[test]
    #[serial]
    fn get_ci_branch_github_actions() {
        temp_env::with_vars(
            ci_env(&[
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_REF_NAME", Some("main")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("main".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_gitlab_ci() {
        temp_env::with_vars(
            ci_env(&[
                ("GITLAB_CI", Some("true")),
                ("CI_COMMIT_REF_NAME", Some("develop")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("develop".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_circleci() {
        temp_env::with_vars(
            ci_env(&[
                ("CIRCLECI", Some("true")),
                ("CIRCLE_BRANCH", Some("feature/test")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("feature/test".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_travis() {
        temp_env::with_vars(
            ci_env(&[
                ("TRAVIS", Some("true")),
                ("TRAVIS_BRANCH", Some("release/v1")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("release/v1".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_azure() {
        temp_env::with_vars(
            ci_env(&[
                ("TF_BUILD", Some("True")),
                ("BUILD_SOURCEBRANCHNAME", Some("main")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("main".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_jenkins() {
        temp_env::with_vars(
            ci_env(&[
                ("JENKINS_URL", Some("http://ci.local")),
                ("GIT_BRANCH", Some("origin/main")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("origin/main".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_bitbucket() {
        temp_env::with_vars(
            ci_env(&[
                ("BITBUCKET_BUILD_NUMBER", Some("99")),
                ("BITBUCKET_BRANCH", Some("hotfix")),
            ]),
            || {
                assert_eq!(get_ci_branch(), Some("hotfix".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_branch_returns_none_for_local() {
        temp_env::with_vars(ci_env(&[]), || {
            assert_eq!(get_ci_branch(), None);
        });
    }

    #[test]
    #[serial]
    fn get_ci_branch_none_when_branch_var_missing() {
        temp_env::with_vars(
            ci_env(&[("GITHUB_ACTIONS", Some("true")), ("GITHUB_REF_NAME", None)]),
            || {
                assert_eq!(get_ci_branch(), None);
            },
        );
    }

    // ── get_ci_commit_sha ──

    #[test]
    #[serial]
    fn get_ci_commit_sha_github() {
        temp_env::with_vars(
            ci_env(&[
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_SHA", Some("abc123def456")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("abc123def456".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_gitlab() {
        temp_env::with_vars(
            ci_env(&[
                ("GITLAB_CI", Some("true")),
                ("CI_COMMIT_SHA", Some("deadbeef")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("deadbeef".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_circleci() {
        temp_env::with_vars(
            ci_env(&[
                ("CIRCLECI", Some("true")),
                ("CIRCLE_SHA1", Some("cafebabe")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("cafebabe".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_travis() {
        temp_env::with_vars(
            ci_env(&[
                ("TRAVIS", Some("true")),
                ("TRAVIS_COMMIT", Some("aabbccdd")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("aabbccdd".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_azure() {
        temp_env::with_vars(
            ci_env(&[
                ("TF_BUILD", Some("True")),
                ("BUILD_SOURCEVERSION", Some("11223344")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("11223344".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_jenkins() {
        temp_env::with_vars(
            ci_env(&[
                ("JENKINS_URL", Some("http://ci.local")),
                ("GIT_COMMIT", Some("55667788")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("55667788".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_bitbucket() {
        temp_env::with_vars(
            ci_env(&[
                ("BITBUCKET_BUILD_NUMBER", Some("1")),
                ("BITBUCKET_COMMIT", Some("99aabb")),
            ]),
            || {
                assert_eq!(get_ci_commit_sha(), Some("99aabb".to_string()));
            },
        );
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_returns_none_for_local() {
        temp_env::with_vars(ci_env(&[]), || {
            assert_eq!(get_ci_commit_sha(), None);
        });
    }

    #[test]
    #[serial]
    fn get_ci_commit_sha_none_when_sha_var_missing() {
        temp_env::with_vars(
            ci_env(&[("GITLAB_CI", Some("true")), ("CI_COMMIT_SHA", None)]),
            || {
                assert_eq!(get_ci_commit_sha(), None);
            },
        );
    }

    // ── is_pull_request ──

    #[test]
    #[serial]
    fn is_pull_request_github_true() {
        temp_env::with_vars(
            ci_env(&[
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_EVENT_NAME", Some("pull_request")),
            ]),
            || {
                assert!(is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_github_false_on_push() {
        temp_env::with_vars(
            ci_env(&[
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_EVENT_NAME", Some("push")),
            ]),
            || {
                assert!(!is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_gitlab_true() {
        temp_env::with_vars(
            ci_env(&[
                ("GITLAB_CI", Some("true")),
                ("CI_MERGE_REQUEST_ID", Some("42")),
            ]),
            || {
                assert!(is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_gitlab_false() {
        temp_env::with_vars(
            ci_env(&[("GITLAB_CI", Some("true")), ("CI_MERGE_REQUEST_ID", None)]),
            || {
                assert!(!is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_circleci_true() {
        temp_env::with_vars(
            ci_env(&[
                ("CIRCLECI", Some("true")),
                (
                    "CIRCLE_PULL_REQUEST",
                    Some("https://github.com/org/repo/pull/1"),
                ),
            ]),
            || {
                assert!(is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_travis_true() {
        temp_env::with_vars(
            ci_env(&[
                ("TRAVIS", Some("true")),
                ("TRAVIS_PULL_REQUEST", Some("42")),
            ]),
            || {
                assert!(is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_travis_false_when_false_string() {
        temp_env::with_vars(
            ci_env(&[
                ("TRAVIS", Some("true")),
                ("TRAVIS_PULL_REQUEST", Some("false")),
            ]),
            || {
                assert!(!is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_azure_true() {
        temp_env::with_vars(
            ci_env(&[
                ("TF_BUILD", Some("True")),
                ("BUILD_REASON", Some("PullRequest")),
            ]),
            || {
                assert!(is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_azure_false_on_manual() {
        temp_env::with_vars(
            ci_env(&[("TF_BUILD", Some("True")), ("BUILD_REASON", Some("Manual"))]),
            || {
                assert!(!is_pull_request());
            },
        );
    }

    #[test]
    #[serial]
    fn is_pull_request_jenkins_always_false() {
        temp_env::with_vars(ci_env(&[("JENKINS_URL", Some("http://ci.local"))]), || {
            assert!(!is_pull_request());
        });
    }

    #[test]
    #[serial]
    fn is_pull_request_bitbucket_always_false() {
        temp_env::with_vars(ci_env(&[("BITBUCKET_BUILD_NUMBER", Some("1"))]), || {
            assert!(!is_pull_request());
        });
    }

    #[test]
    #[serial]
    fn is_pull_request_false_for_local() {
        temp_env::with_vars(ci_env(&[]), || {
            assert!(!is_pull_request());
        });
    }

    // ── Serialization / deserialization ──

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

    // ── Edge cases ──

    #[test]
    #[serial]
    fn detect_environment_with_empty_value() {
        // Setting a CI var to an empty string still counts as "set"
        temp_env::with_vars(ci_env(&[("GITHUB_ACTIONS", Some(""))]), || {
            assert_eq!(detect_environment(), CiEnvironment::GitHubActions);
        });
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
    fn ci_environment_debug_impl() {
        let debug = format!("{:?}", CiEnvironment::GitHubActions);
        assert_eq!(debug, "GitHubActions");
    }

    // ── Property-based tests (proptest) ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating arbitrary `CiEnvironment` variants.
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

        /// The CI detection env var for each provider.
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

        // ── Environment detection with arbitrary values ──

        proptest! {
            #[test]
            #[serial]
            fn detect_environment_returns_correct_provider_for_any_value(
                ci_env in arb_ci_environment().prop_filter(
                    "skip Local - it has no trigger var",
                    |e| *e != CiEnvironment::Local,
                ),
                value in "[a-zA-Z0-9_.-]+",
            ) {
                let var = ci_var_for(&ci_env).unwrap();
                let pair = (var, Some(value.as_str()));
                let overrides = [pair];
                let env_spec = super::ci_env(&overrides);
                temp_env::with_vars(env_spec, || {
                    prop_assert_eq!(detect_environment(), ci_env);
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
        }

        // ── CI detection: setting any single CI var is detected ──

        proptest! {
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
        }

        // ── Fingerprint properties ──

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
                // 5 base components + n_vars env var components
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
        }

        // ── normalize_tool_version properties ──

        proptest! {
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
        }

        // ── CiEnvironment Display roundtrip ──

        proptest! {
            #[test]
            fn ci_environment_display_never_panics(ci_env in arb_ci_environment()) {
                let display = format!("{ci_env}");
                prop_assert!(!display.is_empty());
            }
        }

        // ── Serialization roundtrip ──

        proptest! {
            #[test]
            fn ci_environment_serde_roundtrip(ci_env in arb_ci_environment()) {
                let json = serde_json::to_string(&ci_env).unwrap();
                let back: CiEnvironment = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(ci_env, back);
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

        // ── collect_env_vars: only known keys are captured ──

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

        // ── Hostname / workspace path handling ──

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
