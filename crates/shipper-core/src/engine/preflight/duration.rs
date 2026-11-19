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

#[cfg(test)]
mod tests {
    use super::*;

    type EstimateSummary = Option<(String, usize, usize, Duration)>;

    fn package(name: &str, is_new_crate: bool) -> PreflightPackage {
        PreflightPackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            already_published: false,
            is_new_crate,
            auth_type: None,
            ownership_verified: false,
            dry_run_passed: true,
            dry_run_output: None,
        }
    }

    fn summary(registry_name: &str, packages: &[PreflightPackage]) -> EstimateSummary {
        estimate_preflight_duration(registry_name, packages).map(|estimate| {
            (
                estimate.registry_profile,
                estimate.first_publish_count,
                estimate.update_count,
                estimate.minimum_registry_pacing,
            )
        })
    }

    #[test]
    fn unknown_registry_has_no_pacing_estimate() {
        let packages = [package("new-crate", true)];

        assert_eq!(summary("private-registry", &packages), None);
    }

    #[test]
    fn crates_io_counts_publish_regimes_without_pacing_inside_burst() {
        let packages = [
            package("first-a", true),
            package("update-a", false),
            package("first-b", true),
        ];

        assert_eq!(
            summary("crates-io", &packages),
            Some(("crates-io".to_string(), 2, 1, Duration::ZERO))
        );
    }

    #[test]
    fn crates_io_first_five_new_crates_fit_the_documented_burst() {
        let packages = (0..5)
            .map(|index| package(&format!("new-{index}"), true))
            .collect::<Vec<_>>();

        assert_eq!(
            summary("crates.io", &packages),
            Some(("crates-io".to_string(), 5, 0, Duration::ZERO))
        );
    }

    #[test]
    fn crates_io_sixth_new_crate_adds_one_refill_window() {
        let packages = (0..6)
            .map(|index| package(&format!("new-{index}"), true))
            .collect::<Vec<_>>();

        assert_eq!(
            summary("crates_io", &packages),
            Some(("crates-io".to_string(), 6, 0, Duration::from_mins(10),))
        );
    }

    #[test]
    fn crates_io_pacing_scales_only_with_new_crates_beyond_the_burst() {
        let mut packages = (0..8)
            .map(|index| package(&format!("new-{index}"), true))
            .collect::<Vec<_>>();
        packages.extend((0..4).map(|index| package(&format!("update-{index}"), false)));

        assert_eq!(
            summary(" CRATES-IO ", &packages),
            Some(("crates-io".to_string(), 8, 4, Duration::from_mins(30),))
        );
    }

    #[test]
    fn duration_multiplication_saturates_instead_of_wrapping() {
        assert_eq!(multiply_duration(Duration::MAX, 2), Duration::MAX);
    }
}
