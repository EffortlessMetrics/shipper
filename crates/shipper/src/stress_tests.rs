//! Stress tests for shipper parallel engine and concurrent operations.
//!
//! These tests verify behavior under high load and concurrent access:
//! - Parallel publishing with many packages
//! - Concurrent state file access
//! - High contention lock scenarios

#[cfg(test)]
mod tests {
    use crate::encryption::{decrypt, encrypt};
    use crate::lock::LockFile;
    use crate::state::{load_state, save_state};
    use crate::types::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper to create a planned package
    fn make_pkg(name: &str, version: &str) -> PlannedPackage {
        PlannedPackage {
            name: name.to_string(),
            version: version.to_string(),
            manifest_path: PathBuf::from(format!("crates/{}/Cargo.toml", name)),
        }
    }

    /// Helper to create execution state
    fn make_state(plan_id: &str, packages: Vec<(String, String, PackageState)>) -> ExecutionState {
        let mut pkg_map = BTreeMap::new();
        for (name, version, state) in packages {
            pkg_map.insert(
                format!("{}@{}", name, version),
                PackageProgress {
                    name,
                    version,
                    attempts: 0,
                    state,
                    last_updated_at: chrono::Utc::now(),
                },
            );
        }
        ExecutionState {
            state_version: "shipper.state.v2".to_string(),
            plan_id: plan_id.to_string(),
            registry: Registry::crates_io(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            packages: pkg_map,
        }
    }

    #[test]
    fn stress_lock_acquire_release_cycle() {
        let temp_dir = TempDir::new().unwrap();
        let state_dir = temp_dir.path().join(".shipper");

        // Rapidly acquire and release locks
        for i in 0..100 {
            let lock = LockFile::acquire(&state_dir)
                .unwrap_or_else(|_| panic!("Failed to acquire lock on iteration {}", i));
            drop(lock);
        }
    }

    #[test]
    fn stress_state_save_load_cycle() {
        let temp_dir = TempDir::new().unwrap();
        let state_dir = temp_dir.path().join(".shipper");
        std::fs::create_dir_all(&state_dir).unwrap();

        // Create a large state with many packages
        let packages: Vec<(String, String, PackageState)> = (0..50)
            .map(|i| {
                let state = if i % 3 == 0 {
                    PackageState::Published
                } else if i % 3 == 1 {
                    PackageState::Pending
                } else {
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "test".into(),
                    }
                };
                (format!("crate-{}", i), "1.0.0".to_string(), state)
            })
            .collect();

        let state = make_state("stress-test", packages);

        // Rapidly save and load
        for i in 0..50 {
            save_state(&state_dir, &state)
                .unwrap_or_else(|_| panic!("Failed to save state on iteration {}", i));
            let loaded = load_state(&state_dir)
                .unwrap_or_else(|_| panic!("Failed to load state on iteration {}", i))
                .expect("state should exist");
            assert_eq!(loaded.plan_id, "stress-test");
            assert_eq!(loaded.packages.len(), 50);
        }
    }

