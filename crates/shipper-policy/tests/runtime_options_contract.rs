use std::path::PathBuf;
use std::time::Duration;

use shipper_policy::apply_policy;
use shipper_types::{ParallelConfig, PublishPolicy, ReadinessConfig, RuntimeOptions, VerifyMode};

fn sample_runtime_options() -> RuntimeOptions {
    RuntimeOptions {
        allow_dirty: true,
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
    }
}

#[test]
fn apply_policy_reads_runtime_options_for_safe_mode() {
    let mut opts = sample_runtime_options();
    opts.policy = PublishPolicy::Safe;
    opts.no_verify = true;
    opts.skip_ownership_check = true;
    opts.strict_ownership = true;
    opts.readiness.enabled = false;

    let effects = apply_policy(&opts);
    assert!(!effects.run_dry_run);
    assert!(!effects.check_ownership);
    assert!(effects.strict_ownership);
    assert!(!effects.readiness_enabled);
}

#[test]
fn apply_policy_balanced_ignores_strict_ownership() {
    let mut opts = sample_runtime_options();
    opts.policy = PublishPolicy::Balanced;
    opts.strict_ownership = true;
    opts.skip_ownership_check = false;
    opts.readiness.enabled = true;

    let effects = apply_policy(&opts);
    assert!(effects.run_dry_run);
    assert!(!effects.check_ownership);
    assert!(!effects.strict_ownership);
    assert!(effects.readiness_enabled);
}

#[test]
fn apply_policy_fast_disables_all_safety_checks() {
    let mut opts = sample_runtime_options();
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
