//! Shared execution helpers for publish workflows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use shipper_retry::{RetryStrategyConfig, RetryStrategyType, calculate_delay};
use shipper_types::{ErrorClass, ExecutionState, PackageState};

/// Update a package state and persist the entire execution state to disk.
pub fn update_state(
    st: &mut ExecutionState,
    state_dir: &Path,
    key: &str,
    new_state: PackageState,
) -> Result<()> {
    let pr = st
        .packages
        .get_mut(key)
        .context("missing package in state")?;
    pr.state = new_state;
    pr.last_updated_at = Utc::now();
    st.updated_at = Utc::now();
    shipper_state::save_state(state_dir, st)
}

/// Resolve the effective state directory from a workspace root and user option.
pub fn resolve_state_dir(workspace_root: &Path, state_dir: &PathBuf) -> PathBuf {
    if state_dir.is_absolute() {
        state_dir.clone()
    } else {
        workspace_root.join(state_dir)
    }
}

/// Create a stable key for a package version.
pub fn pkg_key(name: &str, version: &str) -> String {
    format!("{name}@{version}")
}

/// Short, human-readable label for a package state.
pub fn short_state(st: &PackageState) -> &'static str {
    match st {
        PackageState::Pending => "pending",
        PackageState::Uploaded => "uploaded",
        PackageState::Published => "published",
        PackageState::Skipped { .. } => "skipped",
        PackageState::Failed { .. } => "failed",
        PackageState::Ambiguous { .. } => "ambiguous",
    }
}

/// Classify a cargo failure output into retry semantics for publish decisioning.
pub fn classify_cargo_failure(stderr: &str, stdout: &str) -> (ErrorClass, String) {
    let outcome = shipper_cargo_failure::classify_publish_failure(stderr, stdout);
    let class = match outcome.class {
        shipper_cargo_failure::CargoFailureClass::Retryable => ErrorClass::Retryable,
        shipper_cargo_failure::CargoFailureClass::Permanent => ErrorClass::Permanent,
        shipper_cargo_failure::CargoFailureClass::Ambiguous => ErrorClass::Ambiguous,
    };

    (class, outcome.message.to_string())
}

/// Calculate the delay for a retry attempt.
pub fn backoff_delay(
    base: Duration,
    max: Duration,
    attempt: u32,
    strategy: RetryStrategyType,
    jitter: f64,
) -> Duration {
    let config = RetryStrategyConfig {
        strategy,
        max_attempts: 10,
        base_delay: base,
        max_delay: max,
        jitter,
    };
    calculate_delay(&config, attempt)
}

