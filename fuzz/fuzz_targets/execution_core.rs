#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::runtime::execution;
use shipper_retry::RetryStrategyType;
use shipper_types::{ExecutionState, PackageProgress, PackageState, Registry};

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;

fuzz_target!(|input: (Vec<u8>, Vec<u8>, u8, bool)| {
    let (name_bytes, version_bytes, attempt, use_linear) = input;
    let name = String::from_utf8_lossy(&name_bytes).to_string();
    let version = String::from_utf8_lossy(&version_bytes).to_string();

    let key = execution::pkg_key(&name, &version);
    assert!(!key.is_empty() || name.is_empty() && version.is_empty());

    let strategy = if use_linear {
        RetryStrategyType::Linear
    } else {
        RetryStrategyType::Exponential
    };
    let base = Duration::from_millis(u64::from(attempt) * 11);
    let max = Duration::from_millis(u64::from(attempt).saturating_mul(23).saturating_add(1));
    let _ = execution::backoff_delay(base, max.max(base), u32::from(attempt), strategy, 0.15);

    let workspace_root = PathBuf::from("workspace-root");
    let rel = execution::resolve_state_dir(&workspace_root, &PathBuf::from(".shipper"));
    assert!(!rel.as_os_str().is_empty());

    let mut st = ExecutionState {
        state_version: "shipper.state.v1".to_string(),
        plan_id: "fuzz-plan".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::from([(
            key.clone(),
            PackageProgress {
                name,
                version,
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        )]),
    };

    execution::update_state_locked(&mut st, &key, PackageState::Uploaded);
    assert!(matches!(
        st.packages.get(&key).expect("pkg").state,
        PackageState::Uploaded
    ));

    let _ = execution::classify_cargo_failure(
        "warning: temporary network issue",
        "error: 429 too many requests",
    );
    let _ = execution::short_state(&PackageState::Published);
    let _ = execution::short_state(&PackageState::Skipped {
        reason: "already".to_string(),
    });
});
