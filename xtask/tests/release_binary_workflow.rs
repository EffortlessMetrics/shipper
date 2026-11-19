//! Exercise the actual release workflow Bash and packaged-executable verifier.
//! The native probe is a synthetic protocol fixture, not platform qualification
//! of Shipper. Real native release archives remain the release matrix's duty.

#[cfg(unix)]
#[test]
fn packaged_executable_proof_rejects_invalid_payloads_and_missing_native_evidence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    let output = std::process::Command::new("python3")
        .arg("scripts/ci/test-release-binary-proof.py")
        .current_dir(root)
        .output()
        .expect("execute packaged executable regression fixtures with python3");
    assert!(
        output.status.success(),
        "packaged executable proof fixtures failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
