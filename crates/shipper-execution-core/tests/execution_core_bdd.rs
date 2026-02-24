use chrono::Utc;
use std::collections::BTreeMap;

use shipper_execution_core as execution_core;
use shipper_types::{ExecutionState, PackageProgress, PackageState, Registry};

#[test]
fn bdd_given_existing_pending_package_when_state_is_updated_and_persisted_then_state_is_written() {
    let key = execution_core::pkg_key("demo", "0.1.0");
    let mut st = ExecutionState {
        state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "plan-bdd".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::from([(
            key.clone(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        )]),
    };

    let temp_dir = tempfile::tempdir().expect("tempdir");

    execution_core::update_state(&mut st, temp_dir.path(), &key, PackageState::Uploaded)
        .expect("persist state");

    let loaded = shipper_state::load_state(temp_dir.path())
        .expect("load state")
        .expect("state exists");
    assert!(matches!(
        loaded.packages.get(&key).expect("pkg").state,
        PackageState::Uploaded
    ));
}

#[test]
fn bdd_given_in_memory_update_then_state_uses_key_lookup_contract() {
    let key = execution_core::pkg_key("demo", "0.1.0");
    let mut st = ExecutionState {
        state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "plan-bdd".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages: BTreeMap::from([(
            key.clone(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        )]),
    };

    execution_core::update_state_locked(
        &mut st,
        &key,
        PackageState::Failed {
            class: shipper_types::ErrorClass::Ambiguous,
            message: "registry check ambiguous".to_string(),
        },
    );

    assert!(matches!(
        st.packages.get(&key).expect("pkg").state,
        PackageState::Failed { .. }
    ));
}
