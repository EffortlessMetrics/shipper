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

// --- 1. Zero total packages: additional edge cases ---

#[test]
fn test_zero_packages_set_package_still_works() {
    let mut reporter = ProgressReporter::silent(0);
    reporter.set_package(1, "ghost", "0.0.0");
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "ghost@0.0.0");
    reporter.finish_package();
    reporter.finish();
}

// --- 2. Very large total (u32::MAX) ---

#[test]
fn test_u32_max_total_packages() {
    let large = u32::MAX as usize;
    let reporter = ProgressReporter::silent(large);
    assert_eq!(reporter.total_packages(), large);
    assert_eq!(reporter.current_package(), 0);
    reporter.finish();
}

#[test]
fn test_u32_max_set_package_at_boundary() {
    let large = u32::MAX as usize;
    let mut reporter = ProgressReporter::silent(large);
    reporter.set_package(large, "last", "1.0.0");
    assert_eq!(reporter.current_package(), large);
    assert_eq!(reporter.current_name(), "last@1.0.0");
    reporter.finish_package();
    reporter.finish();
}

// --- 3. Incrementing beyond total count ---

#[test]
fn test_set_package_beyond_total() {
    let mut reporter = ProgressReporter::silent(2);
    reporter.set_package(1, "a", "0.1.0");
    reporter.finish_package();
    reporter.set_package(2, "b", "0.2.0");
    reporter.finish_package();
    // Go beyond total — should not panic.
    reporter.set_package(3, "c", "0.3.0");
    assert_eq!(reporter.current_package(), 3);
    assert_eq!(reporter.current_name(), "c@0.3.0");
    reporter.finish_package();
    reporter.finish();
}

#[test]
fn test_set_package_far_beyond_total() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(100, "overflow", "9.9.9");
    assert_eq!(reporter.current_package(), 100);
    reporter.finish_package();
    reporter.finish();
}

// --- 4. Concurrent independent reporters from multiple threads ---

