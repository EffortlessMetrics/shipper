//! Publish policy evaluation for shipper.
//!
//! This crate isolates policy decision logic from the publish engine.

use serde::Serialize;
use shipper_types::{PublishPolicy, RuntimeOptions};

/// Policy kind independent from any specific runtime options type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PolicyKind {
    Safe,
    Balanced,
    Fast,
}

impl From<PublishPolicy> for PolicyKind {
    fn from(value: PublishPolicy) -> Self {
        match value {
            PublishPolicy::Safe => PolicyKind::Safe,
            PublishPolicy::Balanced => PolicyKind::Balanced,
            PublishPolicy::Fast => PolicyKind::Fast,
        }
    }
}

/// Derived policy behavior used by publish/preflight execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PolicyEffects {
    /// Whether preflight dry-run verification should execute.
    pub run_dry_run: bool,
    /// Whether ownership checks should execute.
    pub check_ownership: bool,
    /// Whether missing ownership proof should fail execution.
    pub strict_ownership: bool,
    /// Whether post-publish readiness checks should execute.
    pub readiness_enabled: bool,
}

/// Evaluate policy effects directly from individual flags.
///
/// # Examples
///
/// ```
/// use shipper_policy::{PolicyKind, evaluate};
///
/// let effects = evaluate(PolicyKind::Safe, false, false, true, true);
/// assert!(effects.run_dry_run);
/// assert!(effects.check_ownership);
/// assert!(effects.strict_ownership);
/// assert!(effects.readiness_enabled);
///
/// // Fast policy disables all safety checks
/// let fast = evaluate(PolicyKind::Fast, false, false, true, true);
/// assert!(!fast.run_dry_run);
/// assert!(!fast.check_ownership);
/// ```
pub fn evaluate(
    policy: PolicyKind,
    no_verify: bool,
    skip_ownership_check: bool,
    strict_ownership: bool,
    readiness_enabled: bool,
) -> PolicyEffects {
    match policy {
        PolicyKind::Safe => PolicyEffects {
            run_dry_run: !no_verify,
            check_ownership: !skip_ownership_check,
            strict_ownership,
            readiness_enabled,
        },
        PolicyKind::Balanced => PolicyEffects {
            run_dry_run: !no_verify,
            check_ownership: false,
            strict_ownership: false,
            readiness_enabled,
        },
        PolicyKind::Fast => PolicyEffects {
            run_dry_run: false,
            check_ownership: false,
            strict_ownership: false,
            readiness_enabled: false,
        },
    }
}

