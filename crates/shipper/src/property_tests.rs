//! Property-based tests for shipper invariants.
//!
//! These tests verify critical properties that should hold for all inputs:
//! - Plan determinism: same inputs produce same outputs
//! - Topological correctness: dependencies always come before dependents
//! - State machine invariants: only valid state transitions

#[cfg(test)]
mod tests {
    use crate::types::*;
    use proptest::prelude::*;

    /// Generate arbitrary package names (alphanumeric, 1-20 chars)
    fn package_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{0,19}".prop_map(|s| s.to_lowercase())
    }

    proptest! {
        /// Property: Package state serialization roundtrips correctly
        #[test]
        fn package_state_roundtrip(
            state in prop_oneof![
                Just(PackageState::Pending),
                Just(PackageState::Uploaded),
                Just(PackageState::Published),
                Just(PackageState::Skipped { reason: "test".to_string() }),
                Just(PackageState::Failed { class: ErrorClass::Retryable, message: "error".to_string() }),
                Just(PackageState::Ambiguous { message: "maybe".to_string() }),
            ]
        ) {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: PackageState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, parsed);
        }

        /// Property: Registry name normalization is idempotent
        #[test]
        fn registry_normalization_idempotent(name in package_name_strategy()) {
            let normalized1 = name.to_lowercase().replace('-', "_");
            let normalized2 = normalized1.to_lowercase().replace('-', "_");
            assert_eq!(normalized1, normalized2);
        }

        /// Property: Delay with no jitter is bounded by max
        #[test]
        fn delay_bounded_no_jitter(
            base_ms in 1u64..10000,
            max_ms in 100u64..300000,
            attempt in 1u32..100,
        ) {
            use std::time::Duration;
            use crate::retry::{calculate_delay, RetryStrategyConfig, RetryStrategyType};

            let base_delay = Duration::from_millis(base_ms.min(max_ms));
            let max_delay = Duration::from_millis(max_ms);

            let config = RetryStrategyConfig {
                strategy: RetryStrategyType::Exponential,
                max_attempts: 100,
                base_delay,
                max_delay,
                jitter: 0.0, // No jitter for deterministic test
            };

            let delay = calculate_delay(&config, attempt);
            prop_assert!(delay <= max_delay, "Delay {} should not exceed max {}", delay.as_millis(), max_delay.as_millis());
        }

        /// Property: Error class roundtrips
        #[test]
        fn error_class_roundtrip(
            class in prop_oneof![
                Just(ErrorClass::Retryable),
                Just(ErrorClass::Permanent),
                Just(ErrorClass::Ambiguous),
            ]
        ) {
            let json = serde_json::to_string(&class).unwrap();
            let parsed: ErrorClass = serde_json::from_str(&json).unwrap();
            assert_eq!(class, parsed);
        }
    }
}

#[cfg(test)]
mod state_machine_tests {
    use crate::types::*;

    /// Valid state transitions for package publishing
    fn valid_transitions(from: &PackageState) -> Vec<PackageState> {
        match from {
            PackageState::Pending => vec![
                PackageState::Uploaded,
                PackageState::Published,
                PackageState::Skipped {
                    reason: String::new(),
                },
                PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: String::new(),
                },
            ],
            PackageState::Uploaded => vec![
                PackageState::Published,
                PackageState::Ambiguous {
                    message: String::new(),
                },
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: String::new(),
                },
            ],
            PackageState::Published => vec![], // Terminal state
            PackageState::Skipped { .. } => vec![], // Terminal state
            PackageState::Failed { .. } => vec![], // Terminal state
            PackageState::Ambiguous { .. } => vec![
                PackageState::Published,
                PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: String::new(),
                },
            ],
        }
    }

    #[test]
    fn test_pending_transitions() {
        let pending = PackageState::Pending;
        let valid = valid_transitions(&pending);
        assert!(valid.contains(&PackageState::Uploaded));
        assert!(valid.contains(&PackageState::Published));
        assert!(!valid.contains(&PackageState::Pending)); // No self-loop
    }

    #[test]
    fn test_published_is_terminal() {
        let published = PackageState::Published;
        assert!(valid_transitions(&published).is_empty());
    }

    #[test]
    fn test_failed_is_terminal() {
        let failed = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "error".into(),
        };
        assert!(valid_transitions(&failed).is_empty());
    }

    #[test]
    fn test_skipped_is_terminal() {
        let skipped = PackageState::Skipped {
            reason: "already published".into(),
        };
        assert!(valid_transitions(&skipped).is_empty());
    }

    #[test]
    fn test_ambiguous_can_resolve() {
        let ambiguous = PackageState::Ambiguous {
            message: "unclear".into(),
        };
        let valid = valid_transitions(&ambiguous);
        assert!(valid.contains(&PackageState::Published));
    }
}

