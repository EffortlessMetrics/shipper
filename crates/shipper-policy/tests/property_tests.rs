use proptest::prelude::*;
use shipper_policy::{PolicyEffects, PolicyKind, apply_policy, evaluate};
use shipper_types::{ParallelConfig, PublishPolicy, ReadinessConfig, RuntimeOptions, VerifyMode};
use std::path::PathBuf;
use std::time::Duration;

fn policy_kind_strategy() -> impl Strategy<Value = PolicyKind> {
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
        any::<bool>(), // no_verify
        any::<bool>(), // skip_ownership_check
        any::<bool>(), // strict_ownership
        any::<bool>(), // readiness_enabled
        any::<bool>(), // allow_dirty
    )
        .prop_map(
            |(
                policy,
                no_verify,
                skip_ownership_check,
                strict_ownership,
                readiness_enabled,
                allow_dirty,
            )| {
                let readiness = ReadinessConfig {
                    enabled: readiness_enabled,
                    ..Default::default()
                };
                RuntimeOptions {
                    allow_dirty,
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
                    readiness,
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

/// Count the number of enabled safety checks in a `PolicyEffects`.
fn enabled_count(e: &PolicyEffects) -> u8 {
    u8::from(e.run_dry_run)
        + u8::from(e.check_ownership)
        + u8::from(e.strict_ownership)
        + u8::from(e.readiness_enabled)
}

proptest! {
    // Determinism: evaluate is a pure function — same inputs always produce
    // the same output.
    #[test]
    fn evaluate_is_deterministic(
        policy in policy_kind_strategy(),
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let a = evaluate(policy, no_verify, skip_ownership, strict, readiness);
        let b = evaluate(policy, no_verify, skip_ownership, strict, readiness);
        prop_assert_eq!(a, b);
    }

    // apply_policy is consistent with evaluate + From<PublishPolicy>.
    #[test]
    fn apply_policy_consistent_with_evaluate(opts in runtime_options_strategy()) {
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

    // From<PublishPolicy> preserves variant identity.
    #[test]
    fn publish_policy_to_policy_kind_roundtrip(pp in publish_policy_strategy()) {
        let kind = PolicyKind::from(pp);
        let expected = match pp {
            PublishPolicy::Safe => PolicyKind::Safe,
            PublishPolicy::Balanced => PolicyKind::Balanced,
            PublishPolicy::Fast => PolicyKind::Fast,
        };
        prop_assert_eq!(kind, expected);
    }

    // Fast policy is an absolute override — every field is false regardless
    // of input flags.
    #[test]
    fn fast_policy_disables_everything(
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(PolicyKind::Fast, no_verify, skip_ownership, strict, readiness);
        prop_assert!(!effects.run_dry_run);
        prop_assert!(!effects.check_ownership);
        prop_assert!(!effects.strict_ownership);
        prop_assert!(!effects.readiness_enabled);
    }

    // Balanced policy always disables ownership fields regardless of flags.
    #[test]
    fn balanced_always_disables_ownership(
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(PolicyKind::Balanced, no_verify, skip_ownership, strict, readiness);
        prop_assert!(!effects.check_ownership);
        prop_assert!(!effects.strict_ownership);
    }

    // Monotonicity: for the same input flags, Safe enables at least as many
    // checks as Balanced, and Balanced at least as many as Fast.
    #[test]
    fn safety_monotonicity(
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let safe = evaluate(PolicyKind::Safe, no_verify, skip_ownership, strict, readiness);
        let balanced = evaluate(PolicyKind::Balanced, no_verify, skip_ownership, strict, readiness);
        let fast = evaluate(PolicyKind::Fast, no_verify, skip_ownership, strict, readiness);
        prop_assert!(enabled_count(&safe) >= enabled_count(&balanced));
        prop_assert!(enabled_count(&balanced) >= enabled_count(&fast));
    }

    // Safe policy is a passthrough — effects mirror the input flags exactly.
    #[test]
    fn safe_policy_is_passthrough(
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(PolicyKind::Safe, no_verify, skip_ownership, strict, readiness);
        prop_assert_eq!(effects.run_dry_run, !no_verify);
        prop_assert_eq!(effects.check_ownership, !skip_ownership);
        prop_assert_eq!(effects.strict_ownership, strict);
        prop_assert_eq!(effects.readiness_enabled, readiness);
    }

    // Balanced preserves dry-run and readiness from flags (only ownership is
    // overridden).
    #[test]
    fn balanced_preserves_dry_run_and_readiness(
        no_verify in any::<bool>(),
        skip_ownership in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(PolicyKind::Balanced, no_verify, skip_ownership, strict, readiness);
        prop_assert_eq!(effects.run_dry_run, !no_verify);
        prop_assert_eq!(effects.readiness_enabled, readiness);
    }

    // PolicyEffects Debug roundtrip — the Debug representation can be
    // formatted without panicking for any valid effects value.
    #[test]
    fn policy_effects_debug_is_valid(
        policy in policy_kind_strategy(),
        no_verify in any::<bool>(),
        skip in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(policy, no_verify, skip, strict, readiness);
        let debug_str = format!("{effects:?}");
        prop_assert!(!debug_str.is_empty());
    }

    // PolicyKind Debug roundtrip — the Debug representation can be formatted
    // without panicking for any variant.
    #[test]
    fn policy_kind_debug_is_valid(kind in policy_kind_strategy()) {
        let debug_str = format!("{kind:?}");
        prop_assert!(!debug_str.is_empty());
    }

    // Clone produces an equal value for PolicyEffects.
    #[test]
    fn policy_effects_clone_eq(
        policy in policy_kind_strategy(),
        no_verify in any::<bool>(),
        skip in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let effects = evaluate(policy, no_verify, skip, strict, readiness);
        let cloned = effects;
        prop_assert_eq!(effects, cloned);
    }

    // PolicyKind clone is identical.
    #[test]
    fn policy_kind_clone_eq(kind in policy_kind_strategy()) {
        let cloned = kind;
        prop_assert_eq!(kind, cloned);
    }

    // Fast is strictly weaker than or equal to every other policy for any
    // given set of flags (all fields false).
    #[test]
    fn fast_is_weakest_policy(
        other in policy_kind_strategy(),
        no_verify in any::<bool>(),
        skip in any::<bool>(),
        strict in any::<bool>(),
        readiness in any::<bool>(),
    ) {
        let fast = evaluate(PolicyKind::Fast, no_verify, skip, strict, readiness);
        let other_effects = evaluate(other, no_verify, skip, strict, readiness);
        prop_assert!(enabled_count(&fast) <= enabled_count(&other_effects));
    }
}
