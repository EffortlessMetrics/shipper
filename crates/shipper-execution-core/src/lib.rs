//! Shared execution helpers for publish workflows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use shipper_cargo_failure;
use shipper_retry::{calculate_delay, RetryStrategyConfig, RetryStrategyType};
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

    fn make_progress(name: &str, version: &str, state: PackageState) -> shipper_types::PackageProgress {
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
        assert_eq!(short_state(&shipper_types::PackageState::Pending), "pending");
        assert_eq!(short_state(&shipper_types::PackageState::Uploaded), "uploaded");
        assert_eq!(short_state(&shipper_types::PackageState::Published), "published");
        assert_eq!(short_state(&shipper_types::PackageState::Skipped { reason: "x".into() }), "skipped");
        assert_eq!(short_state(&shipper_types::PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "x".into()
        }), "failed");
        assert_eq!(short_state(&shipper_types::PackageState::Ambiguous { message: "x".into() }), "ambiguous");
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
        let loaded = shipper_state::load_state(state_dir).expect("load state").expect("state exists");
        assert!(matches!(
            loaded
                .packages
                .get("demo@0.1.0")
                .expect("pkg")
                .state,
            shipper_types::PackageState::Uploaded
        ));
    }

    #[test]
    fn update_state_fails_for_missing_package() {
        let mut st = sample_state("demo@0.1.0", shipper_types::PackageState::Pending);
        let td = tempdir().expect("tempdir");
        assert!(update_state(
            &mut st,
            td.path(),
            "missing",
            shipper_types::PackageState::Uploaded,
        )
        .is_err());
    }

    #[test]
    fn update_state_locked_is_noop_for_missing_package() {
        let mut st = sample_state("demo@0.1.0", shipper_types::PackageState::Pending);
        let before = st.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        update_state_locked(&mut st, "missing", shipper_types::PackageState::Published);
        assert_eq!(
            st.packages
                .get("demo@0.1.0")
                .expect("pkg")
                .state,
            shipper_types::PackageState::Pending
        );
        assert!(st.updated_at >= before);
    }

    #[test]
    fn backoff_delay_is_bounded_with_jitter() {
        let base = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_millis(500);
        let d1 = backoff_delay(base, max, 1, shipper_retry::RetryStrategyType::Exponential, 0.5);
        let d20 = backoff_delay(base, max, 20, shipper_retry::RetryStrategyType::Exponential, 0.5);

        assert!(d1 >= std::time::Duration::from_millis(50));
        assert!(d1 <= std::time::Duration::from_millis(150));
        assert!(d20 >= std::time::Duration::from_millis(250));
        assert!(d20 <= std::time::Duration::from_millis(750));
    }

    fn ascii_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(any::<char>(), 0..128)
            .prop_map(|chars| chars.into_iter().collect())
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
            prop_assert_eq!(lower.class, upper.class);
        }
    }
}