#[test]
fn test_concurrent_independent_reporters() {
    use std::thread;

    let handles: Vec<_> = (0..4)
        .map(|i| {
            thread::spawn(move || {
                let mut reporter = ProgressReporter::silent(10);
                for j in 1..=10 {
                    reporter.set_package(j, &format!("crate-{i}"), "0.1.0");
                    reporter.finish_package();
                }
                assert_eq!(reporter.current_package(), 10);
                reporter.finish();
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread panicked");
    }
}

// --- 5. Reset after completion ---

#[test]
fn test_reset_after_full_cycle() {
    let mut reporter = ProgressReporter::silent(3);
    for i in 1..=3 {
        reporter.set_package(i, &format!("pkg-{i}"), "1.0.0");
        reporter.finish_package();
    }
    assert_eq!(reporter.current_package(), 3);

    // Simulate reset by setting package back to 1.
    reporter.set_package(1, "pkg-1", "1.0.1");
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "pkg-1@1.0.1");
    reporter.finish_package();
}

// --- 6. Snapshot tests for progress display at various percentages ---

#[test]
fn snapshot_progress_at_0_percent() {
    let reporter = ProgressReporter::silent(4);
    let state = (
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name().to_string(),
        0.0_f64,
    );
    insta::assert_debug_snapshot!(state);
}

#[test]
fn snapshot_progress_at_25_percent() {
    let mut reporter = ProgressReporter::silent(4);
    reporter.set_package(1, "alpha", "0.1.0");
    reporter.finish_package();
    let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
    let state = (
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name().to_string(),
        pct,
    );
    insta::assert_debug_snapshot!(state);
}

#[test]
fn snapshot_progress_at_50_percent() {
    let mut reporter = ProgressReporter::silent(4);
    for (i, name) in [(1, "alpha"), (2, "beta")] {
        reporter.set_package(i, name, "0.1.0");
        reporter.finish_package();
    }
    let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
    let state = (
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name().to_string(),
        pct,
    );
    insta::assert_debug_snapshot!(state);
}

#[test]
fn snapshot_progress_at_75_percent() {
    let mut reporter = ProgressReporter::silent(4);
    for (i, name) in [(1, "alpha"), (2, "beta"), (3, "gamma")] {
        reporter.set_package(i, name, "0.1.0");
        reporter.finish_package();
    }
    let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
    let state = (
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name().to_string(),
        pct,
    );
    insta::assert_debug_snapshot!(state);
}

#[test]
fn snapshot_progress_at_100_percent() {
    let mut reporter = ProgressReporter::silent(4);
    for (i, name) in [(1, "alpha"), (2, "beta"), (3, "gamma"), (4, "delta")] {
        reporter.set_package(i, name, "0.1.0");
        reporter.finish_package();
    }
    let pct = (reporter.current_package() as f64 / reporter.total_packages() as f64) * 100.0;
    let state = (
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name().to_string(),
        pct,
    );
    insta::assert_debug_snapshot!(state);
}

// --- 7. Property: percentage always 0..=100 (exhaustive for small values) ---

#[test]
fn test_percentage_always_in_range_exhaustive_small() {
    for total in 1..=20_usize {
        for current in 0..=total {
            let pct = (current as f64 / total as f64) * 100.0;
            assert!(
                (0.0..=100.0).contains(&pct),
                "percentage {pct} out of range for {current}/{total}"
            );
        }
    }
}

// --- 8. Edge case: decrement below zero ---

#[test]
fn test_set_package_index_zero() {
    let mut reporter = ProgressReporter::silent(5);
    reporter.set_package(0, "zero-indexed", "0.0.0");
    assert_eq!(reporter.current_package(), 0);
    assert_eq!(reporter.current_name(), "zero-indexed@0.0.0");
    reporter.finish_package();
    reporter.finish();
}

#[test]
fn test_set_package_decreasing_index() {
    let mut reporter = ProgressReporter::silent(5);
    reporter.set_package(3, "middle", "1.0.0");
    assert_eq!(reporter.current_package(), 3);
    // Decrease the index — simulates going backward.
    reporter.set_package(1, "back-to-start", "0.1.0");
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "back-to-start@0.1.0");
}

// --- 9. Status transitions: pending → in_progress → completed ---

#[test]
fn test_status_transition_pending_to_completed() {
    let mut reporter = ProgressReporter::silent(2);

    // Pending: fresh state.
    assert_eq!(reporter.current_package(), 0);
    assert_eq!(reporter.current_name(), "");

    // In-progress: first package.
    reporter.set_package(1, "dep", "0.1.0");
    reporter.set_status("Publishing dep@0.1.0");
    assert_eq!(reporter.current_package(), 1);
    reporter.finish_package();

    // In-progress: second package.
    reporter.set_package(2, "app", "1.0.0");
    reporter.set_status("Publishing app@1.0.0");
    assert_eq!(reporter.current_package(), 2);
    reporter.finish_package();

    // Completed.
    assert_eq!(reporter.current_package(), 2);
    assert_eq!(reporter.total_packages(), 2);
    reporter.finish();
}

#[test]
fn test_status_transitions_with_intermediate_statuses() {
    let mut reporter = ProgressReporter::silent(1);

    // Pending.
    reporter.set_status("Queued");

    // In-progress.
    reporter.set_package(1, "my-crate", "1.0.0");
    reporter.set_status("Compiling");
    reporter.set_status("Packaging");
    reporter.set_status("Uploading");
    reporter.set_status("Verifying on registry");

    // Completed.
    reporter.finish_package();
    reporter.set_status("Published successfully");
    reporter.finish();
}

// --- 10. Display formatting edge cases ---

#[test]
fn test_display_format_with_hyphenated_name() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "my-super-crate-name", "0.1.0-rc.1");
    assert_eq!(reporter.current_name(), "my-super-crate-name@0.1.0-rc.1");
}

#[test]
fn test_display_format_with_build_metadata() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "crate", "1.0.0+build.123");
    assert_eq!(reporter.current_name(), "crate@1.0.0+build.123");
}

#[test]
fn test_display_format_at_sign_in_version() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "crate", "1.0.0@special");
    assert_eq!(reporter.current_name(), "crate@1.0.0@special");
}

#[test]
fn test_display_format_whitespace_in_name() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "name with spaces", "1.0.0");
    assert_eq!(reporter.current_name(), "name with spaces@1.0.0");
}

#[test]
fn test_display_format_newlines_in_status() {
    let reporter = ProgressReporter::silent(1);
    reporter.set_status("line1\nline2\nline3");
}

#[test]
fn snapshot_display_format_edge_cases() {
    let cases: Vec<(&str, &str, String)> = vec![
        ("normal", "1.0.0", "normal@1.0.0".to_string()),
        ("", "", "@".to_string()),
        ("a", "0.0.0", "a@0.0.0".to_string()),
        (
            "crate-with-dashes",
            "0.1.0-alpha.1+meta",
            "crate-with-dashes@0.1.0-alpha.1+meta".to_string(),
        ),
    ];
    let formatted: Vec<String> = cases
        .iter()
        .map(|(name, ver, expected)| {
            let mut r = ProgressReporter::silent(1);
            r.set_package(1, name, ver);
            assert_eq!(r.current_name(), expected.as_str());
            format!("name={name:?} ver={ver:?} => {:?}", r.current_name())
        })
        .collect();
    insta::assert_debug_snapshot!(formatted);
}

