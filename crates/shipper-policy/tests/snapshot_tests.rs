use insta::assert_yaml_snapshot;
use shipper_policy::{PolicyEffects, PolicyKind, evaluate};

// --- Policy evaluation results ---

#[test]
fn safe_policy_default_flags() {
    let effects = evaluate(PolicyKind::Safe, false, false, true, true);
    assert_yaml_snapshot!(effects);
}

#[test]
fn balanced_policy_default_flags() {
    let effects = evaluate(PolicyKind::Balanced, false, false, true, true);
    assert_yaml_snapshot!(effects);
}

#[test]
fn fast_policy_default_flags() {
    let effects = evaluate(PolicyKind::Fast, false, false, true, true);
    assert_yaml_snapshot!(effects);
}

// --- Publish decisions based on different policies ---

#[test]
fn safe_policy_no_verify_skips_dry_run() {
    let effects = evaluate(PolicyKind::Safe, true, false, true, true);
    assert_yaml_snapshot!(effects);
}

#[test]
fn safe_policy_skip_ownership_check() {
    let effects = evaluate(PolicyKind::Safe, false, true, true, true);
    assert_yaml_snapshot!(effects);
}

#[test]
fn safe_policy_all_disabled() {
    let effects = evaluate(PolicyKind::Safe, true, true, false, false);
    assert_yaml_snapshot!(effects);
}

#[test]
fn balanced_policy_no_verify() {
    let effects = evaluate(PolicyKind::Balanced, true, false, true, true);
    assert_yaml_snapshot!(effects);
}

#[test]
fn balanced_policy_readiness_disabled() {
    let effects = evaluate(PolicyKind::Balanced, false, false, false, false);
    assert_yaml_snapshot!(effects);
}

#[test]
fn fast_policy_ignores_all_overrides() {
    // Fast disables everything regardless of input flags
    let effects = evaluate(PolicyKind::Fast, false, false, true, true);
    assert_yaml_snapshot!(effects);
}

// --- Display formatting of policy variants ---

#[test]
fn policy_kind_display_formatting() {
    assert_yaml_snapshot!("safe", format!("{:?}", PolicyKind::Safe));
    assert_yaml_snapshot!("balanced", format!("{:?}", PolicyKind::Balanced));
    assert_yaml_snapshot!("fast", format!("{:?}", PolicyKind::Fast));
}

#[test]
fn policy_effects_display_formatting() {
    let effects = PolicyEffects {
        run_dry_run: true,
        check_ownership: true,
        strict_ownership: false,
        readiness_enabled: true,
    };
    assert_yaml_snapshot!(format!("{effects:?}"));
}