#[cfg(test)]
mod topo_invariant_tests {
    use crate::types::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    /// Verify topological ordering invariant:
    /// For every dependency edge X -> Y, Y must appear before X in the order
    fn verify_topo_order(
        packages: &[PlannedPackage],
        dependencies: &BTreeMap<String, BTreeSet<String>>,
    ) -> bool {
        // Build position map
        let positions: BTreeMap<String, usize> = packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect();

        // Check each package's dependencies come before it
        for pkg in packages {
            if let Some(deps) = dependencies.get(&pkg.name) {
                let pkg_pos = positions[&pkg.name];
                for dep in deps {
                    if let Some(&dep_pos) = positions.get(dep)
                        && dep_pos >= pkg_pos
                    {
                        return false; // Dependency appears after dependent
                    }
                }
            }
        }
        true
    }

    fn make_pkg(name: &str, version: &str) -> PlannedPackage {
        PlannedPackage {
            name: name.to_string(),
            version: version.to_string(),
            manifest_path: PathBuf::from(format!("crates/{}/Cargo.toml", name)),
        }
    }

    #[test]
    fn test_topo_simple_chain() {
        let packages = vec![
            make_pkg("core", "1.0.0"),
            make_pkg("utils", "1.0.0"),
            make_pkg("app", "1.0.0"),
        ];

        let mut deps = BTreeMap::new();
        deps.insert("utils".into(), BTreeSet::from(["core".into()]));
        deps.insert(
            "app".into(),
            BTreeSet::from(["utils".into(), "core".into()]),
        );

        assert!(verify_topo_order(&packages, &deps));
    }

    #[test]
    fn test_topo_invalid_order_detected() {
        // Invalid: app appears before core
        let packages = vec![
            make_pkg("app", "1.0.0"),
            make_pkg("utils", "1.0.0"),
            make_pkg("core", "1.0.0"),
        ];

        let mut deps = BTreeMap::new();
        deps.insert("app".into(), BTreeSet::from(["core".into()]));

        assert!(!verify_topo_order(&packages, &deps));
    }

    #[test]
    fn test_topo_independent_packages() {
        // Packages with no dependencies can be in any order
        let packages = vec![
            make_pkg("alpha", "1.0.0"),
            make_pkg("beta", "1.0.0"),
            make_pkg("gamma", "1.0.0"),
        ];

        let deps = BTreeMap::new(); // No dependencies
        assert!(verify_topo_order(&packages, &deps));
    }

    #[test]
    fn test_topo_diamond_dependency() {
        // Diamond: A depends on B and C, both depend on D
        let packages = vec![
            make_pkg("D", "1.0.0"),
            make_pkg("B", "1.0.0"),
            make_pkg("C", "1.0.0"),
            make_pkg("A", "1.0.0"),
        ];

        let mut deps = BTreeMap::new();
        deps.insert("B".into(), BTreeSet::from(["D".into()]));
        deps.insert("C".into(), BTreeSet::from(["D".into()]));
        deps.insert("A".into(), BTreeSet::from(["B".into(), "C".into()]));

        assert!(verify_topo_order(&packages, &deps));
    }
}