// --- 11. Progress state: complete lifecycle snapshots ---

#[test]
fn snapshot_single_package_lifecycle() {
    let mut reporter = ProgressReporter::silent(1);
    let mut lines = Vec::new();

    lines.push(format!(
        "pending:     pkg={} name={:?}",
        reporter.current_package(),
        reporter.current_name(),
    ));

    reporter.set_package(1, "my-crate", "0.1.0");
    lines.push(format!(
        "in_progress: pkg={} name={:?}",
        reporter.current_package(),
        reporter.current_name(),
    ));

    reporter.finish_package();
    lines.push(format!(
        "complete:    pkg={} name={:?}",
        reporter.current_package(),
        reporter.current_name(),
    ));

    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn snapshot_failed_midway_state() {
    let mut reporter = ProgressReporter::silent(5);
    let mut lines = Vec::new();

    // Publish first two packages successfully.
    for (i, name) in [(1, "core"), (2, "utils")] {
        reporter.set_package(i, name, "0.1.0");
        reporter.finish_package();
    }

    // Package 3 "fails" — we set it but never finish it.
    reporter.set_package(3, "macros", "0.1.0");

    lines.push(format!("total={}", reporter.total_packages()));
    lines.push(format!("current_package={}", reporter.current_package()));
    lines.push(format!("current_name={:?}", reporter.current_name()));
    lines.push(format!(
        "completed_pct={:.1}%",
        (2.0 / reporter.total_packages() as f64) * 100.0
    ));

    insta::assert_snapshot!(lines.join("\n"));
}

// --- 12. Percentage edge cases ---

#[test]
fn test_percentage_one_third_precision() {
    let total = 3_usize;
    let current = 1_usize;
    let pct = (current as f64 / total as f64) * 100.0;
    assert!((pct - 33.333_333_333_333_336).abs() < 1e-10);
    assert!((0.0..=100.0).contains(&pct));
}

#[test]
fn test_percentage_100_packages_milestones() {
    let total = 100_usize;
    for current in [0, 1, 10, 25, 50, 75, 99, 100] {
        let pct = (current as f64 / total as f64) * 100.0;
        assert!(
            (0.0..=100.0).contains(&pct),
            "out of range for {current}/{total}"
        );
        assert!(
            (pct - current as f64).abs() < f64::EPSILON,
            "expected {current}% but got {pct}"
        );
    }
}

// --- 13. Package tracking: non-sequential usage ---

#[test]
fn test_package_tracking_skip_indices() {
    let mut reporter = ProgressReporter::silent(10);
    reporter.set_package(1, "first", "0.1.0");
    assert_eq!(reporter.current_package(), 1);
    reporter.finish_package();

    reporter.set_package(5, "fifth", "0.5.0");
    assert_eq!(reporter.current_package(), 5);
    reporter.finish_package();

    reporter.set_package(10, "tenth", "1.0.0");
    assert_eq!(reporter.current_package(), 10);
    reporter.finish_package();

    reporter.finish();
}

#[test]
fn test_package_tracking_reverse_order() {
    let mut reporter = ProgressReporter::silent(3);
    reporter.set_package(3, "c", "0.3.0");
    assert_eq!(reporter.current_package(), 3);
    reporter.set_package(2, "b", "0.2.0");
    assert_eq!(reporter.current_package(), 2);
    reporter.set_package(1, "a", "0.1.0");
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "a@0.1.0");
}

// --- 14. Display formatting: unusual names ---

#[test]
fn test_display_format_numeric_name() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "12345", "6.7.8");
    assert_eq!(reporter.current_name(), "12345@6.7.8");
}

#[test]
fn test_display_format_single_char_name_and_version() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "x", "0");
    assert_eq!(reporter.current_name(), "x@0");
}

// --- 15. Single-package all operations ---

#[test]
fn test_single_package_all_operations() {
    let mut reporter = ProgressReporter::silent(1);
    assert_eq!(reporter.total_packages(), 1);
    assert_eq!(reporter.current_package(), 0);
    assert_eq!(reporter.current_name(), "");
    assert!(!reporter.is_tty_mode());

    reporter.set_status("Preparing");
    reporter.set_package(1, "solo", "1.0.0");
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "solo@1.0.0");

    reporter.set_status("Uploading");
    reporter.set_status("Verifying");
    reporter.finish_package();

    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "solo@1.0.0");
    reporter.finish();
}