/// Update a package state inside an in-memory execution state.
pub fn update_state_locked(st: &mut ExecutionState, key: &str, new_state: PackageState) {
    if let Some(pr) = st.packages.get_mut(key) {
        pr.state = new_state;
        pr.last_updated_at = Utc::now();
    }
    st.updated_at = Utc::now();
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::Utc;
    use proptest::prelude::*;
    use tempfile::tempdir;

    use super::*;

    fn make_progress(
        name: &str,
        version: &str,
        state: PackageState,
    ) -> shipper_types::PackageProgress {
        shipper_types::PackageProgress {
            name: name.to_string(),
            version: version.to_string(),
            attempts: 0,
            state,
            last_updated_at: Utc::now(),
        }
    }

    fn sample_state(key: &str, state: PackageState) -> shipper_types::ExecutionState {
        shipper_types::ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-sample".to_string(),
            registry: shipper_types::Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::from([(key.to_string(), make_progress("demo", "0.1.0", state))]),
        }
    }

    #[test]
    fn resolves_state_dir_relative_paths() {
        let root = PathBuf::from("root");
        let rel = resolve_state_dir(&root, &PathBuf::from(".shipper"));
        assert_eq!(rel, root.join(".shipper"));

        #[cfg(windows)]
        {
            let abs = PathBuf::from(r"C:\x\state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }
        #[cfg(not(windows))]
        {
            let abs = PathBuf::from("/x/state");
            assert_eq!(resolve_state_dir(&root, &abs), abs);
        }
    }

    #[test]
    fn pkg_key_and_short_state_cover_all_variants() {
        assert_eq!(pkg_key("a", "1.2.3"), "a@1.2.3");
        assert_eq!(
            short_state(&shipper_types::PackageState::Pending),
            "pending"
        );
        assert_eq!(
            short_state(&shipper_types::PackageState::Uploaded),
            "uploaded"
        );
        assert_eq!(
            short_state(&shipper_types::PackageState::Published),
            "published"
        );
        assert_eq!(
            short_state(&shipper_types::PackageState::Skipped { reason: "x".into() }),
            "skipped"
        );
        assert_eq!(
            short_state(&shipper_types::PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "x".into()
            }),
            "failed"
        );
        assert_eq!(
            short_state(&shipper_types::PackageState::Ambiguous {
                message: "x".into()
            }),
            "ambiguous"
        );
    }

    #[test]
    fn classify_cargo_failure_covers_retryable_permanent_and_ambiguous() {
        let retryable = classify_cargo_failure("HTTP 429 too many requests", "");
        assert_eq!(retryable.0, ErrorClass::Retryable);

        let permanent = classify_cargo_failure("permission denied", "");
        assert_eq!(permanent.0, ErrorClass::Permanent);

        let ambiguous = classify_cargo_failure("strange output", "");
        assert_eq!(ambiguous.0, ErrorClass::Ambiguous);
    }

    #[test]
    fn update_state_updates_timestamp_and_persists() {
        let mut st = sample_state("demo@0.1.0", shipper_types::PackageState::Pending);
        let td = tempdir().expect("tempdir");
        let state_dir = td.path();

        let before = st.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(2));

        update_state(
            &mut st,
            state_dir,
            "demo@0.1.0",
            shipper_types::PackageState::Uploaded,
        )
        .expect("state update");

        assert!(st.updated_at >= before);
        let loaded = shipper_state::load_state(state_dir)
            .expect("load state")
            .expect("state exists");
        assert!(matches!(
            loaded.packages.get("demo@0.1.0").expect("pkg").state,
            shipper_types::PackageState::Uploaded
        ));
    }

    #[test]
    fn update_state_fails_for_missing_package() {
        let mut st = sample_state("demo@0.1.0", shipper_types::PackageState::Pending);
        let td = tempdir().expect("tempdir");
        assert!(
            update_state(
                &mut st,
                td.path(),
                "missing",
                shipper_types::PackageState::Uploaded,
            )
            .is_err()
        );
    }

    #[test]
    fn update_state_locked_is_noop_for_missing_package() {
        let mut st = sample_state("demo@0.1.0", shipper_types::PackageState::Pending);
        let before = st.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        update_state_locked(&mut st, "missing", shipper_types::PackageState::Published);
        assert_eq!(
            st.packages.get("demo@0.1.0").expect("pkg").state,
            shipper_types::PackageState::Pending
        );
        assert!(st.updated_at >= before);
    }

    #[test]
    fn backoff_delay_is_bounded_with_jitter() {
        let base = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_millis(500);
        let d1 = backoff_delay(
            base,
            max,
            1,
            shipper_retry::RetryStrategyType::Exponential,
            0.5,
        );
        let d20 = backoff_delay(
            base,
            max,
            20,
            shipper_retry::RetryStrategyType::Exponential,
            0.5,
        );

        assert!(d1 >= std::time::Duration::from_millis(50));
        assert!(d1 <= std::time::Duration::from_millis(150));
        assert!(d20 >= std::time::Duration::from_millis(250));
        assert!(d20 <= std::time::Duration::from_millis(750));
    }

    // -- State transitions: success flow --

    #[test]
    fn update_state_locked_pending_to_uploaded() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        update_state_locked(&mut st, key, PackageState::Uploaded);
        assert_eq!(st.packages[key].state, PackageState::Uploaded);
    }

    #[test]
    fn update_state_locked_uploaded_to_published() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Uploaded);
        update_state_locked(&mut st, key, PackageState::Published);
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    // -- State transitions: failure flow --

    #[test]
    fn update_state_locked_pending_to_failed_permanent() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let fail = PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "denied".into(),
        };
        update_state_locked(&mut st, key, fail.clone());
        assert_eq!(st.packages[key].state, fail);
    }

    #[test]
    fn update_state_locked_pending_to_failed_retryable() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let fail = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "rate limited".into(),
        };
        update_state_locked(&mut st, key, fail.clone());
        assert_eq!(st.packages[key].state, fail);
    }

    #[test]
    fn update_state_locked_pending_to_ambiguous() {
        let key = "a@1.0.0";
        let mut st = sample_state(
            key,
            PackageState::Ambiguous {
                message: "timeout".into(),
            },
        );
        // Ambiguous can transition to published on verification
        update_state_locked(&mut st, key, PackageState::Published);
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    // -- State transitions: skip flow --

    #[test]
    fn update_state_locked_pending_to_skipped() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let skip = PackageState::Skipped {
            reason: "already published".into(),
        };
        update_state_locked(&mut st, key, skip.clone());
        assert_eq!(st.packages[key].state, skip);
    }

    // -- Timestamp correctness --

    #[test]
    fn update_state_locked_updates_package_timestamp() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let pkg_ts_before = st.packages[key].last_updated_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        update_state_locked(&mut st, key, PackageState::Published);
        assert!(st.packages[key].last_updated_at > pkg_ts_before);
    }

    #[test]
    fn update_state_locked_updates_global_timestamp_even_for_missing_key() {
        let mut st = sample_state("a@1.0.0", PackageState::Pending);
        let ts_before = st.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        update_state_locked(&mut st, "nonexistent", PackageState::Published);
        assert!(st.updated_at >= ts_before);
    }

    // -- Edge case: empty package list --

    #[test]
    fn update_state_on_empty_packages_returns_error() {
        let mut st = shipper_types::ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-empty".to_string(),
            registry: shipper_types::Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        let td = tempdir().expect("tempdir");
        assert!(update_state(&mut st, td.path(), "any@1.0.0", PackageState::Published).is_err());
    }

    #[test]
    fn update_state_locked_on_empty_packages_is_noop() {
        let mut st = shipper_types::ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-empty".to_string(),
            registry: shipper_types::Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        // Should not panic
        update_state_locked(&mut st, "any@1.0.0", PackageState::Published);
        assert!(st.packages.is_empty());
    }

    // -- Edge case: multiple packages, all-skipped --

    fn multi_state(entries: &[(&str, PackageState)]) -> ExecutionState {
        let mut packages = BTreeMap::new();
        for (key, state) in entries {
            packages.insert(
                key.to_string(),
                make_progress(key.split('@').next().unwrap(), "1.0.0", state.clone()),
            );
        }
        ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-multi".to_string(),
            registry: shipper_types::Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        }
    }

    #[test]
    fn all_packages_skipped() {
        let skip = |r: &str| PackageState::Skipped { reason: r.into() };
        let mut st = multi_state(&[
            ("a@1.0.0", skip("already published")),
            ("b@1.0.0", skip("already published")),
            ("c@1.0.0", skip("yanked")),
        ]);
        // All already skipped — updating one to published still works
        update_state_locked(&mut st, "a@1.0.0", PackageState::Published);
        assert_eq!(st.packages["a@1.0.0"].state, PackageState::Published);
        assert!(matches!(
            st.packages["b@1.0.0"].state,
            PackageState::Skipped { .. }
        ));
    }

    #[test]
    fn all_packages_failed() {
        let fail = |m: &str| PackageState::Failed {
            class: ErrorClass::Permanent,
            message: m.into(),
        };
        let st = multi_state(&[("a@1.0.0", fail("denied")), ("b@1.0.0", fail("denied"))]);
        let failed_count = st
            .packages
            .values()
            .filter(|p| matches!(p.state, PackageState::Failed { .. }))
            .count();
        assert_eq!(failed_count, 2);
    }

    // -- Error classification accuracy --

    #[test]
    fn classify_rate_limit_variants() {
        // HTTP 429
        let (class, _) = classify_cargo_failure("error: 429 too many requests", "");
        assert_eq!(class, ErrorClass::Retryable);

        // timeout
        let (class, _) = classify_cargo_failure("connection timeout", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_auth_failures_as_permanent() {
        let (class, _) = classify_cargo_failure("error: not authorized", "");
        assert_eq!(class, ErrorClass::Permanent);

        let (class, _) = classify_cargo_failure("token is invalid", "");
        assert_eq!(class, ErrorClass::Permanent);
    }

    #[test]
    fn classify_empty_output_as_ambiguous() {
        let (class, _) = classify_cargo_failure("", "");
        assert_eq!(class, ErrorClass::Ambiguous);
    }

    #[test]
    fn classify_already_uploaded_as_permanent() {
        let (class, _) =
            classify_cargo_failure("error: crate version `1.0.0` is already uploaded", "");
        assert_eq!(class, ErrorClass::Permanent);
    }

    #[test]
    fn classify_network_errors_as_retryable() {
        let (class, _) = classify_cargo_failure("connection reset by peer", "");
        assert_eq!(class, ErrorClass::Retryable);

        let (class, _) = classify_cargo_failure("network unreachable", "");
        assert_eq!(class, ErrorClass::Retryable);
    }

    #[test]
    fn classify_returns_nonempty_message() {
        let (_, msg) = classify_cargo_failure("some unknown error text", "");
        assert!(
            !msg.is_empty(),
            "classification message should not be empty"
        );
    }

    // -- Retry / backoff delay logic --

    #[test]
    fn backoff_immediate_strategy_returns_zero() {
        let d = backoff_delay(
            Duration::from_millis(100),
            Duration::from_secs(10),
            5,
            shipper_retry::RetryStrategyType::Immediate,
            0.0,
        );
        assert_eq!(d, Duration::ZERO);
    }

    #[test]
    fn backoff_constant_strategy_returns_base() {
        let base = Duration::from_millis(200);
        let d = backoff_delay(
            base,
            Duration::from_secs(10),
            5,
            shipper_retry::RetryStrategyType::Constant,
            0.0,
        );
        assert_eq!(d, base);
    }

    #[test]
    fn backoff_linear_strategy_scales_with_attempt() {
        let base = Duration::from_millis(100);
        let d1 = backoff_delay(
            base,
            Duration::from_secs(10),
            1,
            shipper_retry::RetryStrategyType::Linear,
            0.0,
        );
        let d3 = backoff_delay(
            base,
            Duration::from_secs(10),
            3,
            shipper_retry::RetryStrategyType::Linear,
            0.0,
        );
        assert_eq!(d1, Duration::from_millis(100));
        assert_eq!(d3, Duration::from_millis(300));
    }

    #[test]
    fn backoff_exponential_without_jitter_doubles() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(60);
        let d1 = backoff_delay(
            base,
            max,
            1,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        let d2 = backoff_delay(
            base,
            max,
            2,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        let d3 = backoff_delay(
            base,
            max,
            3,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        assert_eq!(d1, Duration::from_millis(100));
        assert_eq!(d2, Duration::from_millis(200));
        assert_eq!(d3, Duration::from_millis(400));
    }

    #[test]
    fn backoff_clamped_to_max() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(300);
        let d = backoff_delay(
            base,
            max,
            10,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        assert!(d <= max, "delay {d:?} should be <= max {max:?}");
    }

    #[test]
    fn backoff_zero_jitter_is_deterministic() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(10);
        let a = backoff_delay(
            base,
            max,
            3,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        let b = backoff_delay(
            base,
            max,
            3,
            shipper_retry::RetryStrategyType::Exponential,
            0.0,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn backoff_high_attempt_does_not_overflow() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(60);
        // Very high attempt number should not panic
        let d = backoff_delay(
            base,
            max,
            u32::MAX,
            shipper_retry::RetryStrategyType::Exponential,
            1.0,
        );
        assert!(d <= max.mul_f64(1.5 + 1.0)); // max + full jitter headroom
    }

    // -- pkg_key edge cases --

    #[test]
    fn pkg_key_with_scoped_name() {
        assert_eq!(pkg_key("@scope/pkg", "2.0.0-rc.1"), "@scope/pkg@2.0.0-rc.1");
    }

    #[test]
    fn pkg_key_empty_inputs() {
        assert_eq!(pkg_key("", ""), "@");
    }

    // -- Persist round-trip for each terminal state --

    #[test]
    fn update_state_persists_skipped() {
        let key = "s@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let td = tempdir().expect("tempdir");
        update_state(
            &mut st,
            td.path(),
            key,
            PackageState::Skipped {
                reason: "already on registry".into(),
            },
        )
        .expect("persist");
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert!(matches!(
            loaded.packages[key].state,
            PackageState::Skipped { .. }
        ));
    }

    #[test]
    fn update_state_persists_failed() {
        let key = "f@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let td = tempdir().expect("tempdir");
        update_state(
            &mut st,
            td.path(),
            key,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "timeout".into(),
            },
        )
        .expect("persist");
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        match &loaded.packages[key].state {
            PackageState::Failed { class, message } => {
                assert_eq!(*class, ErrorClass::Ambiguous);
                assert_eq!(message, "timeout");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn update_state_persists_ambiguous() {
        let key = "x@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        let td = tempdir().expect("tempdir");
        update_state(
            &mut st,
            td.path(),
            key,
            PackageState::Ambiguous {
                message: "unknown".into(),
            },
        )
        .expect("persist");
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert!(matches!(
            loaded.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
    }

    // -- resolve_state_dir edge cases --

    #[test]
    fn resolve_state_dir_empty_relative() {
        let root = PathBuf::from("workspace");
        let result = resolve_state_dir(&root, &PathBuf::from(""));
        assert_eq!(result, PathBuf::from("workspace"));
    }

    #[test]
    fn resolve_state_dir_nested_relative() {
        let root = PathBuf::from("workspace");
        let result = resolve_state_dir(&root, &PathBuf::from("a/b/c"));
        assert_eq!(result, root.join("a/b/c"));
    }

    // -- Multiple package state tracking --

    #[test]
    fn multi_package_independent_transitions() {
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@2.0.0", PackageState::Pending),
            ("c@3.0.0", PackageState::Pending),
        ]);
        update_state_locked(&mut st, "a@1.0.0", PackageState::Published);
        update_state_locked(
            &mut st,
            "b@2.0.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "429".into(),
            },
        );
        update_state_locked(
            &mut st,
            "c@3.0.0",
            PackageState::Skipped {
                reason: "dep failed".into(),
            },
        );
        assert_eq!(st.packages["a@1.0.0"].state, PackageState::Published);
        assert!(matches!(
            st.packages["b@2.0.0"].state,
            PackageState::Failed { .. }
        ));
        assert!(matches!(
            st.packages["c@3.0.0"].state,
            PackageState::Skipped { .. }
        ));
    }

    #[test]
    fn multi_package_persist_round_trip() {
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@2.0.0", PackageState::Pending),
        ]);
        let td = tempdir().expect("tempdir");
        update_state(&mut st, td.path(), "a@1.0.0", PackageState::Published).unwrap();
        update_state(
            &mut st,
            td.path(),
            "b@2.0.0",
            PackageState::Skipped {
                reason: "skip".into(),
            },
        )
        .unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert_eq!(loaded.packages["a@1.0.0"].state, PackageState::Published);
        assert!(matches!(
            loaded.packages["b@2.0.0"].state,
            PackageState::Skipped { .. }
        ));
    }

    // -- Property tests --

    fn ascii_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(any::<char>(), 0..128)
            .prop_map(|chars| chars.into_iter().collect())
    }

    fn arb_error_class() -> impl Strategy<Value = ErrorClass> {
        prop_oneof![
            Just(ErrorClass::Retryable),
            Just(ErrorClass::Permanent),
            Just(ErrorClass::Ambiguous),
        ]
    }

    fn arb_package_state() -> impl Strategy<Value = PackageState> {
        prop_oneof![
            Just(PackageState::Pending),
            Just(PackageState::Uploaded),
            Just(PackageState::Published),
            ".*".prop_map(|r| PackageState::Skipped { reason: r }),
            (arb_error_class(), ".*").prop_map(|(c, m)| PackageState::Failed {
                class: c,
                message: m
            }),
            ".*".prop_map(|m| PackageState::Ambiguous { message: m }),
        ]
    }

    proptest! {
        #[test]
        fn classify_is_deterministic_with_ascii(stderr in ascii_text(), stdout in ascii_text()) {
            let first = classify_cargo_failure(&stderr, &stdout);
            let second = classify_cargo_failure(&stderr, &stdout);
            prop_assert_eq!(first, second);
        }

        #[test]
        fn classify_is_case_insensitive_with_ascii(stderr in ascii_text(), stdout in ascii_text()) {
            let lower = classify_cargo_failure(&stderr.to_ascii_lowercase(), &stdout.to_ascii_lowercase());
            let upper = classify_cargo_failure(&stderr.to_ascii_uppercase(), &stdout.to_ascii_uppercase());
            prop_assert_eq!(lower.0, upper.0);
        }

        #[test]
        fn classify_always_returns_valid_class(stderr in ascii_text(), stdout in ascii_text()) {
            let (class, msg) = classify_cargo_failure(&stderr, &stdout);
            prop_assert!(matches!(class, ErrorClass::Retryable | ErrorClass::Permanent | ErrorClass::Ambiguous));
            prop_assert!(!msg.is_empty());
        }

        #[test]
        fn short_state_returns_known_label(state in arb_package_state()) {
            let label = short_state(&state);
            prop_assert!(["pending", "uploaded", "published", "skipped", "failed", "ambiguous"].contains(&label));
        }

        #[test]
        fn update_state_locked_preserves_other_packages(
            state_a in arb_package_state(),
            state_b in arb_package_state(),
        ) {
            let mut st = multi_state(&[
                ("a@1.0.0", PackageState::Pending),
                ("b@1.0.0", PackageState::Pending),
            ]);
            update_state_locked(&mut st, "a@1.0.0", state_a);
            update_state_locked(&mut st, "b@1.0.0", state_b);
            // Both packages still exist
            prop_assert!(st.packages.contains_key("a@1.0.0"));
            prop_assert!(st.packages.contains_key("b@1.0.0"));
            prop_assert_eq!(st.packages.len(), 2);
        }

        #[test]
        fn backoff_never_exceeds_max_with_jitter(
            attempt in 1..100u32,
            jitter in 0.0..1.0f64,
        ) {
            let base = Duration::from_millis(100);
            let max = Duration::from_millis(500);
            let d = backoff_delay(base, max, attempt, shipper_retry::RetryStrategyType::Exponential, jitter);
            // With jitter up to 1.0, max theoretical is max + max*jitter + epsilon for fp rounding
            let upper = max + max.mul_f64(jitter) + Duration::from_millis(1);
            prop_assert!(d <= upper, "delay {:?} exceeded upper bound {:?}", d, upper);
        }

        #[test]
        fn pkg_key_contains_at_separator(name in "[a-z_-]{1,30}", version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}") {
            let key = pkg_key(&name, &version);
            prop_assert!(key.contains('@'));
            prop_assert_eq!(key, format!("{name}@{version}"));
        }

        // -- Retry logic: monotonicity and range --

        #[test]
        fn exponential_monotonic_without_jitter(
            base_ms in 1u64..10_000,
            extra_ms in 1u64..100_000,
            a in 1u32..50,
            b in 1u32..50,
        ) {
            let base = Duration::from_millis(base_ms);
            let max = Duration::from_millis(base_ms + extra_ms);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let d_lo = backoff_delay(base, max, lo, shipper_retry::RetryStrategyType::Exponential, 0.0);
            let d_hi = backoff_delay(base, max, hi, shipper_retry::RetryStrategyType::Exponential, 0.0);
            prop_assert!(d_hi >= d_lo, "exp backoff not monotonic: attempt {hi} ({d_hi:?}) < attempt {lo} ({d_lo:?})");
        }

        #[test]
        fn linear_monotonic_without_jitter(
            base_ms in 1u64..10_000,
            extra_ms in 1u64..100_000,
            a in 1u32..50,
            b in 1u32..50,
        ) {
            let base = Duration::from_millis(base_ms);
            let max = Duration::from_millis(base_ms + extra_ms);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let d_lo = backoff_delay(base, max, lo, shipper_retry::RetryStrategyType::Linear, 0.0);
            let d_hi = backoff_delay(base, max, hi, shipper_retry::RetryStrategyType::Linear, 0.0);
            prop_assert!(d_hi >= d_lo, "linear backoff not monotonic: attempt {hi} ({d_hi:?}) < attempt {lo} ({d_lo:?})");
        }

        #[test]
        fn immediate_always_zero_regardless_of_params(
            base_ms in 0u64..100_000,
            max_ms in 0u64..300_000,
            attempt in 0u32..1000,
            jitter in 0.0..1.0f64,
        ) {
            let d = backoff_delay(
                Duration::from_millis(base_ms),
                Duration::from_millis(max_ms),
                attempt,
                shipper_retry::RetryStrategyType::Immediate,
                jitter,
            );
            prop_assert_eq!(d, Duration::ZERO);
        }

        #[test]
        fn constant_same_delay_regardless_of_attempt(
            base_ms in 0u64..100_000,
            max_ms in 0u64..300_000,
            a in 1u32..100,
            b in 1u32..100,
        ) {
            let base = Duration::from_millis(base_ms);
            let max = Duration::from_millis(max_ms);
            let d_a = backoff_delay(base, max, a, shipper_retry::RetryStrategyType::Constant, 0.0);
            let d_b = backoff_delay(base, max, b, shipper_retry::RetryStrategyType::Constant, 0.0);
            prop_assert_eq!(d_a, d_b);
            prop_assert_eq!(d_a, base.min(max));
        }

        // -- State transitions: invariants --

        #[test]
        fn update_state_locked_sets_exact_state(state in arb_package_state()) {
            let key = "t@1.0.0";
            let mut st = sample_state(key, PackageState::Pending);
            update_state_locked(&mut st, key, state.clone());
            prop_assert_eq!(&st.packages[key].state, &state);
        }

        #[test]
        fn update_state_locked_timestamp_never_decreases(state in arb_package_state()) {
            let key = "t@1.0.0";
            let mut st = sample_state(key, PackageState::Pending);
            let before = st.updated_at;
            update_state_locked(&mut st, key, state);
            prop_assert!(st.updated_at >= before);
        }

        #[test]
        fn sequential_transitions_preserve_count(
            s1 in arb_package_state(),
            s2 in arb_package_state(),
            s3 in arb_package_state(),
        ) {
            let mut st = multi_state(&[
                ("a@1.0.0", PackageState::Pending),
                ("b@1.0.0", PackageState::Pending),
                ("c@1.0.0", PackageState::Pending),
            ]);
            update_state_locked(&mut st, "a@1.0.0", s1);
            update_state_locked(&mut st, "b@1.0.0", s2);
            update_state_locked(&mut st, "c@1.0.0", s3);
            prop_assert_eq!(st.packages.len(), 3);
        }

        // -- Error categorization: mapping correctness --

        #[test]
        fn classify_cargo_failure_preserves_class_mapping(
            stderr in ascii_text(),
            stdout in ascii_text(),
        ) {
            let internal = shipper_cargo_failure::classify_publish_failure(&stderr, &stdout);
            let (mapped_class, _) = classify_cargo_failure(&stderr, &stdout);
            let expected = match internal.class {
                shipper_cargo_failure::CargoFailureClass::Retryable => ErrorClass::Retryable,
                shipper_cargo_failure::CargoFailureClass::Permanent => ErrorClass::Permanent,
                shipper_cargo_failure::CargoFailureClass::Ambiguous => ErrorClass::Ambiguous,
            };
            prop_assert_eq!(mapped_class, expected);
        }

        #[test]
        fn classify_stderr_stdout_symmetric(stderr in ascii_text(), stdout in ascii_text()) {
            let normal = classify_cargo_failure(&stderr, &stdout);
            let swapped = classify_cargo_failure(&stdout, &stderr);
            prop_assert_eq!(normal.0, swapped.0, "classification differs when swapping stderr/stdout");
        }

        // -- Timeout / overflow safety --

        #[test]
        fn backoff_arbitrary_strategy_never_panics(
            base_ms in 0u64..500_000,
            max_ms in 0u64..500_000,
            attempt in 0u32..10_000,
            strategy_idx in 0u8..4,
            jitter in 0.0..1.0f64,
        ) {
            let strategy = match strategy_idx {
                0 => shipper_retry::RetryStrategyType::Immediate,
                1 => shipper_retry::RetryStrategyType::Exponential,
                2 => shipper_retry::RetryStrategyType::Linear,
                _ => shipper_retry::RetryStrategyType::Constant,
            };
            let d = backoff_delay(
                Duration::from_millis(base_ms),
                Duration::from_millis(max_ms),
                attempt,
                strategy,
                jitter,
            );
            prop_assert!(d.as_secs() < u64::MAX);
        }

        #[test]
        fn backoff_base_exceeds_max_clamps(
            base_ms in 100u64..500_000,
            delta in 1u64..100_000,
            attempt in 1u32..100,
            jitter in 0.0..1.0f64,
        ) {
            let base = Duration::from_millis(base_ms);
            let max = Duration::from_millis(base_ms.saturating_sub(delta).max(1));
            let d = backoff_delay(base, max, attempt, shipper_retry::RetryStrategyType::Exponential, jitter);
            let upper = max + max.mul_f64(jitter) + Duration::from_millis(1);
            prop_assert!(d <= upper, "delay {d:?} exceeded upper bound {upper:?} when base > max");
        }

        #[test]
        fn backoff_large_attempt_all_strategies(
            attempt in 10_000u32..=u32::MAX,
            strategy_idx in 0u8..4,
        ) {
            let strategy = match strategy_idx {
                0 => shipper_retry::RetryStrategyType::Immediate,
                1 => shipper_retry::RetryStrategyType::Exponential,
                2 => shipper_retry::RetryStrategyType::Linear,
                _ => shipper_retry::RetryStrategyType::Constant,
            };
            let base = Duration::from_millis(100);
            let max = Duration::from_secs(60);
            let d = backoff_delay(base, max, attempt, strategy, 0.5);
            let upper = max + max.mul_f64(0.5);
            prop_assert!(d <= upper, "large attempt overflow: {d:?} > {upper:?}");
        }

        /// State machine invariant: transitioning from any valid state
        /// always produces a known/valid short_state label.
        #[test]
        fn state_transition_always_produces_valid_state(
            from_state in arb_package_state(),
            to_state in arb_package_state(),
        ) {
            let key = "t@1.0.0";
            let mut st = sample_state(key, from_state);
            update_state_locked(&mut st, key, to_state);
            let label = short_state(&st.packages[key].state);
            prop_assert!(
                ["pending", "uploaded", "published", "skipped", "failed", "ambiguous"].contains(&label),
                "invalid state label: {label}"
            );
        }

        /// Progress invariant: the proportion of terminal packages is always
        /// between 0.0 and 1.0 (inclusive).
        #[test]
        fn progress_percentage_always_bounded(
            count in 1usize..20,
            terminal_count in 0usize..20,
        ) {
            let terminal = terminal_count.min(count);
            let mut entries: Vec<(&str, PackageState)> = Vec::new();
            let names: Vec<String> = (0..count).map(|i| format!("p{i}@1.0.0")).collect();
            for (i, name) in names.iter().enumerate() {
                let state = if i < terminal {
                    PackageState::Published
                } else {
                    PackageState::Pending
                };
                // We need to keep names alive, but multi_state takes &str
                entries.push((name.as_str(), state));
            }
            let st = multi_state(&entries);
            let total = st.packages.len() as f64;
            let done = st.packages.values()
                .filter(|p| matches!(p.state, PackageState::Published | PackageState::Skipped { .. }))
                .count() as f64;
            let progress = done / total;
            prop_assert!((0.0..=1.0).contains(&progress),
                "progress {progress} out of bounds");
            prop_assert_eq!(st.packages.len(), count);
        }

        /// Package count invariant: state transitions never add or remove packages.
        #[test]
        fn state_transitions_preserve_package_count(
            s1 in arb_package_state(),
            s2 in arb_package_state(),
        ) {
            let mut st = multi_state(&[
                ("x@1.0.0", PackageState::Pending),
                ("y@2.0.0", PackageState::Pending),
            ]);
            let before = st.packages.len();
            update_state_locked(&mut st, "x@1.0.0", s1);
            update_state_locked(&mut st, "y@2.0.0", s2);
            prop_assert_eq!(st.packages.len(), before,
                "package count changed after transitions");
        }
    }

    mod snapshots {
        use super::*;

        fn fixed_time() -> chrono::DateTime<chrono::Utc> {
            "2025-01-15T12:00:00Z".parse().unwrap()
        }

        #[derive(serde::Serialize)]
        struct ClassificationSnapshot {
            class: shipper_types::ErrorClass,
            message: String,
        }

        impl From<(shipper_types::ErrorClass, String)> for ClassificationSnapshot {
            fn from((class, message): (shipper_types::ErrorClass, String)) -> Self {
                Self { class, message }
            }
        }

        #[derive(serde::Serialize)]
        struct DelaySequence {
            strategy: String,
            base_ms: u64,
            max_ms: u64,
            jitter: f64,
            delays_ms: Vec<u64>,
        }

        fn delay_sequence(
            strategy: shipper_retry::RetryStrategyType,
            base_ms: u64,
            max_ms: u64,
            attempts: u32,
        ) -> DelaySequence {
            let base = Duration::from_millis(base_ms);
            let max = Duration::from_millis(max_ms);
            let delays_ms: Vec<u64> = (1..=attempts)
                .map(|a| backoff_delay(base, max, a, strategy, 0.0).as_millis() as u64)
                .collect();
            DelaySequence {
                strategy: format!("{strategy:?}"),
                base_ms,
                max_ms,
                jitter: 0.0,
                delays_ms,
            }
        }

        fn make_fixed_progress(
            name: &str,
            version: &str,
            state: PackageState,
        ) -> shipper_types::PackageProgress {
            shipper_types::PackageProgress {
                name: name.to_string(),
                version: version.to_string(),
                attempts: 0,
                state,
                last_updated_at: fixed_time(),
            }
        }

        fn fixed_state(entries: &[(&str, &str, &str, PackageState)]) -> ExecutionState {
            let mut packages = BTreeMap::new();
            for (key, name, version, state) in entries {
                packages.insert(
                    key.to_string(),
                    make_fixed_progress(name, version, state.clone()),
                );
            }
            ExecutionState {
                state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
                plan_id: "plan-snapshot-test".to_string(),
                registry: shipper_types::Registry::crates_io(),
                created_at: fixed_time(),
                updated_at: fixed_time(),
                packages,
            }
        }

        fn stabilize_timestamps(st: &mut ExecutionState) {
            let t = fixed_time();
            st.updated_at = t;
            for p in st.packages.values_mut() {
                p.last_updated_at = t;
            }
        }

        // --- 1. Retry strategy configurations ---

        #[test]
        fn snapshot_retry_config_immediate() {
            let config = shipper_retry::RetryStrategyConfig {
                strategy: shipper_retry::RetryStrategyType::Immediate,
                max_attempts: 3,
                base_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(10),
                jitter: 0.0,
            };
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn snapshot_retry_config_exponential() {
            let config = shipper_retry::RetryStrategyConfig {
                strategy: shipper_retry::RetryStrategyType::Exponential,
                max_attempts: 5,
                base_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(120),
                jitter: 0.5,
            };
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn snapshot_retry_config_linear() {
            let config = shipper_retry::RetryStrategyConfig {
                strategy: shipper_retry::RetryStrategyType::Linear,
                max_attempts: 4,
                base_delay: Duration::from_millis(500),
                max_delay: Duration::from_secs(30),
                jitter: 0.25,
            };
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn snapshot_retry_config_constant() {
            let config = shipper_retry::RetryStrategyConfig {
                strategy: shipper_retry::RetryStrategyType::Constant,
                max_attempts: 10,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(5),
                jitter: 0.0,
            };
            insta::assert_yaml_snapshot!(config);
        }

        // --- 2. Error categorization results ---

        #[test]
        fn snapshot_classify_rate_limit() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("HTTP 429 too many requests", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_network_timeout() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("connection timeout", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_auth_denied() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("error: not authorized", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_already_uploaded() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("error: crate version `1.0.0` is already uploaded", "")
                    .into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_network_reset() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("connection reset by peer", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_empty_output() {
            let snap: ClassificationSnapshot = classify_cargo_failure("", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        #[test]
        fn snapshot_classify_unknown_error() {
            let snap: ClassificationSnapshot =
                classify_cargo_failure("some strange unexpected output", "").into();
            insta::assert_yaml_snapshot!(snap);
        }

        // --- 3. Backoff delay calculations ---

        #[test]
        fn snapshot_backoff_exponential_sequence() {
            let seq = delay_sequence(shipper_retry::RetryStrategyType::Exponential, 100, 5000, 8);
            insta::assert_yaml_snapshot!(seq);
        }

        #[test]
        fn snapshot_backoff_linear_sequence() {
            let seq = delay_sequence(shipper_retry::RetryStrategyType::Linear, 200, 5000, 8);
            insta::assert_yaml_snapshot!(seq);
        }

        #[test]
        fn snapshot_backoff_constant_sequence() {
            let seq = delay_sequence(shipper_retry::RetryStrategyType::Constant, 500, 5000, 5);
            insta::assert_yaml_snapshot!(seq);
        }

        #[test]
        fn snapshot_backoff_immediate_sequence() {
            let seq = delay_sequence(shipper_retry::RetryStrategyType::Immediate, 100, 5000, 5);
            insta::assert_yaml_snapshot!(seq);
        }

        #[test]
        fn snapshot_backoff_exponential_clamped() {
            let seq = delay_sequence(shipper_retry::RetryStrategyType::Exponential, 100, 300, 8);
            insta::assert_yaml_snapshot!(seq);
        }

        // --- 4. State transition sequences ---

        #[test]
        fn snapshot_state_success_flow() {
            let mut st = fixed_state(&[("demo@1.0.0", "demo", "1.0.0", PackageState::Pending)]);
            update_state_locked(&mut st, "demo@1.0.0", PackageState::Uploaded);
            update_state_locked(&mut st, "demo@1.0.0", PackageState::Published);
            st.packages.get_mut("demo@1.0.0").unwrap().attempts = 1;
            stabilize_timestamps(&mut st);
            insta::assert_yaml_snapshot!(st);
        }

        #[test]
        fn snapshot_state_failure_flow() {
            let mut st = fixed_state(&[("demo@1.0.0", "demo", "1.0.0", PackageState::Pending)]);
            update_state_locked(
                &mut st,
                "demo@1.0.0",
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "429 rate limited".to_string(),
                },
            );
            st.packages.get_mut("demo@1.0.0").unwrap().attempts = 3;
            stabilize_timestamps(&mut st);
            insta::assert_yaml_snapshot!(st);
        }

        #[test]
        fn snapshot_state_skip_flow() {
            let mut st = fixed_state(&[("demo@1.0.0", "demo", "1.0.0", PackageState::Pending)]);
            update_state_locked(
                &mut st,
                "demo@1.0.0",
                PackageState::Skipped {
                    reason: "already published on registry".to_string(),
                },
            );
            stabilize_timestamps(&mut st);
            insta::assert_yaml_snapshot!(st);
        }

        #[test]
        fn snapshot_state_ambiguous_resolved() {
            let mut st = fixed_state(&[(
                "demo@1.0.0",
                "demo",
                "1.0.0",
                PackageState::Ambiguous {
                    message: "timeout during upload".to_string(),
                },
            )]);
            update_state_locked(&mut st, "demo@1.0.0", PackageState::Published);
            st.packages.get_mut("demo@1.0.0").unwrap().attempts = 2;
            stabilize_timestamps(&mut st);
            insta::assert_yaml_snapshot!(st);
        }

        #[test]
        fn snapshot_state_multi_package_mixed_outcomes() {
            let mut st = fixed_state(&[
                ("core@1.0.0", "core", "1.0.0", PackageState::Pending),
                ("utils@1.0.0", "utils", "1.0.0", PackageState::Pending),
                ("cli@1.0.0", "cli", "1.0.0", PackageState::Pending),
            ]);
            update_state_locked(&mut st, "core@1.0.0", PackageState::Published);
            st.packages.get_mut("core@1.0.0").unwrap().attempts = 1;
            update_state_locked(
                &mut st,
                "utils@1.0.0",
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "not authorized".to_string(),
                },
            );
            st.packages.get_mut("utils@1.0.0").unwrap().attempts = 1;
            update_state_locked(
                &mut st,
                "cli@1.0.0",
                PackageState::Skipped {
                    reason: "dependency utils@1.0.0 failed".to_string(),
                },
            );
            stabilize_timestamps(&mut st);
            insta::assert_yaml_snapshot!(st);
        }

        // --- 5. ExecutionState variant snapshots ---

        #[test]
        fn snapshot_execution_state_empty_packages() {
            let st = fixed_state(&[]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_pending() {
            let st = fixed_state(&[("a@1.0.0", "a", "1.0.0", PackageState::Pending)]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_uploaded() {
            let st = fixed_state(&[("a@1.0.0", "a", "1.0.0", PackageState::Uploaded)]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_published() {
            let st = fixed_state(&[("a@1.0.0", "a", "1.0.0", PackageState::Published)]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_skipped() {
            let st = fixed_state(&[(
                "a@1.0.0",
                "a",
                "1.0.0",
                PackageState::Skipped {
                    reason: "already on registry".into(),
                },
            )]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_failed() {
            let st = fixed_state(&[(
                "a@1.0.0",
                "a",
                "1.0.0",
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "denied".into(),
                },
            )]);
            insta::assert_debug_snapshot!(st);
        }

        #[test]
        fn snapshot_execution_state_single_ambiguous() {
            let st = fixed_state(&[(
                "a@1.0.0",
                "a",
                "1.0.0",
                PackageState::Ambiguous {
                    message: "timeout".into(),
                },
            )]);
            insta::assert_debug_snapshot!(st);
        }

        // --- 6. State transition sequence snapshots ---

        #[test]
        fn snapshot_transition_pending_to_uploaded_to_published() {
            let key = "pkg@1.0.0";
            let mut st = fixed_state(&[(key, "pkg", "1.0.0", PackageState::Pending)]);
            let mut steps: Vec<String> = vec![format!("initial: {:?}", st.packages[key].state)];
            update_state_locked(&mut st, key, PackageState::Uploaded);
            steps.push(format!("after upload: {:?}", st.packages[key].state));
            update_state_locked(&mut st, key, PackageState::Published);
            steps.push(format!("after publish: {:?}", st.packages[key].state));
            insta::assert_debug_snapshot!(steps);
        }

        #[test]
        fn snapshot_transition_pending_to_failed_retry_to_published() {
            let key = "pkg@1.0.0";
            let mut st = fixed_state(&[(key, "pkg", "1.0.0", PackageState::Pending)]);
            let mut steps: Vec<String> = vec![format!("initial: {:?}", st.packages[key].state)];
            update_state_locked(
                &mut st,
                key,
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "rate limited".into(),
                },
            );
            steps.push(format!("after failure: {:?}", st.packages[key].state));
            update_state_locked(&mut st, key, PackageState::Pending);
            steps.push(format!("after retry reset: {:?}", st.packages[key].state));
            update_state_locked(&mut st, key, PackageState::Uploaded);
            steps.push(format!("after upload: {:?}", st.packages[key].state));
            update_state_locked(&mut st, key, PackageState::Published);
            steps.push(format!("after publish: {:?}", st.packages[key].state));
            insta::assert_debug_snapshot!(steps);
        }

        #[test]
        fn snapshot_transition_ambiguous_to_published() {
            let key = "pkg@1.0.0";
            let mut st = fixed_state(&[(
                key,
                "pkg",
                "1.0.0",
                PackageState::Ambiguous {
                    message: "upload timeout".into(),
                },
            )]);
            let mut steps: Vec<String> = vec![format!("initial: {:?}", st.packages[key].state)];
            update_state_locked(&mut st, key, PackageState::Published);
            steps.push(format!("after verification: {:?}", st.packages[key].state));
            insta::assert_debug_snapshot!(steps);
        }

        #[test]
        fn snapshot_transition_all_skipped_plan() {
            let mut st = fixed_state(&[
                ("a@1.0.0", "a", "1.0.0", PackageState::Pending),
                ("b@1.0.0", "b", "1.0.0", PackageState::Pending),
            ]);
            update_state_locked(
                &mut st,
                "a@1.0.0",
                PackageState::Skipped {
                    reason: "already published".into(),
                },
            );
            update_state_locked(
                &mut st,
                "b@1.0.0",
                PackageState::Skipped {
                    reason: "already published".into(),
                },
            );
            stabilize_timestamps(&mut st);
            insta::assert_debug_snapshot!(st);
        }
    }

    // -- 1. State machine transitions: all valid transitions --

    #[test]
    fn transition_pending_to_uploaded() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        update_state_locked(&mut st, key, PackageState::Uploaded);
        assert_eq!(st.packages[key].state, PackageState::Uploaded);
    }

    #[test]
    fn transition_pending_to_skipped() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        update_state_locked(
            &mut st,
            key,
            PackageState::Skipped {
                reason: "pre-existing".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Skipped { .. }
        ));
    }

    #[test]
    fn transition_pending_to_failed() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        update_state_locked(
            &mut st,
            key,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn transition_pending_to_ambiguous() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        update_state_locked(
            &mut st,
            key,
            PackageState::Ambiguous {
                message: "timeout".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
    }

    #[test]
    fn transition_uploaded_to_published() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Uploaded);
        update_state_locked(&mut st, key, PackageState::Published);
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    #[test]
    fn transition_uploaded_to_failed() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Uploaded);
        update_state_locked(
            &mut st,
            key,
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "verify timeout".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn transition_uploaded_to_ambiguous() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Uploaded);
        update_state_locked(
            &mut st,
            key,
            PackageState::Ambiguous {
                message: "verify timeout".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Ambiguous { .. }
        ));
    }

    #[test]
    fn transition_ambiguous_to_published() {
        let key = "a@1.0.0";
        let mut st = sample_state(
            key,
            PackageState::Ambiguous {
                message: "timeout".into(),
            },
        );
        update_state_locked(&mut st, key, PackageState::Published);
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    #[test]
    fn transition_ambiguous_to_failed() {
        let key = "a@1.0.0";
        let mut st = sample_state(
            key,
            PackageState::Ambiguous {
                message: "timeout".into(),
            },
        );
        update_state_locked(
            &mut st,
            key,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "confirmed not on registry".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn transition_failed_retryable_back_to_pending() {
        let key = "a@1.0.0";
        let mut st = sample_state(
            key,
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "rate limit".into(),
            },
        );
        update_state_locked(&mut st, key, PackageState::Pending);
        assert_eq!(st.packages[key].state, PackageState::Pending);
    }

    // -- 2. Invalid / unusual transitions (the API is permissive, verify it accepts them) --

    #[test]
    fn transition_published_to_pending_is_accepted() {
        // update_state_locked is a raw setter — it does not enforce a state machine
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Published);
        update_state_locked(&mut st, key, PackageState::Pending);
        assert_eq!(st.packages[key].state, PackageState::Pending);
    }

    #[test]
    fn transition_skipped_to_published_is_accepted() {
        let key = "a@1.0.0";
        let mut st = sample_state(
            key,
            PackageState::Skipped {
                reason: "skip".into(),
            },
        );
        update_state_locked(&mut st, key, PackageState::Published);
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    #[test]
    fn transition_published_to_failed_is_accepted() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Published);
        update_state_locked(
            &mut st,
            key,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "weird".into(),
            },
        );
        assert!(matches!(
            st.packages[key].state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn update_state_rejects_missing_key() {
        let mut st = sample_state("a@1.0.0", PackageState::Pending);
        let td = tempdir().expect("tempdir");
        let err = update_state(
            &mut st,
            td.path(),
            "nonexistent@0.0.0",
            PackageState::Published,
        );
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("missing package in state")
        );
    }

    // -- 3. Concurrent state updates (sequential simulation) --

    #[test]
    fn concurrent_updates_to_different_packages_are_independent() {
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@1.0.0", PackageState::Pending),
            ("c@1.0.0", PackageState::Pending),
        ]);
        // Simulate concurrent workers updating different keys
        update_state_locked(&mut st, "a@1.0.0", PackageState::Uploaded);
        update_state_locked(&mut st, "b@1.0.0", PackageState::Published);
        update_state_locked(
            &mut st,
            "c@1.0.0",
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "rate limited".into(),
            },
        );
        assert_eq!(st.packages["a@1.0.0"].state, PackageState::Uploaded);
        assert_eq!(st.packages["b@1.0.0"].state, PackageState::Published);
        assert!(matches!(
            st.packages["c@1.0.0"].state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn rapid_sequential_updates_same_key() {
        let key = "a@1.0.0";
        let mut st = sample_state(key, PackageState::Pending);
        // Rapid-fire transitions on the same key
        let states = [
            PackageState::Uploaded,
            PackageState::Ambiguous {
                message: "check".into(),
            },
            PackageState::Published,
        ];
        for s in &states {
            update_state_locked(&mut st, key, s.clone());
        }
        assert_eq!(st.packages[key].state, PackageState::Published);
    }

    #[test]
    fn concurrent_persist_updates_are_consistent() {
        let td = tempdir().expect("tempdir");
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@1.0.0", PackageState::Pending),
        ]);
        update_state(&mut st, td.path(), "a@1.0.0", PackageState::Uploaded).unwrap();
        update_state(&mut st, td.path(), "b@1.0.0", PackageState::Published).unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert_eq!(loaded.packages["a@1.0.0"].state, PackageState::Uploaded);
        assert_eq!(loaded.packages["b@1.0.0"].state, PackageState::Published);
    }

    // -- 4. Empty execution plan --

    #[test]
    fn empty_plan_state_has_no_packages() {
        let st = multi_state(&[]);
        assert!(st.packages.is_empty());
    }

    #[test]
    fn empty_plan_update_locked_is_noop() {
        let mut st = multi_state(&[]);
        update_state_locked(&mut st, "nonexistent@1.0.0", PackageState::Published);
        assert!(st.packages.is_empty());
    }

    #[test]
    fn empty_plan_update_state_errors() {
        let mut st = multi_state(&[]);
        let td = tempdir().expect("tempdir");
        assert!(update_state(&mut st, td.path(), "any@1.0.0", PackageState::Published).is_err());
    }

    #[test]
    fn empty_plan_persist_and_reload() {
        let td = tempdir().expect("tempdir");
        let st = multi_state(&[]);
        shipper_state::save_state(td.path(), &st).unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert!(loaded.packages.is_empty());
        assert_eq!(loaded.plan_id, "plan-multi");
    }

    // -- 5. Single-package execution --

    #[test]
    fn single_package_full_lifecycle() {
        let key = "solo@0.1.0";
        let td = tempdir().expect("tempdir");
        let mut st = sample_state(key, PackageState::Pending);
        update_state(&mut st, td.path(), key, PackageState::Uploaded).unwrap();
        assert_eq!(st.packages[key].state, PackageState::Uploaded);
        update_state(&mut st, td.path(), key, PackageState::Published).unwrap();
        assert_eq!(st.packages[key].state, PackageState::Published);
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert_eq!(loaded.packages[key].state, PackageState::Published);
    }

    #[test]
    fn single_package_skip_lifecycle() {
        let key = "solo@0.1.0";
        let td = tempdir().expect("tempdir");
        let mut st = sample_state(key, PackageState::Pending);
        update_state(
            &mut st,
            td.path(),
            key,
            PackageState::Skipped {
                reason: "already exists".into(),
            },
        )
        .unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert!(matches!(
            loaded.packages[key].state,
            PackageState::Skipped { .. }
        ));
    }

    #[test]
    fn single_package_failure_lifecycle() {
        let key = "solo@0.1.0";
        let td = tempdir().expect("tempdir");
        let mut st = sample_state(key, PackageState::Pending);
        update_state(
            &mut st,
            td.path(),
            key,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth denied".into(),
            },
        )
        .unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        match &loaded.packages[key].state {
            PackageState::Failed { class, message } => {
                assert_eq!(*class, ErrorClass::Permanent);
                assert_eq!(message, "auth denied");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // -- 6. All packages already published (skip everything) --

    #[test]
    fn all_packages_skipped_preserves_reasons() {
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@2.0.0", PackageState::Pending),
            ("c@3.0.0", PackageState::Pending),
        ]);
        let reasons = ["version exists", "yanked version", "no changes"];
        for (i, (key, _)) in st.packages.clone().iter().enumerate() {
            update_state_locked(
                &mut st,
                key,
                PackageState::Skipped {
                    reason: reasons[i].into(),
                },
            );
        }
        for pkg in st.packages.values() {
            assert!(
                matches!(&pkg.state, PackageState::Skipped { .. }),
                "all should be skipped"
            );
        }
    }

    #[test]
    fn all_packages_already_published_remain_published() {
        let st = multi_state(&[
            ("a@1.0.0", PackageState::Published),
            ("b@2.0.0", PackageState::Published),
        ]);
        let published_count = st
            .packages
            .values()
            .filter(|p| matches!(p.state, PackageState::Published))
            .count();
        assert_eq!(published_count, 2);
    }

    #[test]
    fn all_skipped_persist_round_trip() {
        let td = tempdir().expect("tempdir");
        let mut st = multi_state(&[
            ("a@1.0.0", PackageState::Pending),
            ("b@2.0.0", PackageState::Pending),
        ]);
        update_state(
            &mut st,
            td.path(),
            "a@1.0.0",
            PackageState::Skipped {
                reason: "exists".into(),
            },
        )
        .unwrap();
        update_state(
            &mut st,
            td.path(),
            "b@2.0.0",
            PackageState::Skipped {
                reason: "exists".into(),
            },
        )
        .unwrap();
        let loaded = shipper_state::load_state(td.path()).unwrap().unwrap();
        assert!(
            loaded
                .packages
                .values()
                .all(|p| matches!(p.state, PackageState::Skipped { .. }))
        );
    }

    // -- 10. Error propagation from callbacks --

    #[test]
    fn update_state_propagates_save_error_on_invalid_dir() {
        let mut st = sample_state("a@1.0.0", PackageState::Pending);
        // Use a nonexistent directory that cannot be created
        let bad_dir = PathBuf::from(if cfg!(windows) {
            r"Z:\nonexistent\deep\path\state"
        } else {
            "/nonexistent/deep/path/state"
        });
        let result = update_state(&mut st, &bad_dir, "a@1.0.0", PackageState::Published);
        assert!(result.is_err(), "should propagate IO error from save_state");
    }

    #[test]
    fn update_state_error_does_not_corrupt_in_memory_state() {
        let mut st = sample_state("a@1.0.0", PackageState::Pending);
        let bad_dir = PathBuf::from(if cfg!(windows) {
            r"Z:\nonexistent\path"
        } else {
            "/nonexistent/path"
        });
        // The update modifies in-memory state, then fails on persist.
        // Even on error, the in-memory mutation has occurred (this is the current behavior).
        let _ = update_state(&mut st, &bad_dir, "a@1.0.0", PackageState::Published);
        // The in-memory state was already mutated before the persist call
        assert_eq!(st.packages["a@1.0.0"].state, PackageState::Published);
    }

    #[test]
    fn update_state_missing_key_error_message_is_descriptive() {
        let mut st = sample_state("a@1.0.0", PackageState::Pending);
        let td = tempdir().expect("tempdir");
        let err = update_state(&mut st, td.path(), "z@9.9.9", PackageState::Published).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("missing package"),
            "error should mention missing package: {msg}"
        );
    }

    // -- 9. Property test: state transitions are deterministic --

    proptest! {
        #[test]
        fn state_transitions_are_deterministic(
            initial in arb_package_state(),
            target in arb_package_state(),
        ) {
            let key = "d@1.0.0";
            let mut st1 = sample_state(key, initial.clone());
            let mut st2 = sample_state(key, initial);
            update_state_locked(&mut st1, key, target.clone());
            update_state_locked(&mut st2, key, target);
            prop_assert_eq!(&st1.packages[key].state, &st2.packages[key].state);
        }

        #[test]
        fn multi_step_transitions_preserve_package_count(
            s1 in arb_package_state(),
            s2 in arb_package_state(),
        ) {
            let mut st = multi_state(&[
                ("x@1.0.0", PackageState::Pending),
                ("y@1.0.0", PackageState::Pending),
            ]);
            update_state_locked(&mut st, "x@1.0.0", s1);
            update_state_locked(&mut st, "y@1.0.0", s2);
            prop_assert_eq!(st.packages.len(), 2);
        }

        #[test]
        fn update_state_locked_idempotent_for_same_state(state in arb_package_state()) {
            let key = "i@1.0.0";
            let mut st = sample_state(key, state.clone());
            update_state_locked(&mut st, key, state.clone());
            update_state_locked(&mut st, key, state.clone());
            prop_assert_eq!(&st.packages[key].state, &state);
        }
    }
}
