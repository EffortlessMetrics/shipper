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
    }
}
