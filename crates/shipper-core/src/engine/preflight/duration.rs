//! Estimated lower-bound publish duration derived from registry pacing rules.

use std::time::Duration;

use crate::runtime::execution::RegistryProfile;
use crate::types::{PreflightDurationEstimate, PreflightPackage};

pub(in crate::engine) fn estimate_preflight_duration(
    registry_name: &str,
    packages: &[PreflightPackage],
) -> Option<PreflightDurationEstimate> {
    let profile = RegistryProfile::for_registry_name(registry_name);
    let first_publish_refill = profile.first_publish_refill?;
    let first_publish_burst = profile.first_publish_burst.unwrap_or(0) as usize;
    let first_publish_count = packages.iter().filter(|p| p.is_new_crate).count();
    let update_count = packages.len().saturating_sub(first_publish_count);
    let paced_publishes = first_publish_count.saturating_sub(first_publish_burst);
    let minimum_registry_pacing = multiply_duration(first_publish_refill, paced_publishes);

    Some(PreflightDurationEstimate {
        registry_profile: profile.name.to_string(),
        first_publish_count,
        update_count,
        minimum_registry_pacing,
        notes: vec![
            "Estimate includes documented registry pacing only.".to_string(),
            "It excludes build time, upload time, readiness polling, retries, and human pauses."
                .to_string(),
        ],
    })
}

fn multiply_duration(duration: Duration, count: usize) -> Duration {
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    duration.checked_mul(count).unwrap_or(Duration::MAX)
}
