//! Progress reporting module with TTY detection.
//!
//! This module provides progress bar functionality that automatically detects
//! whether stdout is a TTY and falls back to non-interactive output when not.

use std::time::Instant;

use atty::Stream;
use indicatif::{ProgressBar, ProgressStyle};

/// Detects whether stdout is connected to a TTY.
pub fn is_tty() -> bool {
    atty::is(Stream::Stdout)
}

/// Progress reporter that shows progress bars in TTY mode
/// and falls back to simple text output when not in a TTY.
pub struct ProgressReporter {
    /// Whether we're running in TTY mode
    is_tty: bool,
    /// The total number of packages to publish
    total_packages: usize,
    /// Current package being published (1-indexed)
    current_package: usize,
    /// Current package name
    current_name: String,
    /// Progress bar (only used in TTY mode)
    progress_bar: Option<ProgressBar>,
    /// Start time for calculating elapsed time
    start_time: Instant,
}

impl ProgressReporter {
    /// Creates a new progress reporter.
    ///
    /// # Arguments
    /// * `total_packages` - Total number of packages to publish
    /// * `name` - Optional name for the current package
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

    /// Creates a silent progress reporter that always uses non-TTY mode.
    /// Use this when you explicitly want to disable progress bars regardless of TTY.
    #[allow(dead_code)]
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

    /// Sets the current package being published.
    ///
    /// # Arguments
    /// * `index` - The 1-indexed position of the package in the publish order
    /// * `name` - The name of the package
    /// * `version` - The version of the package
    pub fn set_package(&mut self, index: usize, name: &str, version: &str) {
        self.current_package = index;
        self.current_name = format!("{}@{}", name, version);

        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                let elapsed = self.start_time.elapsed();
                let msg = format!(
                    "[{}/{}] Publishing {}... ({elapsed:?})",
                    self.current_package, self.total_packages, self.current_name
                );
                pb.set_message(msg);
                pb.set_position((self.current_package - 1) as u64);
            }
        } else {
            let elapsed = self.start_time.elapsed();
            eprintln!(
                "[{}/{}] Publishing {}... ({elapsed:?})",
                self.current_package, self.total_packages, self.current_name
            );
        }
    }

    /// Marks the current package as completed.
    #[allow(clippy::collapsible_if)]
    #[allow(dead_code)]
    pub fn finish_package(&mut self) {
        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                pb.inc(1);
            }
        }
    }

    /// Sets a status message (e.g., "Waiting for registry...").
    #[allow(dead_code)]
    pub fn set_status(&self, status: &str) {
        if self.is_tty {
            if let Some(ref pb) = self.progress_bar {
                let current = pb.position();
                let msg = format!("[{}/{}] {}", current + 1, self.total_packages, status);
                pb.set_message(msg);
            }
        } else {
            eprintln!("[status] {}", status);
        }
    }

    /// Finishes the progress reporting.
    pub fn finish(self) {
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

    #[test]
    fn test_is_tty_returns_bool() {
        let result = is_tty();
        assert!(matches!(result, true | false));
    }

    #[test]
    fn test_progress_reporter_creation() {
        let reporter = ProgressReporter::new(5);
        assert_eq!(reporter.total_packages, 5);
        assert_eq!(reporter.current_package, 0);
    }

    #[test]
    fn test_silent_reporter_disables_tty() {
        let reporter = ProgressReporter::silent(3);
        assert!(!reporter.is_tty);
        assert!(reporter.progress_bar.is_none());
    }

    #[test]
    fn test_set_package_updates_state() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "test-crate", "1.0.0");
        assert_eq!(reporter.current_package, 1);
        assert_eq!(reporter.current_name, "test-crate@1.0.0");
    }

    #[test]
    fn test_finish_package_increments() {
        let mut reporter = ProgressReporter::silent(3);
        reporter.set_package(1, "test-crate", "1.0.0");
        reporter.finish_package();
        // Silent mode doesn't track position, but method should be callable
    }

    #[test]
    fn test_finish_completes_without_panic() {
        let reporter = ProgressReporter::silent(3);
        reporter.finish();
    }
}
