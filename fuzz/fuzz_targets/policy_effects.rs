#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_policy::{PolicyKind, evaluate};

fuzz_target!(|data: (u8, bool, bool, bool, bool)| {
    let (policy_byte, no_verify, skip_ownership_check, strict_ownership, readiness_enabled) =
        data;

    let policy = match policy_byte % 3 {
        0 => PolicyKind::Safe,
        1 => PolicyKind::Balanced,
        _ => PolicyKind::Fast,
    };

    let effects = evaluate(
        policy,
        no_verify,
        skip_ownership_check,
        strict_ownership,
        readiness_enabled,
    );

    match policy {
        PolicyKind::Safe => {
            assert_eq!(effects.run_dry_run, !no_verify);
            assert_eq!(effects.check_ownership, !skip_ownership_check);
            assert_eq!(effects.strict_ownership, strict_ownership);
            assert_eq!(effects.readiness_enabled, readiness_enabled);
        }
        PolicyKind::Balanced => {
            assert_eq!(effects.run_dry_run, !no_verify);
            assert!(!effects.check_ownership);
            assert!(!effects.strict_ownership);
            assert_eq!(effects.readiness_enabled, readiness_enabled);
        }
        PolicyKind::Fast => {
            assert!(!effects.run_dry_run);
            assert!(!effects.check_ownership);
            assert!(!effects.strict_ownership);
            assert!(!effects.readiness_enabled);
        }
    }
});
