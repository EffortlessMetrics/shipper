use proptest::prelude::*;

use super::*;

fn simple_token() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range('a', 'z'), 1..12)
        .prop_map(|chars: Vec<char>| chars.into_iter().collect::<String>())
}

fn crate_name() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range('a', 'z'), 1..32)
        .prop_map(|chars: Vec<char>| chars.into_iter().collect::<String>())
}

fn semver_version() -> impl Strategy<Value = String> {
    (0u32..100, 0u32..100, 0u32..100)
        .prop_map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
}

proptest! {
    #[test]
    fn silent_reporter_tracks_random_package_updates(
        total in 1usize..64,
        index_offset in 0usize..64,
        name in simple_token(),
        version in simple_token(),
    ) {
        let index = index_offset % total + 1;
        let mut reporter = ProgressReporter::silent(total);

        reporter.set_package(index, &name, &version);

        assert_eq!(reporter.total_packages(), total);
        assert_eq!(reporter.current_package(), index);
        assert_eq!(reporter.current_name(), format!("{name}@{version}"));

        reporter.finish_package();
        reporter.set_status("ready");
        reporter.finish();
    }

    /// Progress percentage (current / total) is always in [0.0, 100.0].
    #[test]
    fn percentage_always_in_range(
        total in 1usize..256,
        steps in 0usize..256,
    ) {
        let mut reporter = ProgressReporter::silent(total);
        let effective_steps = steps.min(total);

        for i in 1..=effective_steps {
            reporter.set_package(i, "pkg", "0.1.0");
            reporter.finish_package();
        }

        let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
        prop_assert!((0.0..=100.0).contains(&pct),
            "percentage {} out of range for {}/{}", pct, reporter.current_package(), reporter.total_packages());
    }

    /// After sequential publishing, current_package never exceeds total_packages.
    #[test]
    fn step_count_invariant(
        total in 1usize..128,
        publish_count in 0usize..128,
    ) {
        let mut reporter = ProgressReporter::silent(total);
        let to_publish = publish_count.min(total);

        for i in 1..=to_publish {
            reporter.set_package(i, "c", "0.0.1");
            prop_assert!(reporter.current_package() <= reporter.total_packages(),
                "current {} > total {}", reporter.current_package(), reporter.total_packages());
            reporter.finish_package();
        }

        prop_assert!(reporter.current_package() <= reporter.total_packages());
    }

    /// current_name() always matches the "name@version" format after set_package.
    #[test]
    fn display_format_name_at_version(
        name in crate_name(),
        version in semver_version(),
        total in 1usize..16,
    ) {
        let mut reporter = ProgressReporter::silent(total);
        reporter.set_package(1, &name, &version);

        let display = reporter.current_name().to_string();
        let expected = format!("{name}@{version}");
        prop_assert_eq!(&display, &expected);

        // Verify the '@' separator is present exactly once.
        let at_count = display.chars().filter(|&c| c == '@').count();
        prop_assert_eq!(at_count, 1, "expected exactly one '@' in '{}'", display);
    }

    /// A fresh reporter always starts at index 0 with an empty name.
    #[test]
    fn fresh_reporter_initial_state(total in 0usize..512) {
        let reporter = ProgressReporter::silent(total);
        prop_assert_eq!(reporter.total_packages(), total);
        prop_assert_eq!(reporter.current_package(), 0);
        prop_assert_eq!(reporter.current_name(), "");
        prop_assert!(!reporter.is_tty_mode());
    }

    /// Finishing the full sequence does not panic and ends with current == total.
    #[test]
    fn full_publish_cycle_completes(
        total in 1usize..64,
        names in prop::collection::vec(crate_name(), 1..64),
        versions in prop::collection::vec(semver_version(), 1..64),
    ) {
        let mut reporter = ProgressReporter::silent(total);

        for i in 1..=total {
            let name = &names[i % names.len()];
            let version = &versions[i % versions.len()];
            reporter.set_package(i, name, version);
            reporter.set_status("uploading");
            reporter.finish_package();
        }

        prop_assert_eq!(reporter.current_package(), total);
        reporter.finish();
    }

    /// set_status never panics regardless of the message content.
    #[test]
    fn set_status_never_panics(
        total in 0usize..32,
        status in ".*",
    ) {
        let reporter = ProgressReporter::silent(total);
        reporter.set_status(&status);
    }

    /// Percentage stays at 0% when no packages have been started.
    #[test]
    fn percentage_zero_before_any_work(total in 1usize..512) {
        let reporter = ProgressReporter::silent(total);
        let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
        prop_assert!((pct - 0.0).abs() < f64::EPSILON);
    }

    /// total_packages() is immutable: never changes after construction.
    #[test]
    fn total_packages_immutable(
        total in 0usize..256,
        ops in 0usize..64,
        name in crate_name(),
        version in semver_version(),
    ) {
        let mut reporter = ProgressReporter::silent(total);
        for i in 1..=ops.min(total.max(1)) {
            reporter.set_package(i, &name, &version);
            prop_assert_eq!(reporter.total_packages(), total);
            reporter.finish_package();
            prop_assert_eq!(reporter.total_packages(), total);
        }
        reporter.set_status("done");
        prop_assert_eq!(reporter.total_packages(), total);
    }

    /// finish_package does not alter current_name or current_package.
    #[test]
    fn finish_package_preserves_state(
        total in 1usize..64,
        index in 1usize..64,
        name in crate_name(),
        version in semver_version(),
    ) {
        let index = index.min(total);
        let mut reporter = ProgressReporter::silent(total);
        reporter.set_package(index, &name, &version);

        let name_before = reporter.current_name().to_string();
        let pkg_before = reporter.current_package();

        reporter.finish_package();

        prop_assert_eq!(reporter.current_name(), name_before.as_str());
        prop_assert_eq!(reporter.current_package(), pkg_before);
    }
}
