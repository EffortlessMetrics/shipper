//! Repository fixture tests for the retained pre-Changie history boundary.
//!
//! These tests do not execute Changie or turn fragment intake into a CI gate.
//! They prove that the tracked header plus the opaque 0.5.0 baseline still
//! reconstruct the tracked changelog and that the config retains the newline
//! contract used by the local `cargo changelog-roundtrip` command.

use std::fs;
use std::path::{Path, PathBuf};

const RELEASE_HEADINGS: [&str; 7] = [
    "## [0.5.0] - Unreleased",
    "## [0.4.0] - 2026-05-20",
    "## [0.4.0-rc.1] - 2026-05-12",
    "## [0.3.0-rc.2] - 2026-04-18",
    "## [0.3.0-rc.1] - 2026-02-27",
    "## [0.2.0] - 2026-02-14",
    "## [0.1.0] - 2025-01-15",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live directly under the repository root")
        .to_path_buf()
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

fn normalize_final_newline(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

#[test]
fn retained_baseline_reconstructs_the_tracked_changelog() {
    let root = repository_root();
    let header = read(&root, ".changes/header.tpl.md");
    let baseline = read(&root, ".changes/0.5.0.md");
    let tracked = read(&root, "CHANGELOG.md");
    let reconstructed = format!(
        "{}\n\n{}",
        normalize_final_newline(&header),
        normalize_final_newline(&baseline)
    );

    assert_eq!(
        normalize_final_newline(&reconstructed),
        normalize_final_newline(&tracked),
        "the retained pre-Changie history no longer reconstructs CHANGELOG.md"
    );
}

#[test]
fn baseline_owns_every_pre_changie_release_section_once() {
    let root = repository_root();
    let header = read(&root, ".changes/header.tpl.md");
    let baseline = read(&root, ".changes/0.5.0.md");

    assert!(
        header.contains("## [Unreleased]"),
        "the header must own the Unreleased boundary"
    );
    assert!(
        !baseline.contains("## [Unreleased]"),
        "the historical baseline must not duplicate the Unreleased boundary"
    );

    for heading in RELEASE_HEADINGS {
        assert_eq!(
            baseline.matches(heading).count(),
            1,
            "historical heading `{heading}` must appear exactly once"
        );
    }

    let release_heading_count = baseline
        .lines()
        .filter(|line| line.starts_with("## ["))
        .count();
    assert_eq!(
        release_heading_count,
        RELEASE_HEADINGS.len(),
        "add new post-baseline versions as their own `.changes/<version>.md` files; do not fold them into the 0.5.0 baseline"
    );
}

#[test]
fn changie_config_preserves_the_baseline_render_contract() {
    let root = repository_root();
    let config = read(&root, ".changie.yaml");

    for required in [
        "headerPath: header.tpl.md",
        "changelogPath: CHANGELOG.md",
        "versionExt: md",
        "kindFormat: '### {{.Kind}}'",
        "afterChangelogHeader: 1",
        "beforeChangelogVersion: 0",
    ] {
        assert!(
            config.contains(required),
            "Changie config must retain `{required}` or the baseline renderer and docs must change together"
        );
    }
}
