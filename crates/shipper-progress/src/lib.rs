//! Progress reporting primitives for CLI flows.
//!
//! This microcrate extracts the publish progress output behavior from
//! `shipper-cli` so it can be tested, fuzzed, and reused independently.

use std::time::Instant;

use atty::Stream;
use indicatif::{ProgressBar, ProgressStyle};

/// Returns `true` when standard output is connected to a terminal.
pub fn is_tty() -> bool {
    atty::is(Stream::Stdout)
}

/// Progress reporter that emits an interactive progress bar in TTY mode and
/// falls back to non-interactive text output otherwise.
pub struct ProgressReporter {
    is_tty: bool,
    quiet: bool,
    total_packages: usize,
    current_package: usize,
    current_name: String,
    progress_bar: Option<ProgressBar>,
    start_time: Instant,
}

impl ProgressReporter {
    /// Creates a new reporter for the given total package count.
    pub fn new(total_packages: usize, quiet: bool) -> Self {
        let is_tty = is_tty() && !quiet;
        let progress_bar = if is_tty {
            let pb = ProgressBar::new(total_packages as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("#>-"),
            );
            Some(pb)
        } else {
            None
        };

        Self {
            is_tty,
            quiet,
            total_packages,
            current_package: 0,
            current_name: String::new(),
            progress_bar,
            start_time: Instant::now(),
        }
    }

    /// Creates a silent reporter that always uses non-TTY behavior and suppresses output.
    pub fn silent(total_packages: usize) -> Self {
        Self {
            is_tty: false,
            quiet: true,
            total_packages,
            current_package: 0,
            current_name: String::new(),
            progress_bar: None,
            start_time: Instant::now(),
        }
    }

    /// Returns whether this reporter is currently emitting TTY-style output.
    pub fn is_tty_mode(&self) -> bool {
        self.is_tty
    }

    /// Returns the configured package count.
    pub fn total_packages(&self) -> usize {
        self.total_packages
    }

    /// Returns the current 1-indexed package position.
    pub fn current_package(&self) -> usize {
        self.current_package
    }

    /// Returns the currently active package label (`name@version`).
    pub fn current_name(&self) -> &str {
        &self.current_name
    }

