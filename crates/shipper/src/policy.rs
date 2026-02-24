use crate::types::{PublishPolicy, RuntimeOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyKind {
    Safe,
    Balanced,
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyEffects {
    pub run_dry_run: bool,
    pub check_ownership: bool,
    pub strict_ownership: bool,
    pub readiness_enabled: bool,
}

pub(crate) fn policy_effects(opts: &RuntimeOptions) -> PolicyEffects {
    evaluate(
        match opts.policy {
            PublishPolicy::Safe => PolicyKind::Safe,
            PublishPolicy::Balanced => PolicyKind::Balanced,
            PublishPolicy::Fast => PolicyKind::Fast,
        },
        opts.no_verify,
        opts.skip_ownership_check,
        opts.strict_ownership,
        opts.readiness.enabled,
    )
}

pub(crate) fn evaluate(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_policy_respects_flags() {
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

    #[test]
    fn no_verify_forces_safe_policy_to_skip_dry_run() {
        let effects = evaluate(PolicyKind::Safe, true, false, true, true);
        assert!(!effects.run_dry_run);
        assert!(effects.check_ownership);
    }

    #[test]
    fn strict_ownership_override_is_not_active_for_fast_policy() {
        let effects = evaluate(PolicyKind::Fast, false, false, true, true);
        assert!(!effects.strict_ownership);
        assert!(!effects.readiness_enabled);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn policy_strategy() -> impl Strategy<Value = PolicyKind> {
        prop_oneof![
            Just(PolicyKind::Safe),
            Just(PolicyKind::Balanced),
            Just(PolicyKind::Fast),
        ]
    }

    proptest! {
        #[test]
        fn policy_invariants_hold_for_all_inputs(
            policy in policy_strategy(),
            no_verify in any::<bool>(),
            skip_ownership in any::<bool>(),
            strict in any::<bool>(),
            readiness in any::<bool>(),
        ) {
            let effects = evaluate(policy, no_verify, skip_ownership, strict, readiness);

            match policy {
                PolicyKind::Safe => {
                    prop_assert_eq!(effects.run_dry_run, !no_verify);
                    prop_assert_eq!(effects.check_ownership, !skip_ownership);
                    prop_assert_eq!(effects.strict_ownership, strict);
                    prop_assert_eq!(effects.readiness_enabled, readiness);
                }
                PolicyKind::Balanced => {
                    prop_assert_eq!(effects.run_dry_run, !no_verify);
                    prop_assert!(!effects.check_ownership);
                    prop_assert!(!effects.strict_ownership);
                    prop_assert_eq!(effects.readiness_enabled, readiness);
                }
                PolicyKind::Fast => {
                    prop_assert!(!effects.run_dry_run);
                    prop_assert!(!effects.check_ownership);
                    prop_assert!(!effects.strict_ownership);
                    prop_assert!(!effects.readiness_enabled);
                }
            }
        }
    }
}