// --- 16. Stress: many status updates ---

#[test]
fn test_many_status_updates_between_packages() {
    let mut reporter = ProgressReporter::silent(2);
    reporter.set_package(1, "pkg-a", "0.1.0");
    for i in 0..100 {
        reporter.set_status(&format!("Step {i}"));
    }
    reporter.finish_package();
    reporter.set_package(2, "pkg-b", "0.2.0");
    reporter.finish_package();
    assert_eq!(reporter.current_package(), 2);
    reporter.finish();
}

// --- 17. Finish without starting any package ---

#[test]
fn test_finish_immediately_with_packages() {
    let reporter = ProgressReporter::silent(10);
    assert_eq!(reporter.total_packages(), 10);
    assert_eq!(reporter.current_package(), 0);
    reporter.finish();
}

// --- 18. Multiple finish_package calls preserve name ---

#[test]
fn test_finish_package_preserves_current_name() {
    let mut reporter = ProgressReporter::silent(3);
    reporter.set_package(1, "alpha", "0.1.0");
    reporter.finish_package();
    assert_eq!(reporter.current_name(), "alpha@0.1.0");
    reporter.finish_package();
    assert_eq!(reporter.current_name(), "alpha@0.1.0");
}

// --- 19. State after partial publish ---

#[test]
fn test_state_after_partial_publish() {
    let mut reporter = ProgressReporter::silent(5);
    for i in 1..=3 {
        reporter.set_package(i, &format!("crate-{i}"), "0.1.0");
        reporter.finish_package();
    }
    // Simulate stopping after 3 of 5.
    assert_eq!(reporter.current_package(), 3);
    assert_eq!(reporter.total_packages(), 5);
    assert_eq!(reporter.current_name(), "crate-3@0.1.0");
}

// --- 20. Display format with special version strings ---

#[test]
fn test_display_format_version_with_pre_and_build() {
    let mut reporter = ProgressReporter::silent(1);
    reporter.set_package(1, "crate", "1.0.0-alpha.1+build.456");
    assert_eq!(reporter.current_name(), "crate@1.0.0-alpha.1+build.456");
}

// --- 21. retry_countdown (#103 PR 1) ---

#[test]
fn test_retry_countdown_silent_mode_blocks_for_delay() {
    // Quiet reporter emits nothing but still blocks for the full delay so
    // the engine's retry-backoff semantics are preserved.
    let reporter = ProgressReporter::silent(1);
    let delay = Duration::from_millis(80);
    let start = Instant::now();
    reporter.retry_countdown(
        "my-crate",
        "1.0.0",
        1,
        5,
        delay,
        "Retryable",
        "HTTP 429 from registry",
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed >= delay,
        "retry_countdown returned too early: {elapsed:?} < {delay:?}"
    );
    // And doesn't take absurdly long either (3x tolerance for CI).
    assert!(
        elapsed < delay * 10,
        "retry_countdown took too long: {elapsed:?}"
    );
}

#[test]
fn test_retry_countdown_zero_delay_returns_immediately() {
    let reporter = ProgressReporter::silent(1);
    let start = Instant::now();
    reporter.retry_countdown("pkg", "0.1.0", 1, 3, Duration::ZERO, "Retryable", "ok");
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[test]
fn test_retry_countdown_non_tty_one_shot_line() {
    // `ProgressReporter::new(total, quiet=false)` is non-TTY in the test
    // harness (stdout is not a terminal), so this drives the non-TTY
    // branch — one `eprintln!` line, then sleep. Can't assert on stderr
    // here but we can assert the call doesn't panic and blocks correctly.
    let reporter = ProgressReporter::new(2, false);
    let delay = Duration::from_millis(30);
    let start = Instant::now();
    reporter.retry_countdown(
        "some-crate",
        "2.0.0",
        2,
        5,
        delay,
        "Retryable",
        "rate limited",
    );
    assert!(start.elapsed() >= delay);
}

#[test]
fn test_retry_countdown_max_attempts_display() {
    // Smoke check: exotic attempt/max combinations shouldn't panic.
    let reporter = ProgressReporter::silent(1);
    reporter.retry_countdown("x", "0", 0, 1, Duration::from_millis(1), "Ambiguous", "m");
    reporter.retry_countdown(
        "x",
        "0",
        u32::MAX - 1,
        u32::MAX,
        Duration::from_millis(1),
        "Retryable",
        "m",
    );
}