    /// Records the active package being published.
    pub fn set_package(&mut self, index: usize, name: &str, version: &str) {
        self.current_package = index;
        self.current_name = format!("{name}@{version}");

        if self.quiet {
            return;
        }

        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                let elapsed = self.start_time.elapsed();
                let msg = format!(
                    "[{}/{}] Publishing {}... ({elapsed:?})",
                    self.current_package, self.total_packages, self.current_name
                );
                pb.set_message(msg);
                let position = index.saturating_sub(1) as u64;
                pb.set_position(position);
            }
        } else {
            let elapsed = self.start_time.elapsed();
            eprintln!(
                "[{}/{}] Publishing {}... ({elapsed:?})",
                self.current_package, self.total_packages, self.current_name
            );
        }
    }

    /// Marks the package at the current index as completed.
    pub fn finish_package(&mut self) {
        if self.quiet {
            return;
        }

        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                pb.inc(1);
            }
        } else {
            eprintln!(
                "[{}/{}] Finished {}",
                self.current_package, self.total_packages, self.current_name
            );
        }
    }

    /// Updates the message for the current package state.
    pub fn set_status(&self, status: &str) {
        if self.quiet {
            return;
        }

        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                let current = pb.position();
                let msg = format!("[{}/{}] {}", current + 1, self.total_packages, status);
                pb.set_message(msg);
            }
        } else {
            eprintln!("[status] {status}");
        }
    }

    /// Finishes reporting and prints completion summary in non-TTY mode.
    pub fn finish(self) {
        if self.quiet {
            return;
        }

        if self.is_tty {
            if let Some(pb) = self.progress_bar {
                let elapsed = self.start_time.elapsed();
                let msg = format!(
                    "Completed {} packages in {:?}",
                    self.total_packages, elapsed
                );
                pb.set_message(msg);
                pb.finish();
            }
        } else {
            let elapsed = self.start_time.elapsed();
            eprintln!(
                "Completed {}/{} packages in {:?}",
                self.total_packages, self.total_packages, elapsed
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Basic construction ---

    #[test]
    fn test_is_tty_returns_bool() {
        let result = is_tty();
        assert!(matches!(result, true | false));
    }

    #[test]
    fn test_progress_reporter_creation() {
        let reporter = ProgressReporter::new(5, false);
        assert_eq!(reporter.total_packages(), 5);
        assert_eq!(reporter.current_package(), 0);
        assert_eq!(reporter.current_name(), "");
        assert_eq!(reporter.is_tty_mode(), is_tty());
    }

    #[test]
    fn test_silent_reporter_disables_tty() {
        let reporter = ProgressReporter::silent(3);
        assert!(!reporter.is_tty_mode());
        assert_eq!(reporter.total_packages(), 3);
    }

    #[test]
    fn test_new_quiet_mode_disables_tty() {
        let reporter = ProgressReporter::new(4, true);
        assert!(!reporter.is_tty_mode());
        assert_eq!(reporter.total_packages(), 4);
        assert_eq!(reporter.current_package(), 0);
        assert_eq!(reporter.current_name(), "");
    }

    #[test]
    fn test_silent_initial_state() {
        let reporter = ProgressReporter::silent(0);
        assert!(!reporter.is_tty_mode());
        assert_eq!(reporter.total_packages(), 0);
        assert_eq!(reporter.current_package(), 0);
        assert_eq!(reporter.current_name(), "");
    }

    // --- set_package state tracking ---

    #[test]
    fn test_set_package_updates_state() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "test-crate", "1.0.0");
        assert_eq!(reporter.current_package(), 1);
        assert_eq!(reporter.current_name(), "test-crate@1.0.0");
    }

    #[test]
    fn test_set_package_formats_name_at_version() {
        let mut reporter = ProgressReporter::silent(1);
        reporter.set_package(1, "my-lib", "2.3.4-beta.1");
        assert_eq!(reporter.current_name(), "my-lib@2.3.4-beta.1");
    }

    #[test]
    fn test_set_package_overwrites_previous() {
        let mut reporter = ProgressReporter::silent(5);
        reporter.set_package(1, "alpha", "0.1.0");
        assert_eq!(reporter.current_name(), "alpha@0.1.0");

        reporter.set_package(2, "beta", "0.2.0");
        assert_eq!(reporter.current_package(), 2);
        assert_eq!(reporter.current_name(), "beta@0.2.0");
    }

    // --- Multi-crate progress tracking ---

    #[test]
    fn test_multi_crate_sequential_publish() {
        let mut reporter = ProgressReporter::silent(4);
        let crates = [
            (1, "core", "0.1.0"),
            (2, "utils", "0.2.0"),
            (3, "macros", "0.3.0"),
            (4, "cli", "1.0.0"),
        ];

        for (idx, name, version) in &crates {
            reporter.set_package(*idx, name, version);
            assert_eq!(reporter.current_package(), *idx);
            assert_eq!(reporter.current_name(), format!("{name}@{version}"));
            reporter.finish_package();
        }

        assert_eq!(reporter.current_package(), 4);
        reporter.finish();
    }

    #[test]
    fn test_multi_crate_status_updates_between_packages() {
        let mut reporter = ProgressReporter::silent(2);

        reporter.set_package(1, "dep", "0.1.0");
        reporter.set_status("Uploading");
        reporter.set_status("Waiting for registry");
        reporter.finish_package();

        reporter.set_package(2, "app", "1.0.0");
        reporter.set_status("Verifying");
        reporter.finish_package();

        assert_eq!(reporter.current_package(), 2);
        assert_eq!(reporter.current_name(), "app@1.0.0");
        reporter.finish();
    }

    // --- finish_package / finish ---

    #[test]
    fn test_finish_package_increments() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "test-crate", "1.0.0");
        reporter.finish_package();
    }

    #[test]
    fn test_finish_completes_without_panic() {
        let reporter = ProgressReporter::silent(3);
        reporter.finish();
    }

    #[test]
    fn test_finish_package_without_set_package() {
        let mut reporter = ProgressReporter::silent(2);
        // Calling finish_package before set_package should not panic.
        reporter.finish_package();
        assert_eq!(reporter.current_package(), 0);
        assert_eq!(reporter.current_name(), "");
    }

    #[test]
    fn test_finish_on_fresh_reporter() {
        // Finishing immediately without any package activity should be safe.
        let reporter = ProgressReporter::silent(5);
        reporter.finish();
    }

    // --- set_status ---

    #[test]
    fn test_set_status_on_silent_reporter() {
        let reporter = ProgressReporter::silent(1);
        // Should not panic even with no active package.
        reporter.set_status("Idle");
        reporter.set_status("Preparing metadata");
        reporter.set_status("");
    }

    #[test]
    fn test_set_status_with_special_characters() {
        let reporter = ProgressReporter::silent(1);
        reporter.set_status("Retrying (attempt 3/5)...");
        reporter.set_status("Rate limited — backing off 30s");
        reporter.set_status("✓ Published successfully");
    }

    // --- Edge cases: zero packages ---

    #[test]
    fn test_zero_packages_silent() {
        let reporter = ProgressReporter::silent(0);
        assert_eq!(reporter.total_packages(), 0);
        assert_eq!(reporter.current_package(), 0);
        reporter.finish();
    }

    #[test]
    fn test_zero_packages_new_quiet() {
        let reporter = ProgressReporter::new(0, true);
        assert_eq!(reporter.total_packages(), 0);
        reporter.finish();
    }

    #[test]
    fn test_zero_packages_set_status_and_finish() {
        let mut reporter = ProgressReporter::silent(0);
        reporter.set_status("Nothing to publish");
        reporter.finish_package();
        reporter.finish();
    }

    // --- Edge cases: very long package names ---

    #[test]
    fn test_very_long_package_name() {
        let long_name = "a".repeat(256);
        let long_version = "0.0.1-alpha.".to_string() + &"9".repeat(200);
        let mut reporter = ProgressReporter::silent(1);

        reporter.set_package(1, &long_name, &long_version);

        let expected = format!("{long_name}@{long_version}");
        assert_eq!(reporter.current_name(), expected);
        reporter.finish_package();
        reporter.finish();
    }

    // --- Edge cases: empty / unusual strings ---

    #[test]
    fn test_empty_package_name_and_version() {
        let mut reporter = ProgressReporter::silent(1);
        reporter.set_package(1, "", "");
        assert_eq!(reporter.current_name(), "@");
        reporter.finish_package();
        reporter.finish();
    }

    #[test]
    fn test_unicode_package_name() {
        let mut reporter = ProgressReporter::silent(1);
        reporter.set_package(1, "日本語パッケージ", "1.0.0");
        assert_eq!(reporter.current_name(), "日本語パッケージ@1.0.0");
    }

    // --- Edge case: large total package count ---

    #[test]
    fn test_large_total_packages() {
        let reporter = ProgressReporter::silent(10_000);
        assert_eq!(reporter.total_packages(), 10_000);
        reporter.finish();
    }

    // --- Repeated operations ---

    #[test]
    fn test_repeated_set_package_same_index() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "crate-a", "0.1.0");
        reporter.set_package(1, "crate-b", "0.2.0");
        // Last write wins.
        assert_eq!(reporter.current_package(), 1);
        assert_eq!(reporter.current_name(), "crate-b@0.2.0");
    }

    #[test]
    fn test_finish_package_called_multiple_times() {
        let mut reporter = ProgressReporter::silent(2);
        reporter.set_package(1, "foo", "1.0.0");
        reporter.finish_package();
        reporter.finish_package();
        // Should not panic; state is unchanged after extra calls.
        assert_eq!(reporter.current_package(), 1);
    }

    // --- Non-TTY explicit construction (quiet=false but tests are not a TTY) ---

    #[test]
    fn test_new_non_quiet_in_test_environment() {
        // In CI / test environment stdout is typically not a TTY.
        let mut reporter = ProgressReporter::new(2, false);
        reporter.set_package(1, "pkg", "0.1.0");
        reporter.set_status("Publishing");
        reporter.finish_package();
        reporter.set_package(2, "pkg2", "0.2.0");
        reporter.finish_package();
        reporter.finish();
    }

    // --- Interleaved status and package operations ---

    #[test]
    fn test_status_before_and_after_package() {
        let mut reporter = ProgressReporter::silent(1);
        reporter.set_status("Initializing");
        reporter.set_package(1, "only-crate", "0.1.0");
        reporter.set_status("Uploading tarball");
        reporter.set_status("Waiting for index");
        reporter.finish_package();
        reporter.set_status("All done");
        reporter.finish();
    }
}

#[cfg(test)]
mod property_tests {
    use proptest::prelude::*;

    use super::*;

    fn simple_token() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::char::range('a', 'z'), 1..12)
            .prop_map(|chars: Vec<char>| chars.into_iter().collect::<String>())
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
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::*;

    fn crate_name() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::char::range('a', 'z'), 1..32)
            .prop_map(|chars: Vec<char>| chars.into_iter().collect::<String>())
    }

    fn semver_version() -> impl Strategy<Value = String> {
        (0u32..100, 0u32..100, 0u32..100)
            .prop_map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
    }

    proptest! {
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
    }
}
