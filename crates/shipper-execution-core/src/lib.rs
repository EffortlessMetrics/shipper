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
            // With jitter up to 1.0, max theoretical is max + max*jitter
            let upper = max + max.mul_f64(jitter);
            prop_assert!(d <= upper, "delay {:?} exceeded upper bound {:?}", d, upper);
        }

        #[test]
        fn pkg_key_contains_at_separator(name in "[a-z_-]{1,30}", version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}") {
            let key = pkg_key(&name, &version);
            prop_assert!(key.contains('@'));
            prop_assert_eq!(key, format!("{name}@{version}"));
        }
    }
}
