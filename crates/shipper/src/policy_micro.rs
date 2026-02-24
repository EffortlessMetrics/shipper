use crate::types::RuntimeOptions;

pub(crate) use shipper_policy::PolicyEffects;

pub(crate) fn policy_effects(opts: &RuntimeOptions) -> PolicyEffects {
    shipper_policy::apply_policy(opts)
}
