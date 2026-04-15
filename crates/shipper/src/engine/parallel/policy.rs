//! Policy-effect adapter for parallel publish.
//!
//! Translates `PublishPolicy` + flags into the resolved `PolicyEffects` used
//! internally by the publish loop (readiness, verify, ownership).

use shipper_types::RuntimeOptions;

pub(super) fn policy_effects(opts: &RuntimeOptions) -> shipper_policy::PolicyEffects {
    let policy = match opts.policy {
        shipper_types::PublishPolicy::Safe => shipper_policy::PolicyKind::Safe,
        shipper_types::PublishPolicy::Balanced => shipper_policy::PolicyKind::Balanced,
        shipper_types::PublishPolicy::Fast => shipper_policy::PolicyKind::Fast,
    };

    shipper_policy::evaluate(
        policy,
        opts.no_verify,
        opts.skip_ownership_check,
        opts.strict_ownership,
        opts.readiness.enabled,
    )
}
