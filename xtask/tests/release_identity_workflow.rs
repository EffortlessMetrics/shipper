//! Execute the Linux release gate's actual Bash with the production validator.
//! Only the GitHub API and Cargo metadata are bounded fixture inputs. No token,
//! network access, release dispatch, or publication is needed.

#[cfg(unix)]
#[test]
fn tag_identity_reads_use_explicit_auth_and_reject_invalid_reads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    let output = std::process::Command::new("python3")
        .arg("scripts/ci/test-release-identity-gate.py")
        .arg("--xtask")
        .arg(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .output()
        .expect("execute the release gate fixture with python3");
    assert!(
        output.status.success(),
        "release identity workflow fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
