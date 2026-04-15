//! CI-specific helpers: branch, commit SHA, and pull-request detection.

use std::env;

use super::{CiEnvironment, detect_environment};

/// Get the current branch name from the CI environment.
pub(crate) fn get_ci_branch() -> Option<String> {
    let env_ = detect_environment();

    match env_ {
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

/// Get the current commit SHA from the CI environment.
pub(crate) fn get_ci_commit_sha() -> Option<String> {
    let env_ = detect_environment();

    match env_ {
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

/// Check if running on a pull request.
pub(crate) fn is_pull_request() -> bool {
    let env_ = detect_environment();

    match env_ {
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

    const ALL_CI_VARS: &[&str] = &[
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "CIRCLECI",
        "TRAVIS",
        "TF_BUILD",
        "JENKINS_URL",
        "BITBUCKET_BUILD_NUMBER",
    ];

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

    #[test]
    #[serial]
    fn get_ci_branch_returns_none_locally() {
        temp_env::with_vars(ci_env(&[]), || {
            assert!(get_ci_branch().is_none());
        });
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

    #[test]
    #[serial]
    fn get_ci_commit_sha_returns_none_locally() {
        temp_env::with_vars(ci_env(&[]), || {
            assert!(get_ci_commit_sha().is_none());
        });
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

    #[test]
    #[serial]
    fn is_pull_request_false_locally() {
        temp_env::with_vars(ci_env(&[]), || {
            assert!(!is_pull_request());
        });
    }
}
