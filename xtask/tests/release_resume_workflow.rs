//! The production resume selector consumes the explicitly selected run only.

#[cfg(unix)]
#[test]
fn resume_artifact_selector_rejects_unusable_or_ambiguous_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    let output = std::process::Command::new("python3")
        .arg("scripts/ci/test-release-resume-artifacts.py")
        .current_dir(root)
        .output()
        .expect("execute the resume artifact selector fixture with python3");
    assert!(
        output.status.success(),
        "resume artifact selector fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
