//! Progress reporting primitives for CLI flows.
//!
//! Originally the standalone `shipper-progress` crate; absorbed into
//! `shipper-cli::output::progress` as a crate-private module because it is
//! CLI-only and has no upstream library consumer.

use std::io::IsTerminal;
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};

#[cfg(test)]
mod bdd_tests;
#[cfg(test)]
mod proptests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;

/// Returns `true` when standard output is connected to a terminal.
pub(crate) fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Progress reporter that emits an interactive progress bar in TTY mode and
/// falls back to non-interactive text output otherwise.
pub(crate) struct ProgressReporter {
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
    pub(crate) fn new(total_packages: usize, quiet: bool) -> Self {
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
    #[cfg(test)]
    pub(crate) fn silent(total_packages: usize) -> Self {
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
    #[cfg(test)]
    pub(crate) fn is_tty_mode(&self) -> bool {
        self.is_tty
    }

    /// Returns the configured package count.
    #[cfg(test)]
    pub(crate) fn total_packages(&self) -> usize {
        self.total_packages
    }

    /// Returns the current 1-indexed package position.
    #[cfg(test)]
    pub(crate) fn current_package(&self) -> usize {
        self.current_package
    }

    /// Returns the currently active package label (`name@version`).
    #[cfg(test)]
    pub(crate) fn current_name(&self) -> &str {
        &self.current_name
    }

    /// Records the active package being published.
    pub(crate) fn set_package(&mut self, index: usize, name: &str, version: &str) {
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
    ///
    /// Currently only exercised by tests; retained as part of the reporter API
    /// for future publish-flow changes that report per-package completion.
    #[allow(dead_code)]
    pub(crate) fn finish_package(&mut self) {
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
    ///
    /// Used by [`ProgressReporter::retry_countdown`] to render the live retry
    /// countdown, and available for future intra-package status updates
    /// (uploading, verifying, etc.).
    pub(crate) fn set_status(&self, status: &str) {
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

    /// Render a live retry-backoff countdown and block for the full `delay`.
    ///
    /// In TTY mode, refreshes the progress-bar message every second with a
    /// ticking `"retrying {pkg}@{ver} in {Ns}... (attempt N/M, reason: ...)"`
    /// line so operators watching a CI log never see a silent sleep. In
    /// non-TTY mode, emits a single one-shot `eprintln!` (avoiding stream
    /// spam in log files) and then sleeps. In quiet mode, skips narration
    /// entirely but still blocks for the backoff. Closes #103 PR 1 — lifts
    /// `set_status` off the dead-code list by wiring it to the retry path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn retry_countdown(
        &self,
        pkg_name: &str,
        pkg_version: &str,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        reason: &str,
        message: &str,
    ) {
        // Quiet mode: preserve the retry wait but emit nothing.
        if self.quiet {
            thread::sleep(delay);
            return;
        }

        let next_attempt = attempt.saturating_add(1);

        if self.is_tty {
            let start = Instant::now();
            let tick = Duration::from_millis(1000);
            // Tick down once per second. `remaining == 0` ends the loop; we
            // still call `set_status` once on exit to show "retrying now".
            loop {
                let elapsed = start.elapsed();
                let remaining = delay.saturating_sub(elapsed);
                let remaining_secs = remaining.as_secs();

                if remaining.is_zero() {
                    self.set_status(&format!(
                        "retrying {pkg_name}@{pkg_version} now... (attempt {next_attempt}/{max_attempts}, reason: {reason})"
                    ));
                    break;
                }

                self.set_status(&format!(
                    "retrying {pkg_name}@{pkg_version} in {remaining_secs}s... (attempt {next_attempt}/{max_attempts}, reason: {reason}) — {message}"
                ));

                let sleep_for = remaining.min(tick);
                thread::sleep(sleep_for);
            }
        } else {
            // Non-TTY: one-shot line so pipelines/CI logs don't get spammed
            // with per-second updates. Matches the pre-#103 warn shape.
            eprintln!(
                "[retry] {pkg_name}@{pkg_version}: {message} ({reason}); next attempt in {} (attempt {next_attempt}/{max_attempts})",
                humantime::format_duration(delay),
            );
            thread::sleep(delay);
        }
    }

    /// Finishes reporting and prints completion summary in non-TTY mode.
    pub(crate) fn finish(self) {
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
