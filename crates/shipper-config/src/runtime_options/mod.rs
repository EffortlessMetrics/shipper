use std::path::PathBuf;
use std::time::Duration;

use shipper_types::{ParallelConfig, ReadinessConfig, RuntimeOptions};

use crate::{CliOverrides, ShipperConfig};

mod registry;
mod retry;
mod secrets;

pub(crate) fn build(config: &ShipperConfig, cli: CliOverrides) -> RuntimeOptions {
    let retry = retry::resolve(&config.retry, &cli);
    let readiness = resolve_readiness(config, &cli);
    let parallel = resolve_parallel(config, &cli);
    let webhook = secrets::resolve_webhook(&config.webhook, &cli);
    let encryption = secrets::resolve_encryption(&config.encryption, &cli);
    let registries = registry::resolve(&config.registries, &cli);
    let rehearsal_registry = resolve_rehearsal_registry(config, &cli);

    RuntimeOptions {
        allow_dirty: cli.allow_dirty || config.flags.allow_dirty,
        skip_ownership_check: cli.skip_ownership_check || config.flags.skip_ownership_check,
        strict_ownership: cli.strict_ownership || config.flags.strict_ownership,
        no_verify: cli.no_verify,
        max_attempts: retry.max_attempts,
        base_delay: retry.base_delay,
        max_delay: retry.max_delay,
        retry_strategy: retry.strategy,
        retry_jitter: retry.jitter,
        retry_per_error: retry.per_error,
        verify_timeout: cli.verify_timeout.unwrap_or(Duration::from_secs(120)),
        verify_poll_interval: cli.verify_poll_interval.unwrap_or(Duration::from_secs(5)),
        state_dir: cli
            .state_dir
            .unwrap_or_else(|| configured_state_dir(config)),
        force_resume: cli.force_resume,
        force: cli.force,
        lock_timeout: cli.lock_timeout.unwrap_or(config.lock.timeout),
        policy: cli.policy.unwrap_or(config.policy.mode),
        verify_mode: cli.verify_mode.unwrap_or(config.verify.mode),
        readiness,
        output_lines: cli.output_lines.unwrap_or(config.output.lines),
        parallel,
        webhook,
        encryption,
        registries,
        resume_from: cli.resume_from,
        rehearsal_registry,
        rehearsal_skip: cli.skip_rehearsal,
        rehearsal_smoke_install: cli.rehearsal_smoke_install,
    }
}

fn configured_state_dir(config: &ShipperConfig) -> PathBuf {
    config
        .state_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(".shipper"))
}

fn resolve_readiness(config: &ShipperConfig, cli: &CliOverrides) -> ReadinessConfig {
    ReadinessConfig {
        enabled: !cli.no_readiness && config.readiness.enabled,
        method: cli.readiness_method.unwrap_or(config.readiness.method),
        initial_delay: config.readiness.initial_delay,
        max_delay: config.readiness.max_delay,
        max_total_wait: cli
            .readiness_timeout
            .unwrap_or(config.readiness.max_total_wait),
        poll_interval: cli.readiness_poll.unwrap_or(config.readiness.poll_interval),
        jitter_factor: config.readiness.jitter_factor,
        index_path: config.readiness.index_path.clone(),
        prefer_index: config.readiness.prefer_index,
    }
}

fn resolve_parallel(config: &ShipperConfig, cli: &CliOverrides) -> ParallelConfig {
    ParallelConfig {
        enabled: cli.parallel_enabled || config.parallel.enabled,
        max_concurrent: cli.max_concurrent.unwrap_or(config.parallel.max_concurrent),
        per_package_timeout: cli
            .per_package_timeout
            .unwrap_or(config.parallel.per_package_timeout),
    }
}

fn resolve_rehearsal_registry(config: &ShipperConfig, cli: &CliOverrides) -> Option<String> {
    cli.rehearsal_registry.clone().or_else(|| {
        if config.rehearsal.enabled {
            config.rehearsal.registry.clone()
        } else {
            None
        }
    })
}
