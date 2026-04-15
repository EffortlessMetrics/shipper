//! Policy-effect adapter for parallel publish.
//!
//! Translates `PublishPolicy` + flags into the resolved `PolicyEffects` used
//! internally by the publish loop (readiness, verify, ownership).

use shipper_types::RuntimeOptions;

pub(super) fn policy_effects(opts: &RuntimeOptions) -> crate::runtime::policy::PolicyEffects {
    let policy = match opts.policy {
        shipper_types::PublishPolicy::Safe => crate::runtime::policy::PolicyKind::Safe,
        shipper_types::PublishPolicy::Balanced => crate::runtime::policy::PolicyKind::Balanced,
        shipper_types::PublishPolicy::Fast => crate::runtime::policy::PolicyKind::Fast,
    };

    crate::runtime::policy::evaluate(
        policy,
        opts.no_verify,
        opts.skip_ownership_check,
        opts.strict_ownership,
        opts.readiness.enabled,
    )
}