/// Evaluate policy effects from full runtime options.
pub fn apply_policy(opts: &RuntimeOptions) -> PolicyEffects {
    evaluate(
        PolicyKind::from(opts.policy),
        opts.no_verify,
        opts.skip_ownership_check,
        opts.strict_ownership,
        opts.readiness.enabled,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_policy_respects_verify_ownership_and_readiness_flags() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: true,
                strict_ownership: true,
                readiness_enabled: true,
            }
        );
    }

    #[test]
    fn balanced_policy_disables_ownership_enforcement() {
        let effects = evaluate(PolicyKind::Balanced, false, false, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: true,
            }
        );
    }

    #[test]
    fn fast_policy_disables_safety_checks() {
        let effects = evaluate(PolicyKind::Fast, false, false, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- From<PublishPolicy> conversion tests ---

    #[test]
    fn from_publish_policy_safe() {
        assert_eq!(PolicyKind::from(PublishPolicy::Safe), PolicyKind::Safe);
    }

    #[test]
    fn from_publish_policy_balanced() {
        assert_eq!(
            PolicyKind::from(PublishPolicy::Balanced),
            PolicyKind::Balanced
        );
    }

    #[test]
    fn from_publish_policy_fast() {
        assert_eq!(PolicyKind::from(PublishPolicy::Fast), PolicyKind::Fast);
    }

    // --- Edge case: Safe with all flags false ---

    #[test]
    fn safe_all_flags_false() {
        let effects = evaluate(PolicyKind::Safe, false, false, false, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: true,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Edge case: Safe with all flags true ---

    #[test]
    fn safe_all_flags_true() {
        let effects = evaluate(PolicyKind::Safe, true, true, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: true,
                readiness_enabled: true,
            }
        );
    }

    // --- Edge case: Balanced with all flags false ---

    #[test]
    fn balanced_all_flags_false() {
        let effects = evaluate(PolicyKind::Balanced, false, false, false, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Edge case: Balanced with all flags true ---

    #[test]
    fn balanced_all_flags_true() {
        let effects = evaluate(PolicyKind::Balanced, true, true, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: true,
            }
        );
    }

    // --- Edge case: Fast with all flags false ---

    #[test]
    fn fast_all_flags_false() {
        let effects = evaluate(PolicyKind::Fast, false, false, false, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Edge case: Fast with all flags true ---

    #[test]
    fn fast_all_flags_true() {
        let effects = evaluate(PolicyKind::Fast, true, true, true, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Safe: no_verify only ---

    #[test]
    fn safe_no_verify_only() {
        let effects = evaluate(PolicyKind::Safe, true, false, false, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: true,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Safe: skip_ownership only ---

    #[test]
    fn safe_skip_ownership_only() {
        let effects = evaluate(PolicyKind::Safe, false, true, false, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }

    // --- Safe: strict_ownership only ---

    #[test]
    fn safe_strict_ownership_only() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: true,
                strict_ownership: true,
                readiness_enabled: false,
            }
        );
    }

    // --- Safe: readiness_enabled only ---

    #[test]
    fn safe_readiness_only() {
        let effects = evaluate(PolicyKind::Safe, false, false, false, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: true,
                strict_ownership: false,
                readiness_enabled: true,
            }
        );
    }

    // --- Balanced: no_verify disables dry-run ---

    #[test]
    fn balanced_no_verify_disables_dry_run() {
        let effects = evaluate(PolicyKind::Balanced, true, false, false, true);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: false,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: true,
            }
        );
    }

    // --- Balanced: ownership flags are always ignored ---

    #[test]
    fn balanced_ignores_ownership_flags() {
        let effects = evaluate(PolicyKind::Balanced, false, false, true, false);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: false,
                strict_ownership: false,
                readiness_enabled: false,
            }
        );
    }
}

#[cfg(test)]
mod apply_policy_tests {
    use super::*;
    use shipper_types::{ParallelConfig, ReadinessConfig, VerifyMode};
    use std::path::PathBuf;
    use std::time::Duration;

    fn base_opts() -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: false,
            skip_ownership_check: false,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(3),
            retry_strategy: shipper_retry::RetryStrategyType::Exponential,
            retry_jitter: 0.0,
            retry_per_error: shipper_retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_secs(2),
            verify_poll_interval: Duration::from_millis(200),
            state_dir: PathBuf::from(".shipper"),
            force_resume: false,
            policy: PublishPolicy::Safe,
            verify_mode: VerifyMode::Workspace,
            readiness: ReadinessConfig::default(),
            output_lines: 200,
            force: false,
            lock_timeout: Duration::from_secs(30),
            parallel: ParallelConfig::default(),
            webhook: Default::default(),
            encryption: Default::default(),
            registries: vec![],
            resume_from: None,
        }
    }

    #[test]
    fn apply_policy_safe_defaults() {
        let opts = base_opts();
        let effects = apply_policy(&opts);
        assert_eq!(
            effects,
            PolicyEffects {
                run_dry_run: true,
                check_ownership: true,
                strict_ownership: false,
                readiness_enabled: true, // ReadinessConfig::default().enabled == true
            }
        );
    }

    #[test]
    fn apply_policy_safe_with_no_verify() {
        let mut opts = base_opts();
        opts.no_verify = true;
        let effects = apply_policy(&opts);
        assert!(!effects.run_dry_run);
        assert!(effects.check_ownership);
    }

    #[test]
    fn apply_policy_safe_with_skip_ownership() {
        let mut opts = base_opts();
        opts.skip_ownership_check = true;
        let effects = apply_policy(&opts);
        assert!(effects.run_dry_run);
        assert!(!effects.check_ownership);
    }

    #[test]
    fn apply_policy_safe_with_strict_ownership() {
        let mut opts = base_opts();
        opts.strict_ownership = true;
        let effects = apply_policy(&opts);
        assert!(effects.strict_ownership);
    }

    #[test]
    fn apply_policy_safe_readiness_disabled() {
        let mut opts = base_opts();
        opts.readiness.enabled = false;
        let effects = apply_policy(&opts);
        assert!(!effects.readiness_enabled);
    }

    #[test]
    fn apply_policy_balanced_defaults() {
        let mut opts = base_opts();
        opts.policy = PublishPolicy::Balanced;
        let effects = apply_policy(&opts);
        assert!(effects.run_dry_run);
        assert!(!effects.check_ownership);
        assert!(!effects.strict_ownership);
        assert!(effects.readiness_enabled);
    }

    #[test]
    fn apply_policy_balanced_with_no_verify() {
        let mut opts = base_opts();
        opts.policy = PublishPolicy::Balanced;
        opts.no_verify = true;
        let effects = apply_policy(&opts);
        assert!(!effects.run_dry_run);
    }

    #[test]
    fn apply_policy_balanced_ignores_strict_ownership() {
        let mut opts = base_opts();
        opts.policy = PublishPolicy::Balanced;
        opts.strict_ownership = true;
        let effects = apply_policy(&opts);
        assert!(!effects.strict_ownership);
    }

    #[test]
    fn apply_policy_fast_ignores_all_flags() {
        let mut opts = base_opts();
        opts.policy = PublishPolicy::Fast;
        opts.no_verify = false;
        opts.skip_ownership_check = false;
        opts.strict_ownership = true;
        opts.readiness.enabled = true;
        let effects = apply_policy(&opts);
        assert!(!effects.run_dry_run);
        assert!(!effects.check_ownership);
        assert!(!effects.strict_ownership);
        assert!(!effects.readiness_enabled);
    }

    #[test]
    fn apply_policy_matches_evaluate() {
        let mut opts = base_opts();
        opts.policy = PublishPolicy::Safe;
        opts.no_verify = true;
        opts.skip_ownership_check = true;
        opts.strict_ownership = true;
        opts.readiness.enabled = false;

        let via_apply = apply_policy(&opts);
        let via_evaluate = evaluate(
            PolicyKind::from(opts.policy),
            opts.no_verify,
            opts.skip_ownership_check,
            opts.strict_ownership,
            opts.readiness.enabled,
        );
        assert_eq!(via_apply, via_evaluate);
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_debug_snapshot;

    // --- Safe policy snapshots ---

    #[test]
    fn snapshot_safe_all_enabled() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_safe_all_disabled() {
        let effects = evaluate(PolicyKind::Safe, true, true, false, false);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_safe_no_verify() {
        let effects = evaluate(PolicyKind::Safe, true, false, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_safe_skip_ownership() {
        let effects = evaluate(PolicyKind::Safe, false, true, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_safe_all_flags_false() {
        let effects = evaluate(PolicyKind::Safe, false, false, false, false);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_safe_all_flags_true() {
        let effects = evaluate(PolicyKind::Safe, true, true, true, true);
        assert_debug_snapshot!(effects);
    }

    // --- Balanced policy snapshots ---

    #[test]
    fn snapshot_balanced_defaults() {
        let effects = evaluate(PolicyKind::Balanced, false, false, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_balanced_no_verify() {
        let effects = evaluate(PolicyKind::Balanced, true, false, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_balanced_readiness_disabled() {
        let effects = evaluate(PolicyKind::Balanced, false, false, false, false);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_balanced_all_flags_true() {
        let effects = evaluate(PolicyKind::Balanced, true, true, true, true);
        assert_debug_snapshot!(effects);
    }

    // --- Fast policy snapshots ---

    #[test]
    fn snapshot_fast_defaults() {
        let effects = evaluate(PolicyKind::Fast, false, false, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_fast_all_flags_true() {
        let effects = evaluate(PolicyKind::Fast, true, true, true, true);
        assert_debug_snapshot!(effects);
    }

    #[test]
    fn snapshot_fast_all_flags_false() {
        let effects = evaluate(PolicyKind::Fast, false, false, false, false);
        assert_debug_snapshot!(effects);
    }

    // --- PolicyKind snapshots ---

    #[test]
    fn snapshot_policy_kind_safe() {
        assert_debug_snapshot!(PolicyKind::Safe);
    }

    #[test]
    fn snapshot_policy_kind_balanced() {
        assert_debug_snapshot!(PolicyKind::Balanced);
    }

    #[test]
    fn snapshot_policy_kind_fast() {
        assert_debug_snapshot!(PolicyKind::Fast);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use shipper_types::{ParallelConfig, ReadinessConfig, VerifyMode};
    use std::path::PathBuf;
    use std::time::Duration;

    fn policy_strategy() -> impl Strategy<Value = PolicyKind> {
        prop_oneof![
            Just(PolicyKind::Safe),
            Just(PolicyKind::Balanced),
            Just(PolicyKind::Fast),
        ]
    }

    fn publish_policy_strategy() -> impl Strategy<Value = PublishPolicy> {
        prop_oneof![
            Just(PublishPolicy::Safe),
            Just(PublishPolicy::Balanced),
            Just(PublishPolicy::Fast),
        ]
    }

    fn runtime_options_strategy() -> impl Strategy<Value = RuntimeOptions> {
        (
            publish_policy_strategy(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(policy, no_verify, skip_ownership_check, strict_ownership, readiness_enabled)| {
                    RuntimeOptions {
                        allow_dirty: false,
                        skip_ownership_check,
                        strict_ownership,
                        no_verify,
                        max_attempts: 3,
                        base_delay: Duration::from_millis(100),
                        max_delay: Duration::from_secs(3),
                        retry_strategy: shipper_retry::RetryStrategyType::Exponential,
                        retry_jitter: 0.0,
                        retry_per_error: shipper_retry::PerErrorConfig::default(),
                        verify_timeout: Duration::from_secs(2),
                        verify_poll_interval: Duration::from_millis(200),
                        state_dir: PathBuf::from(".shipper"),
                        force_resume: false,
                        policy,
                        verify_mode: VerifyMode::Workspace,
                        readiness: ReadinessConfig {
                            enabled: readiness_enabled,
                            ..Default::default()
                        },
                        output_lines: 200,
                        force: false,
                        lock_timeout: Duration::from_secs(30),
                        parallel: ParallelConfig::default(),
                        webhook: Default::default(),
                        encryption: Default::default(),
                        registries: vec![],
                        resume_from: None,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn policy_invariants_hold_for_all_inputs(
            policy in policy_strategy(),
            no_verify in any::<bool>(),
            skip_ownership_check in any::<bool>(),
            strict_ownership in any::<bool>(),
            readiness_enabled in any::<bool>(),
        ) {
            let effects = evaluate(
                policy,
                no_verify,
                skip_ownership_check,
                strict_ownership,
                readiness_enabled,
            );

            match policy {
                PolicyKind::Safe => {
                    prop_assert_eq!(effects.run_dry_run, !no_verify);
                    prop_assert_eq!(effects.check_ownership, !skip_ownership_check);
                    prop_assert_eq!(effects.strict_ownership, strict_ownership);
                    prop_assert_eq!(effects.readiness_enabled, readiness_enabled);
                }
                PolicyKind::Balanced => {
                    prop_assert_eq!(effects.run_dry_run, !no_verify);
                    prop_assert!(!effects.check_ownership);
                    prop_assert!(!effects.strict_ownership);
                    prop_assert_eq!(effects.readiness_enabled, readiness_enabled);
                }
                PolicyKind::Fast => {
                    prop_assert!(!effects.run_dry_run);
                    prop_assert!(!effects.check_ownership);
                    prop_assert!(!effects.strict_ownership);
                    prop_assert!(!effects.readiness_enabled);
                }
            }
        }

        #[test]
        fn apply_policy_roundtrip_matches_evaluate(opts in runtime_options_strategy()) {
            let via_apply = apply_policy(&opts);
            let via_evaluate = evaluate(
                PolicyKind::from(opts.policy),
                opts.no_verify,
                opts.skip_ownership_check,
                opts.strict_ownership,
                opts.readiness.enabled,
            );
            prop_assert_eq!(via_apply, via_evaluate);
        }
    }
}

// ---------------------------------------------------------------------------
// Policy ordering invariant tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod policy_ordering_tests {
    use super::*;

    fn enabled_count(e: &PolicyEffects) -> u8 {
        u8::from(e.run_dry_run)
            + u8::from(e.check_ownership)
            + u8::from(e.strict_ownership)
            + u8::from(e.readiness_enabled)
    }

    #[test]
    fn safe_at_least_as_conservative_as_balanced() {
        let safe = evaluate(PolicyKind::Safe, false, false, true, true);
        let balanced = evaluate(PolicyKind::Balanced, false, false, true, true);
        assert!(safe.run_dry_run >= balanced.run_dry_run);
        assert!(safe.check_ownership >= balanced.check_ownership);
        assert!(safe.strict_ownership >= balanced.strict_ownership);
        assert!(safe.readiness_enabled >= balanced.readiness_enabled);
    }

    #[test]
    fn balanced_at_least_as_conservative_as_fast() {
        let balanced = evaluate(PolicyKind::Balanced, false, false, true, true);
        let fast = evaluate(PolicyKind::Fast, false, false, true, true);
        assert!(balanced.run_dry_run >= fast.run_dry_run);
        assert!(balanced.check_ownership >= fast.check_ownership);
        assert!(balanced.strict_ownership >= fast.strict_ownership);
        assert!(balanced.readiness_enabled >= fast.readiness_enabled);
    }

    #[test]
    fn fast_output_is_constant_regardless_of_flags() {
        let expected = PolicyEffects {
            run_dry_run: false,
            check_ownership: false,
            strict_ownership: false,
            readiness_enabled: false,
        };
        for nv in [false, true] {
            for so in [false, true] {
                for stro in [false, true] {
                    for re in [false, true] {
                        assert_eq!(
                            evaluate(PolicyKind::Fast, nv, so, stro, re),
                            expected,
                            "Fast must ignore flags: no_verify={nv}, skip_own={so}, strict={stro}, ready={re}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn conservativeness_count_ordering_default_flags() {
        let safe = evaluate(PolicyKind::Safe, false, false, true, true);
        let balanced = evaluate(PolicyKind::Balanced, false, false, true, true);
        let fast = evaluate(PolicyKind::Fast, false, false, true, true);
        assert!(enabled_count(&safe) >= enabled_count(&balanced));
        assert!(enabled_count(&balanced) >= enabled_count(&fast));
    }
}

// ---------------------------------------------------------------------------
// Per-field coverage across all three variants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod field_coverage_tests {
    use super::*;

    #[test]
    fn run_dry_run_field_all_variants() {
        // no_verify=false ⇒ Safe and Balanced enable, Fast does not
        assert!(evaluate(PolicyKind::Safe, false, false, false, false).run_dry_run);
        assert!(evaluate(PolicyKind::Balanced, false, false, false, false).run_dry_run);
        assert!(!evaluate(PolicyKind::Fast, false, false, false, false).run_dry_run);

        // no_verify=true ⇒ all disable
        assert!(!evaluate(PolicyKind::Safe, true, false, false, false).run_dry_run);
        assert!(!evaluate(PolicyKind::Balanced, true, false, false, false).run_dry_run);
        assert!(!evaluate(PolicyKind::Fast, true, false, false, false).run_dry_run);
    }

    #[test]
    fn check_ownership_field_all_variants() {
        // Only Safe respects the flag
        assert!(evaluate(PolicyKind::Safe, false, false, false, false).check_ownership);
        assert!(!evaluate(PolicyKind::Balanced, false, false, false, false).check_ownership);
        assert!(!evaluate(PolicyKind::Fast, false, false, false, false).check_ownership);

        // Safe with skip_ownership disables it
        assert!(!evaluate(PolicyKind::Safe, false, true, false, false).check_ownership);
    }

    #[test]
    fn strict_ownership_field_all_variants() {
        // Only Safe forwards the flag
        assert!(evaluate(PolicyKind::Safe, false, false, true, false).strict_ownership);
        assert!(!evaluate(PolicyKind::Balanced, false, false, true, false).strict_ownership);
        assert!(!evaluate(PolicyKind::Fast, false, false, true, false).strict_ownership);
    }

    #[test]
    fn readiness_enabled_field_all_variants() {
        // Safe and Balanced forward, Fast ignores
        assert!(evaluate(PolicyKind::Safe, false, false, false, true).readiness_enabled);
        assert!(evaluate(PolicyKind::Balanced, false, false, false, true).readiness_enabled);
        assert!(!evaluate(PolicyKind::Fast, false, false, false, true).readiness_enabled);

        // When disabled at input, Safe and Balanced also report false
        assert!(!evaluate(PolicyKind::Safe, false, false, false, false).readiness_enabled);
        assert!(!evaluate(PolicyKind::Balanced, false, false, false, false).readiness_enabled);
    }
}

// ---------------------------------------------------------------------------
// Serialization snapshot tests (YAML)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod serialization_snapshot_tests {
    use super::*;
    use insta::assert_yaml_snapshot;

    #[test]
    fn snapshot_yaml_safe_policy_effects_default() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, true);
        assert_yaml_snapshot!("safe_policy_effects_default", effects);
    }

    #[test]
    fn snapshot_yaml_balanced_policy_effects_default() {
        let effects = evaluate(PolicyKind::Balanced, false, false, true, true);
        assert_yaml_snapshot!("balanced_policy_effects_default", effects);
    }

    #[test]
    fn snapshot_yaml_fast_policy_effects_default() {
        let effects = evaluate(PolicyKind::Fast, false, false, true, true);
        assert_yaml_snapshot!("fast_policy_effects_default", effects);
    }

    #[test]
    fn snapshot_yaml_all_policy_kinds() {
        assert_yaml_snapshot!(
            "all_policy_kinds",
            vec![PolicyKind::Safe, PolicyKind::Balanced, PolicyKind::Fast,]
        );
    }
}

// ---------------------------------------------------------------------------
// Trait behavior tests (Clone, Copy, Eq)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod trait_behavior_tests {
    use super::*;

    #[test]
    fn policy_kind_clone_equals_original() {
        for kind in [PolicyKind::Safe, PolicyKind::Balanced, PolicyKind::Fast] {
            let copied = kind; // Copy
            assert_eq!(kind, copied);
            #[allow(clippy::clone_on_copy)]
            let cloned = kind.clone(); // Clone
            assert_eq!(kind, cloned);
        }
    }

    #[test]
    fn policy_effects_clone_equals_original() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, true);
        let copied = effects; // Copy
        assert_eq!(effects, copied);
        #[allow(clippy::clone_on_copy)]
        let cloned = effects.clone(); // Clone
        assert_eq!(effects, cloned);
    }

    #[test]
    fn policy_kind_eq_is_reflexive() {
        assert_eq!(PolicyKind::Safe, PolicyKind::Safe);
        assert_eq!(PolicyKind::Balanced, PolicyKind::Balanced);
        assert_eq!(PolicyKind::Fast, PolicyKind::Fast);
    }

    #[test]
    fn policy_kind_ne_across_variants() {
        assert_ne!(PolicyKind::Safe, PolicyKind::Balanced);
        assert_ne!(PolicyKind::Safe, PolicyKind::Fast);
        assert_ne!(PolicyKind::Balanced, PolicyKind::Fast);
    }

    #[test]
    fn policy_effects_ne_when_different() {
        let safe = evaluate(PolicyKind::Safe, false, false, true, true);
        let fast = evaluate(PolicyKind::Fast, false, false, true, true);
        assert_ne!(safe, fast);
    }
}

// ---------------------------------------------------------------------------
// Debug format stability tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod debug_format_tests {
    use super::*;

    #[test]
    fn debug_format_policy_kind_variants() {
        assert_eq!(format!("{:?}", PolicyKind::Safe), "Safe");
        assert_eq!(format!("{:?}", PolicyKind::Balanced), "Balanced");
        assert_eq!(format!("{:?}", PolicyKind::Fast), "Fast");
    }

    #[test]
    fn debug_format_policy_effects_contains_field_names() {
        let effects = evaluate(PolicyKind::Safe, false, false, true, true);
        let debug_str = format!("{effects:?}");
        assert!(debug_str.contains("run_dry_run"));
        assert!(debug_str.contains("check_ownership"));
        assert!(debug_str.contains("strict_ownership"));
        assert!(debug_str.contains("readiness_enabled"));
    }
}

// ---------------------------------------------------------------------------
// Additional property-based tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod additional_property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn fast_always_produces_constant_effects(
            no_verify in any::<bool>(),
            skip_ownership in any::<bool>(),
            strict_ownership in any::<bool>(),
            readiness in any::<bool>(),
        ) {
            let effects = evaluate(PolicyKind::Fast, no_verify, skip_ownership, strict_ownership, readiness);
            prop_assert!(!effects.run_dry_run);
            prop_assert!(!effects.check_ownership);
            prop_assert!(!effects.strict_ownership);
            prop_assert!(!effects.readiness_enabled);
        }

        #[test]
        fn safe_at_least_as_conservative_as_balanced_all_inputs(
            no_verify in any::<bool>(),
            skip_ownership in any::<bool>(),
            strict_ownership in any::<bool>(),
            readiness in any::<bool>(),
        ) {
            let safe = evaluate(PolicyKind::Safe, no_verify, skip_ownership, strict_ownership, readiness);
            let balanced = evaluate(PolicyKind::Balanced, no_verify, skip_ownership, strict_ownership, readiness);
            prop_assert!(safe.run_dry_run >= balanced.run_dry_run);
            prop_assert!(safe.check_ownership >= balanced.check_ownership);
            prop_assert!(safe.strict_ownership >= balanced.strict_ownership);
            prop_assert!(safe.readiness_enabled >= balanced.readiness_enabled);
        }

        #[test]
        fn balanced_at_least_as_conservative_as_fast_all_inputs(
            no_verify in any::<bool>(),
            skip_ownership in any::<bool>(),
            strict_ownership in any::<bool>(),
            readiness in any::<bool>(),
        ) {
            let balanced = evaluate(PolicyKind::Balanced, no_verify, skip_ownership, strict_ownership, readiness);
            let fast = evaluate(PolicyKind::Fast, no_verify, skip_ownership, strict_ownership, readiness);
            prop_assert!(balanced.run_dry_run >= fast.run_dry_run);
            prop_assert!(balanced.check_ownership >= fast.check_ownership);
            prop_assert!(balanced.strict_ownership >= fast.strict_ownership);
            prop_assert!(balanced.readiness_enabled >= fast.readiness_enabled);
        }

        #[test]
        fn publish_policy_to_policy_kind_roundtrip_preserves_variant(
            pp in prop_oneof![
                Just(PublishPolicy::Safe),
                Just(PublishPolicy::Balanced),
                Just(PublishPolicy::Fast),
            ]
        ) {
            let kind = PolicyKind::from(pp);
            match pp {
                PublishPolicy::Safe => prop_assert_eq!(kind, PolicyKind::Safe),
                PublishPolicy::Balanced => prop_assert_eq!(kind, PolicyKind::Balanced),
                PublishPolicy::Fast => prop_assert_eq!(kind, PolicyKind::Fast),
            }
        }
    }
}
