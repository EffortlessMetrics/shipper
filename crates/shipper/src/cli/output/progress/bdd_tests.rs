use super::ProgressReporter;

#[test]
fn bdd_given_progress_reporting_when_a_single_package_is_marked_then_state_tracks_name_and_index() {
    // Given: a silent reporter used by non-TTY workflows.
    let mut reporter = ProgressReporter::silent(3);

    // When: we mark package progress.
    reporter.set_package(1, "demo", "0.1.0");
    reporter.set_status("Preparing metadata");

    // Then: state reflects the selected package and package slot.
    assert_eq!(reporter.current_package(), 1);
    assert_eq!(reporter.current_name(), "demo@0.1.0");
}

#[test]
fn bdd_given_multiple_updates_when_packages_advance_then_last_state_is_reflected() {
    // Given: an active publish pipeline with two packages.
    let mut reporter = ProgressReporter::silent(2);

    // When: we advance through package sequence.
    reporter.set_package(1, "alpha", "0.1.0");
    reporter.finish_package();
    reporter.set_package(2, "beta", "0.2.0");

    // Then: final state matches the latest package and finish is safe.
    assert_eq!(reporter.current_package(), 2);
    assert_eq!(reporter.current_name(), "beta@0.2.0");

    reporter.set_status("Finalizing");
    reporter.finish();
}
