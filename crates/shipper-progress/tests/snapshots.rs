use shipper_progress::ProgressReporter;

// ---------------------------------------------------------------------------
// Helper: build a display‐format string identical to the non‐TTY messages
// produced by `set_package` / `finish_package` / `finish`, but without the
// elapsed‐time component (which is non‐deterministic).
// ---------------------------------------------------------------------------

fn publish_msg(current: usize, total: usize, name: &str, version: &str) -> String {
    format!("[{current}/{total}] Publishing {name}@{version}...")
}

fn finish_msg(current: usize, total: usize, name: &str, version: &str) -> String {
    format!("[{current}/{total}] Finished {name}@{version}")
}

fn completion_msg(total: usize) -> String {
    format!("Completed {total}/{total} packages")
}

// ---------------------------------------------------------------------------
// Progress display output snapshots
// ---------------------------------------------------------------------------

#[test]
fn display_single_package() {
    let output = publish_msg(1, 1, "my-crate", "0.1.0");
    insta::assert_snapshot!("display_single_package", output);
}

#[test]
fn display_multi_package_sequence() {
    let packages = [
        (1, "core", "0.1.0"),
        (2, "utils", "0.2.0"),
        (3, "macros", "0.3.0"),
        (4, "cli", "1.0.0"),
    ];
    let lines: Vec<String> = packages
        .iter()
        .flat_map(|&(idx, name, ver)| {
            vec![
                publish_msg(idx, 4, name, ver),
                finish_msg(idx, 4, name, ver),
            ]
        })
        .collect();
    let output = format!("{}\n{}", lines.join("\n"), completion_msg(4));
    insta::assert_snapshot!("display_multi_package_sequence", output);
}

#[test]
fn display_prerelease_version() {
    let output = publish_msg(1, 1, "my-lib", "2.0.0-beta.3");
    insta::assert_snapshot!("display_prerelease_version", output);
}

#[test]
fn display_empty_name_and_version() {
    let output = publish_msg(1, 1, "", "");
    insta::assert_snapshot!("display_empty_name_and_version", output);
}

// ---------------------------------------------------------------------------
// Percentage formatting snapshots
// ---------------------------------------------------------------------------

fn percentage(current: usize, total: usize) -> String {
    if total == 0 {
        return "N/A (0 packages)".to_string();
    }
    let pct = (current as f64 / total as f64) * 100.0;
    format!("{current}/{total} = {pct:.1}%")
}

#[test]
fn percentage_milestones() {
    let total = 10;
    let milestones = [0, 1, 2, 5, 7, 10];
    let lines: Vec<String> = milestones
        .iter()
        .map(|&current| percentage(current, total))
        .collect();
    insta::assert_snapshot!("percentage_milestones", lines.join("\n"));
}

#[test]
fn percentage_single_package() {
    let lines = [percentage(0, 1), percentage(1, 1)];
    insta::assert_snapshot!("percentage_single_package", lines.join("\n"));
}

#[test]
fn percentage_three_packages() {
    let lines: Vec<String> = (0..=3).map(|i| percentage(i, 3)).collect();
    insta::assert_snapshot!("percentage_three_packages", lines.join("\n"));
}

#[test]
fn percentage_zero_total() {
    insta::assert_snapshot!("percentage_zero_total", percentage(0, 0));
}

#[test]
fn percentage_large_workspace() {
    let total = 50;
    let points = [0, 1, 10, 25, 49, 50];
    let lines: Vec<String> = points
        .iter()
        .map(|&current| percentage(current, total))
        .collect();
    insta::assert_snapshot!("percentage_large_workspace", lines.join("\n"));
}

// ---------------------------------------------------------------------------
// Multi-package progress state snapshots
// ---------------------------------------------------------------------------

fn state_snapshot(reporter: &ProgressReporter) -> String {
    format!(
        "total={} current={} name={:?} tty={}",
        reporter.total_packages(),
        reporter.current_package(),
        reporter.current_name(),
        reporter.is_tty_mode(),
    )
}

#[test]
fn state_fresh_reporter() {
    let reporter = ProgressReporter::silent(5);
    insta::assert_snapshot!("state_fresh_reporter", state_snapshot(&reporter));
}

#[test]
fn state_after_first_package() {
    let mut reporter = ProgressReporter::silent(3);
    reporter.set_package(1, "alpha", "0.1.0");
    insta::assert_snapshot!("state_after_first_package", state_snapshot(&reporter));
}

#[test]
fn state_full_lifecycle() {
    let mut reporter = ProgressReporter::silent(3);
    let packages = [
        (1, "core", "0.1.0"),
        (2, "utils", "0.2.0"),
        (3, "app", "1.0.0"),
    ];

    let mut lines = vec![format!("initial: {}", state_snapshot(&reporter))];

    for &(idx, name, ver) in &packages {
        reporter.set_package(idx, name, ver);
        lines.push(format!(
            "after set_package({idx}): {}",
            state_snapshot(&reporter)
        ));
        reporter.finish_package();
        lines.push(format!(
            "after finish_package({idx}): {}",
            state_snapshot(&reporter)
        ));
    }

    insta::assert_snapshot!("state_full_lifecycle", lines.join("\n"));
}

#[test]
fn state_overwrite_same_index() {
    let mut reporter = ProgressReporter::silent(2);
    reporter.set_package(1, "old-name", "0.0.1");
    let before = state_snapshot(&reporter);
    reporter.set_package(1, "new-name", "0.0.2");
    let after = state_snapshot(&reporter);
    insta::assert_snapshot!(
        "state_overwrite_same_index",
        format!("before: {before}\nafter:  {after}")
    );
}

#[test]
fn state_zero_packages() {
    let reporter = ProgressReporter::silent(0);
    insta::assert_snapshot!("state_zero_packages", state_snapshot(&reporter));
}