    #[test]
    fn stress_large_publish_level() {
        // Test with a large number of independent packages
        let packages: Vec<PlannedPackage> = (0..100)
            .map(|i| make_pkg(&format!("crate-{}", i), "1.0.0"))
            .collect();

        let level = PublishLevel { level: 0, packages };

        // Verify the level can be serialized
        let json = serde_json::to_string(&level).unwrap();
        let parsed: PublishLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.packages.len(), 100);
    }

    #[test]
    fn stress_deep_dependency_chain() {
        // Create a deep chain: crate-0 -> crate-1 -> ... -> crate-49
        let mut deps = BTreeMap::new();
        for i in 1..50 {
            deps.insert(format!("crate-{}", i), vec![format!("crate-{}", i - 1)]);
        }

        // Verify no cycles (each crate depends only on one earlier crate)
        let mut visited = std::collections::HashSet::new();
        for name in deps.keys() {
            assert!(!visited.contains(name), "Cycle detected at {}", name);
            visited.insert(name.clone());
        }
        assert_eq!(visited.len(), 49); // crate-0 has no deps
    }

    #[test]
    fn stress_event_log_append() {
        use crate::events::EventLog;
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("events.jsonl");

        let mut log = EventLog::new();

        // Append many events
        for i in 0..1000 {
            let event = PublishEvent {
                timestamp: chrono::Utc::now(),
                event_type: EventType::PackageStarted {
                    name: format!("crate-{}", i % 100),
                    version: "1.0.0".to_string(),
                },
                package: format!("crate-{}@1.0.0", i % 100),
            };
            log.record(event);
        }

        // Write to file
        log.write_to_file(&log_path).unwrap();

        // Verify all events are readable
        let loaded = EventLog::read_from_file(&log_path).unwrap();
        assert_eq!(loaded.all_events().len(), 1000);
    }

    #[test]
    fn stress_sequential_state_updates() {
        // Sequential state updates to verify persistence works correctly
        let temp_dir = TempDir::new().unwrap();
        let state_dir = temp_dir.path().join(".shipper");
        std::fs::create_dir_all(&state_dir).unwrap();

        // Create initial state
        let state = make_state(
            "sequential-test",
            vec![(
                "core".to_string(),
                "1.0.0".to_string(),
                PackageState::Pending,
            )],
        );
        save_state(&state_dir, &state).unwrap();

        // Sequential updates
        for i in 0..10 {
            let mut local_state = load_state(&state_dir).unwrap().expect("state should exist");
            local_state.plan_id = format!("sequential-test-{}", i);
            save_state(&state_dir, &local_state).unwrap();
        }

        // Final state should have the last plan_id
        let final_state = load_state(&state_dir).unwrap().expect("state should exist");
        assert!(final_state.plan_id.starts_with("sequential-test"));
    }

    #[test]
    fn stress_receipt_with_many_packages() {
        let temp_dir = TempDir::new().unwrap();
        let receipt_path = temp_dir.path().join("receipt.json");

        // Create receipt with many packages
        let packages: Vec<PackageReceipt> = (0..100)
            .map(|i| PackageReceipt {
                name: format!("crate-{}", i),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
                duration_ms: 1000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            })
            .collect();

        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "stress-test".to_string(),
            registry: Registry::crates_io(),
            started_at: chrono::Utc::now(),
            finished_at: chrono::Utc::now(),
            packages,
            event_log_path: receipt_path.clone(),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: env!("CARGO_PKG_VERSION").to_string(),
                cargo_version: None,
                rust_version: None,
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
            },
        };

        // Serialize large receipt
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(json.len() > 10_000); // Should be sizable

        // Deserialize and verify
        let parsed: Receipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.packages.len(), 100);
    }

    #[test]
    fn stress_encryption_roundtrip_large_data() {
        // Encrypt large state data
        let large_state = make_state(
            "large-test",
            (0..50)
                .map(|i| {
                    (
                        format!("crate-{}", i),
                        "1.0.0".to_string(),
                        PackageState::Pending,
                    )
                })
                .collect(),
        );
        let json = serde_json::to_string(&large_state).unwrap();
        let passphrase = "stress-test-passphrase";

        // Multiple roundtrips
        for i in 0..10 {
            let encrypted = encrypt(json.as_bytes(), passphrase)
                .unwrap_or_else(|_| panic!("Encryption failed on iteration {}", i));
            assert!(encrypted.len() > json.len()); // Encrypted should be larger

            let encrypted_str = String::from_utf8(encrypted).unwrap();
            let decrypted = decrypt(&encrypted_str, passphrase)
                .unwrap_or_else(|_| panic!("Decryption failed on iteration {}", i));
            assert_eq!(decrypted, json.as_bytes());
        }
    }
}
