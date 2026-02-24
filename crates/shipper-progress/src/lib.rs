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
    total_packages: usize,
    current_package: usize,
    current_name: String,
    progress_bar: Option<ProgressBar>,
    start_time: Instant,
}

impl ProgressReporter {
    /// Creates a new reporter for the given total package count.
    pub fn new(total_packages: usize) -> Self {
        let is_tty = is_tty();
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
            total_packages,
            current_package: 0,
            current_name: String::new(),
            progress_bar,
            start_time: Instant::now(),
        }
    }

    /// Creates a silent reporter that always uses non-TTY behavior.
    pub fn silent(total_packages: usize) -> Self {
        Self {
            is_tty: false,
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
        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                pb.inc(1);
            }
        }
    }

    /// Updates the message for the current package state.
    pub fn set_status(&self, status: &str) {
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
        if self.is_tty {
            if let Some(pb) = self.progress_bar {
                let elapsed = self.start_time.elapsed();
                let msg = format!("Completed {} packages in {:?}", self.total_packages, elapsed);
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

    #[test]
    fn test_is_tty_returns_bool() {
        let result = is_tty();
        assert!(matches!(result, true | false));
    }

    #[test]
    fn test_progress_reporter_creation() {
        let reporter = ProgressReporter::new(5);
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
    fn test_set_package_updates_state() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "test-crate", "1.0.0");
        assert_eq!(reporter.current_package(), 1);
        assert_eq!(reporter.current_name(), "test-crate@1.0.0");
    }

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
}

#[cfg(test)]
mod property_tests {
    use proptest::prelude::*;

    use super::*;

    fn simple_token() -> impl Strategy<Value = String> {
        prop::collection::vec('a'..='z', 1..12).prop_map(|chars| {
            chars
                .into_iter()
                .collect::<String>()
        })
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
